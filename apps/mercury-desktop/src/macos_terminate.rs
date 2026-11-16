//! The system's terminate request (`-[NSApplication terminate:]`: a Quit menu item, the Dock's
//! Quit, logging out, restarting, shutting down) ends the run in order instead of ending the
//! process at once.
//!
//! winit 0.30's application delegate implements only `applicationDidFinishLaunching:` and
//! `applicationWillTerminate:` (winit-0.30.13/src/platform_impl/macos/app_state.rs:47-73), so
//! `terminate:` goes straight to `applicationWillTerminate:` and AppKit's `exit()`: the engine
//! is never shut down. This adds `applicationShouldTerminate:` to winit's delegate class at run
//! time. The answer depends on why the request came:
//!
//! - The session is ending (a quit Apple event whose `keyAEQuitReason` is a log-out, restart
//!   or shut-down): the post-run sequence runs inside the callback and the answer is
//!   `NSTerminateNow`, so the system's log-out goes on as soon as the run has ended. Answering
//!   "later" is no option: AppKit then runs its run loop in `NSModalPanelRunLoopMode` until
//!   `replyToApplicationShouldTerminate:`, so the event loop cannot end (tried: it hangs); and
//!   "cancel" would cancel the log-out.
//! - Anything else (no current Apple event: a menu item's `terminate:`; a quit Apple event
//!   without a reason: the Dock's Quit): the answer is `NSTerminateCancel`, and the event loop
//!   is asked to quit, so that `main` runs the same post-run sequence.

use std::ffi::CStr;
use std::time::Instant;

use objc2::ffi::{NSUInteger, class_addMethod, object_getClass};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Imp, NSObjectProtocol, Sel};
use objc2::{MainThreadMarker, class, msg_send, sel};
use objc2_app_kit::{NSApplication, NSApplicationTerminateReply};

/// `kCoreEventClass` (`'aevt'`, AppleEvents.h).
const CORE_EVENT_CLASS: u32 = four_char_code(b"aevt");
/// `kAEQuitApplication` (`'quit'`, AppleEvents.h).
const QUIT_APPLICATION: u32 = four_char_code(b"quit");
/// `kAEQuitReason` (`'why?'`, AERegistry.h): "in a kAEQuitApplication event, this parameter
/// if present is the reason the quit is being sent. The possible values are kAEQuitAll,
/// kAEShutDown, kAERestart, kAEReallyLogOut".
const QUIT_REASON: u32 = four_char_code(b"why?");
/// The reasons that mean the session is ending. Only `kAEReallyLogOut`, `kAERestart` and
/// `kAEShutDown` reach applications; `kAELogOut` and the two `Show…Dialog` codes are what is
/// sent to loginwindow (AERegistry.h: "events that can be sent to the system process"), and are
/// accepted too in case they are ever forwarded.
const SESSION_ENDING_REASONS: [u32; 6] = [
    four_char_code(b"logo"), // kAELogOut
    four_char_code(b"rlgo"), // kAEReallyLogOut
    four_char_code(b"rrst"), // kAEShowRestartDialog
    four_char_code(b"rest"), // kAERestart
    four_char_code(b"rsdn"), // kAEShowShutdownDialog
    four_char_code(b"shut"), // kAEShutDown
];
/// `kAEReallyLogOut`, what `--exit-by logout` sends.
const REALLY_LOG_OUT: u32 = four_char_code(b"rlgo");

/// `NSApplicationTerminateReply applicationShouldTerminate:(NSApplication *)sender`.
const SHOULD_TERMINATE_TYPES: &CStr = c"Q@:@";
/// `void mercuryDiagnosticLogOut:(id)sender`.
const DIAGNOSTIC_TYPES: &CStr = c"v@:@";

const fn four_char_code(code: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*code)
}

/// How a terminate request is answered.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TerminateDecision {
    /// Run the post-run sequence now, then answer `NSTerminateNow`.
    EndTheRunNow,
    /// Answer `NSTerminateCancel` and let the event loop quit; `main` ends the run.
    QuitThroughTheEventLoop,
}

/// `current_event`: the class, ID and quit reason of the Apple event being handled, if any.
pub(crate) fn decide(current_event: Option<(u32, u32, Option<u32>)>) -> TerminateDecision {
    match current_event {
        Some((CORE_EVENT_CLASS, QUIT_APPLICATION, Some(reason)))
            if SESSION_ENDING_REASONS.contains(&reason) =>
        {
            TerminateDecision::EndTheRunNow
        }
        _ => TerminateDecision::QuitThroughTheEventLoop,
    }
}

/// Call on the main thread once the backend (and with it winit's event loop and application
/// delegate) exists.
pub(crate) fn end_the_run_in_order_on_terminate_requests() {
    let Some(class) = delegate_class() else {
        return;
    };
    // SAFETY: the function's signature matches the type encoding: it returns an `NSUInteger`
    // and takes `self`, `_cmd` and one object.
    let imp = unsafe {
        std::mem::transmute::<
            unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject) -> NSUInteger,
            Imp,
        >(application_should_terminate)
    };
    if add_method(
        class,
        sel!(applicationShouldTerminate:),
        imp,
        SHOULD_TERMINATE_TYPES,
    ) {
        log::debug!("terminate requests end the run in order");
    }
}

/// winit's application delegate class, if it is there and lacks `applicationShouldTerminate:`.
fn delegate_class() -> Option<*mut AnyClass> {
    let Some(mtm) = MainThreadMarker::new() else {
        log::warn!("terminate requests are not handled: not on the main thread");
        return None;
    };
    let app = NSApplication::sharedApplication(mtm);
    let Some(delegate) = app.delegate() else {
        log::warn!("terminate requests are not handled: the application has no delegate");
        return None;
    };
    if delegate.respondsToSelector(sel!(applicationShouldTerminate:)) {
        log::warn!(
            "terminate requests are not handled: the application delegate already answers \
             applicationShouldTerminate:"
        );
        return None;
    }
    let delegate: &AnyObject = delegate.as_ref();
    // SAFETY: `delegate` is a live object, so this returns its class.
    Some(unsafe { object_getClass(delegate) }.cast_mut())
}

/// Adds a method the class does not have. Adding a method changes none of winit's own
/// behaviour; the methods added here read no instance state (they ignore `self`).
fn add_method(class: *mut AnyClass, selector: Sel, imp: Imp, types: &CStr) -> bool {
    // SAFETY: `class` is a registered class; `imp` matches `types` (checked by the callers).
    let added = unsafe { class_addMethod(class, selector, imp, types.as_ptr()) }.as_bool();
    if !added {
        log::warn!("could not add {selector} to the application delegate");
    }
    added
}

unsafe extern "C-unwind" fn application_should_terminate(
    _this: *mut AnyObject,
    _cmd: Sel,
    _sender: *mut AnyObject,
) -> NSUInteger {
    // Nothing may unwind into AppKit.
    let reply = std::panic::catch_unwind(|| match decide(current_apple_event()) {
        TerminateDecision::EndTheRunNow => {
            log::info!("the session is ending; ending the run before answering");
            let started = Instant::now();
            let ran = crate::post_run::finish(false).is_some();
            log::info!(
                "ended the run for the system in {} ms{}",
                started.elapsed().as_millis(),
                if ran { "" } else { " (it had already ended)" }
            );
            NSApplicationTerminateReply::TerminateNow
        }
        TerminateDecision::QuitThroughTheEventLoop => {
            log::info!("asked to terminate; quitting in order");
            crate::quit::request_quit();
            NSApplicationTerminateReply::TerminateCancel
        }
    })
    .unwrap_or(NSApplicationTerminateReply::TerminateCancel);
    reply.0
}

/// The class, ID and quit reason of the Apple event AppKit is handling now, if any.
fn current_apple_event() -> Option<(u32, u32, Option<u32>)> {
    // SAFETY: plain Foundation messages with the documented argument and return types
    // (`AEEventClass`, `AEEventID`, `AEKeyword` and `OSType` are 32-bit four-character codes).
    unsafe {
        let manager: Retained<AnyObject> =
            msg_send![class!(NSAppleEventManager), sharedAppleEventManager];
        let event: Option<Retained<AnyObject>> = msg_send![&manager, currentAppleEvent];
        let event = event?;
        let event_class: u32 = msg_send![&event, eventClass];
        let event_id: u32 = msg_send![&event, eventID];
        let mut reason: Option<Retained<AnyObject>> =
            msg_send![&event, paramDescriptorForKeyword: QUIT_REASON];
        if reason.is_none() {
            reason = msg_send![&event, attributeDescriptorForKeyword: QUIT_REASON];
        }
        let reason = reason.map(|reason| {
            let code: u32 = msg_send![&reason, enumCodeValue];
            code
        });
        Some((event_class, event_id, reason))
    }
}

/// Diagnostics (`--exit-by terminate`): sends `terminate:` to the application from the run
/// loop, outside any event handler, as a menu item does.
pub(crate) fn send_terminate() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    // SAFETY: `performSelector:withObject:afterDelay:` takes a selector, an optional object and
    // a delay in seconds; `terminate:` takes one optional object.
    unsafe {
        let _: () = msg_send![
            &app,
            performSelector: sel!(terminate:),
            withObject: std::ptr::null::<AnyObject>(),
            afterDelay: 0.0f64
        ];
    }
}

/// Diagnostics (`--exit-by logout`): from the run loop, outside any event handler, sends this
/// process a quit Apple event whose reason is `kAEReallyLogOut`, the event an application gets
/// when the user logs out. It is addressed to the current process, so the Apple Event Manager
/// dispatches it directly to the handler without serialisation (AEMach.h): it never leaves the
/// process. Never sent to any other process.
pub(crate) fn send_log_out_quit_event() {
    let Some(class) = delegate_class_for_diagnostics() else {
        return;
    };
    let selector = sel!(mercuryDiagnosticLogOut:);
    // SAFETY: the function's signature matches the type encoding: no result, `self`, `_cmd`
    // and one object.
    let imp = unsafe {
        std::mem::transmute::<unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject), Imp>(
            diagnostic_log_out,
        )
    };
    if !add_method(class, selector, imp, DIAGNOSTIC_TYPES) {
        return;
    }
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(delegate) = NSApplication::sharedApplication(mtm).delegate() else {
        return;
    };
    let delegate: &AnyObject = delegate.as_ref();
    // SAFETY: as in `send_terminate`; the method takes one optional object.
    unsafe {
        let _: () = msg_send![
            delegate,
            performSelector: selector,
            withObject: std::ptr::null::<AnyObject>(),
            afterDelay: 0.0f64
        ];
    }
}

fn delegate_class_for_diagnostics() -> Option<*mut AnyClass> {
    let mtm = MainThreadMarker::new()?;
    let delegate = NSApplication::sharedApplication(mtm).delegate()?;
    let delegate: &AnyObject = delegate.as_ref();
    // SAFETY: `delegate` is a live object, so this returns its class.
    Some(unsafe { object_getClass(delegate) }.cast_mut())
}

unsafe extern "C-unwind" fn diagnostic_log_out(
    _this: *mut AnyObject,
    _cmd: Sel,
    _: *mut AnyObject,
) {
    let _ = std::panic::catch_unwind(|| {
        // SAFETY: Foundation messages with their documented types: `AEReturnID` is `i16`
        // (`kAutoGenerateReturnID` = -1), `AETransactionID` is `i32` (`kAnyTransactionID` = 0),
        // `NSAppleEventSendNoReply` = 1, the timeout is in seconds.
        unsafe {
            let target: Retained<AnyObject> =
                msg_send![class!(NSAppleEventDescriptor), currentProcessDescriptor];
            let event: Retained<AnyObject> = msg_send![
                class!(NSAppleEventDescriptor),
                appleEventWithEventClass: CORE_EVENT_CLASS,
                eventID: QUIT_APPLICATION,
                targetDescriptor: &*target,
                returnID: -1i16,
                transactionID: 0i32
            ];
            let reason: Retained<AnyObject> =
                msg_send![class!(NSAppleEventDescriptor), descriptorWithEnumCode: REALLY_LOG_OUT];
            let _: () = msg_send![&event, setParamDescriptor: &*reason, forKeyword: QUIT_REASON];
            let mut error: *mut AnyObject = std::ptr::null_mut();
            let reply: Option<Retained<AnyObject>> = msg_send![
                &event,
                sendEventWithOptions: 1usize,
                timeout: 10.0f64,
                error: &mut error
            ];
            if reply.is_none() {
                log::warn!("diagnostics: sending the log-out quit event failed");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const QUIT: Option<u32> = None;

    #[test]
    fn a_terminate_without_an_apple_event_quits_through_the_event_loop() {
        assert_eq!(decide(None), TerminateDecision::QuitThroughTheEventLoop);
    }

    #[test]
    fn a_quit_event_without_a_reason_quits_through_the_event_loop() {
        assert_eq!(
            decide(Some((CORE_EVENT_CLASS, QUIT_APPLICATION, QUIT))),
            TerminateDecision::QuitThroughTheEventLoop
        );
    }

    #[test]
    fn log_out_restart_and_shut_down_end_the_run_at_once() {
        for reason in [b"logo", b"rlgo", b"rrst", b"rest", b"rsdn", b"shut"] {
            assert_eq!(
                decide(Some((
                    CORE_EVENT_CLASS,
                    QUIT_APPLICATION,
                    Some(four_char_code(reason))
                ))),
                TerminateDecision::EndTheRunNow,
                "{}",
                String::from_utf8_lossy(reason)
            );
        }
    }

    #[test]
    fn other_quit_reasons_and_other_events_quit_through_the_event_loop() {
        assert_eq!(
            decide(Some((
                CORE_EVENT_CLASS,
                QUIT_APPLICATION,
                Some(four_char_code(b"quia"))
            ))),
            TerminateDecision::QuitThroughTheEventLoop
        );
        assert_eq!(
            decide(Some((
                CORE_EVENT_CLASS,
                four_char_code(b"odoc"),
                Some(four_char_code(b"rlgo"))
            ))),
            TerminateDecision::QuitThroughTheEventLoop
        );
    }

    #[test]
    fn four_character_codes_are_big_endian() {
        assert_eq!(CORE_EVENT_CLASS, 0x6165_7674);
        assert_eq!(REALLY_LOG_OUT, 0x726c_676f);
    }
}
