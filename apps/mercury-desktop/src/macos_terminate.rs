//! The system's terminate request (`-[NSApplication terminate:]`: a Quit menu item, the Dock's
//! Quit, logging out, restarting, shutting down) ends the run in order instead of ending the
//! process at once.
//!
//! winit 0.30's application delegate implements only `applicationDidFinishLaunching:` and
//! `applicationWillTerminate:` (winit-0.30.13/src/platform_impl/macos/app_state.rs:47-73), so
//! `terminate:` goes straight to `applicationWillTerminate:` and AppKit's `exit()`: the engine
//! is never shut down. This adds `applicationShouldTerminate:` to winit's delegate class
//! (`macos_app_delegate.rs`). The answer depends on why the request came:
//!
//! - The session is ending (a quit Apple event whose `keyAEQuitReason` is a log-out, restart
//!   or shut-down) and the callback is not nested in Slint (see `EntryState`): the post-run
//!   sequence runs inside the callback and the answer is `NSTerminateNow`, so the log-out goes
//!   on as soon as the run has ended. "Later" is no option: AppKit then runs its run loop in
//!   `NSModalPanelRunLoopMode` until `replyToApplicationShouldTerminate:`, so the event loop
//!   cannot end (tried: it hangs); and "cancel" would cancel the log-out.
//! - Anything else (a menu item's `terminate:` has no current Apple event; the Dock's Quit is a
//!   quit event without a reason), or a session end that arrives where the sequence must not
//!   run: the answer is `NSTerminateCancel`, and the event loop is asked to quit, so that `main`
//!   runs the same post-run sequence.

use std::ffi::CStr;
use std::time::Instant;

use objc2::ffi::NSUInteger;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Imp, Sel};
use objc2::{class, msg_send, sel};
use objc2_app_kit::NSApplicationTerminateReply;
use objc2_foundation::NSString;

use crate::macos_app_delegate::add_delegate_method;
use crate::post_run::PostRun;

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

/// `NSApplicationTerminateReply applicationShouldTerminate:(NSApplication *)sender`.
const SHOULD_TERMINATE_TYPES: &CStr = c"Q@:@";

const fn four_char_code(code: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*code)
}

/// Where the terminate callback was entered from: the post-run sequence may run inside it only
/// if nothing of Slint's or the engine's is in use further up the stack. A Slint callback that
/// opens a nested run loop (a modal panel, menu tracking) would otherwise have the sequence run
/// inside that callback, with Slint's state borrowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EntryState {
    pub(crate) post_run: PostRun,
    /// A Slint callback of Mercury's is running (`callback_scope`).
    pub(crate) inside_slint_callback: bool,
    /// The engine's state is borrowed (a pump, a render).
    pub(crate) engine_busy: bool,
    /// The run loop is not in its default mode: a modal panel's or menu tracking's loop.
    pub(crate) nested_run_loop: bool,
}

#[cfg(test)]
impl EntryState {
    pub(crate) const CLEAR: Self = Self {
        post_run: PostRun::NotStarted,
        inside_slint_callback: false,
        engine_busy: false,
        nested_run_loop: false,
    };
}

/// Why a terminate request is answered through the event loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Why {
    NotASessionEnd,
    AlreadyEnding,
    InsideSlintCallback,
    EngineBusy,
    NestedRunLoop,
}

/// How a terminate request is answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminateDecision {
    /// Run the post-run sequence now, then answer `NSTerminateNow`.
    EndTheRunNow,
    /// Answer `NSTerminateCancel` and let the event loop quit; `main` ends the run.
    QuitThroughTheEventLoop(Why),
}

/// `current_event`: the class, ID and quit reason of the Apple event being handled, if any.
pub(crate) fn decide(
    current_event: Option<(u32, u32, Option<u32>)>,
    entry: EntryState,
) -> TerminateDecision {
    let session_ending = matches!(
        current_event,
        Some((CORE_EVENT_CLASS, QUIT_APPLICATION, Some(reason)))
            if SESSION_ENDING_REASONS.contains(&reason)
    );
    let why = if entry.post_run != PostRun::NotStarted {
        Why::AlreadyEnding
    } else if !session_ending {
        Why::NotASessionEnd
    } else if entry.inside_slint_callback {
        Why::InsideSlintCallback
    } else if entry.engine_busy {
        Why::EngineBusy
    } else if entry.nested_run_loop {
        Why::NestedRunLoop
    } else {
        return TerminateDecision::EndTheRunNow;
    };
    TerminateDecision::QuitThroughTheEventLoop(why)
}

/// Call on the main thread once the backend (and with it winit's event loop and application
/// delegate) exists.
pub(crate) fn end_the_run_in_order_on_terminate_requests() {
    // SAFETY: the function's signature matches the type encoding: it returns an `NSUInteger`
    // and takes `self`, `_cmd` and one object; it ignores `self`.
    let imp = unsafe {
        std::mem::transmute::<
            unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject) -> NSUInteger,
            Imp,
        >(application_should_terminate)
    };
    match add_delegate_method(
        sel!(applicationShouldTerminate:),
        imp,
        SHOULD_TERMINATE_TYPES,
    ) {
        Ok(()) => log::debug!("terminate requests end the run in order"),
        Err(error) => log::warn!("terminate requests are not handled: {error:?}"),
    }
}

unsafe extern "C-unwind" fn application_should_terminate(
    _this: *mut AnyObject,
    _cmd: Sel,
    _sender: *mut AnyObject,
) -> NSUInteger {
    // Nothing may unwind into AppKit.
    let decision = std::panic::catch_unwind(|| decide(current_apple_event(), entry_state()))
        .unwrap_or(TerminateDecision::QuitThroughTheEventLoop(
            Why::NotASessionEnd,
        ));
    let reply = match decision {
        TerminateDecision::EndTheRunNow => end_the_run_for_the_system(),
        TerminateDecision::QuitThroughTheEventLoop(why) => {
            let _ = std::panic::catch_unwind(|| {
                log::info!("asked to terminate ({why:?}); quitting in order");
                crate::quit::request_quit();
            });
            NSApplicationTerminateReply::TerminateCancel
        }
    };
    reply.0
}

/// The session is ending: ends the run here and answers "now", whatever happens, so that the
/// log-out is never cancelled by Mercury. A run that should end with another status (a failed
/// screenshot run, a quit signal received meanwhile) ends the process itself with that status;
/// AppKit would exit with 0.
fn end_the_run_for_the_system() -> NSApplicationTerminateReply {
    let started = Instant::now();
    let outcome = std::panic::catch_unwind(|| {
        log::info!("the session is ending; ending the run before answering");
        crate::post_run::finish(false)
    });
    let panicked = outcome.is_err();
    let _ = std::panic::catch_unwind(|| match panicked {
        false => log::info!(
            "ended the run for the system in {} ms",
            started.elapsed().as_millis()
        ),
        true => log::error!("ending the run for the system panicked; letting the system go on"),
    });
    log::logger().flush();
    if let Some(signal) = crate::signals::received() {
        crate::signals::end_by(signal);
    }
    if panicked {
        // The exit status of a Rust panic.
        std::process::exit(101);
    }
    if let Ok(Some(code)) = outcome
        && code != 0
    {
        std::process::exit(i32::from(code));
    }
    NSApplicationTerminateReply::TerminateNow
}

fn entry_state() -> EntryState {
    EntryState {
        post_run: crate::post_run::state(),
        inside_slint_callback: crate::callback_scope::inside_slint_callback(),
        engine_busy: crate::engine::engine_busy(),
        nested_run_loop: !run_loop_in_default_mode(),
    }
}

fn run_loop_in_default_mode() -> bool {
    // SAFETY: `currentRunLoop`, `currentMode` and `isEqualToString:` are plain Foundation
    // messages; `NSDefaultRunLoopMode` is the string `kCFRunLoopDefaultMode`.
    unsafe {
        let run_loop: Retained<AnyObject> = msg_send![class!(NSRunLoop), currentRunLoop];
        let mode: Option<Retained<NSString>> = msg_send![&run_loop, currentMode];
        let default = NSString::from_str("kCFRunLoopDefaultMode");
        mode.is_some_and(|mode| mode.isEqualToString(&default))
    }
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
/// loop, outside any event handler, as a menu item does. Harmless: it is what the user's Quit
/// does.
pub(crate) fn send_terminate() {
    // SAFETY: `performSelector:withObject:afterDelay:` takes a selector, an optional object and
    // a delay in seconds; `terminate:` takes one optional object.
    unsafe {
        let app: Retained<AnyObject> = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![
            &app,
            performSelector: sel!(terminate:),
            withObject: std::ptr::null::<AnyObject>(),
            afterDelay: 0.0f64
        ];
    }
}

/// Diagnostics (`--exit-by logout`), only in builds with the `quit-diagnostics` feature, so no
/// released binary can send itself a log-out event.
#[cfg(feature = "quit-diagnostics")]
pub(crate) mod log_out_diagnostic {
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, Imp, Sel};
    use objc2::{class, msg_send, sel};

    use super::{CORE_EVENT_CLASS, QUIT_APPLICATION, QUIT_REASON, four_char_code};
    use crate::macos_app_delegate::{AddMethodError, add_delegate_method};

    /// `kAEReallyLogOut`.
    const REALLY_LOG_OUT: u32 = four_char_code(b"rlgo");

    /// From the run loop, outside any event handler, sends this process a quit Apple event
    /// whose reason is `kAEReallyLogOut`, the event an application gets when the user logs
    /// out. It is addressed to the current process, so the Apple Event Manager dispatches it
    /// directly to the handler without serialisation (AEMach.h): it never leaves the process.
    /// Never sent to any other process.
    pub(crate) fn send_log_out_quit_event() {
        let selector = sel!(mercuryDiagnosticLogOut:);
        // SAFETY: the function's signature matches the encoding: no result, `self`, `_cmd` and
        // one object; it ignores `self`.
        let imp = unsafe {
            std::mem::transmute::<
                unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject),
                Imp,
            >(diagnostic_log_out)
        };
        match add_delegate_method(selector, imp, c"v@:@") {
            Ok(()) | Err(AddMethodError::AlreadyImplemented) => {}
            Err(error) => {
                log::warn!("diagnostics: cannot send the log-out quit event: {error:?}");
                return;
            }
        }
        // SAFETY: `performSelector:withObject:afterDelay:` on the application's delegate, which
        // now implements the one-object method.
        unsafe {
            let app: Retained<AnyObject> = msg_send![class!(NSApplication), sharedApplication];
            let delegate: Option<Retained<AnyObject>> = msg_send![&app, delegate];
            let Some(delegate) = delegate else {
                return;
            };
            let _: () = msg_send![
                &delegate,
                performSelector: selector,
                withObject: std::ptr::null::<AnyObject>(),
                afterDelay: 0.0f64
            ];
        }
    }

    unsafe extern "C-unwind" fn diagnostic_log_out(
        _this: *mut AnyObject,
        _cmd: Sel,
        _: *mut AnyObject,
    ) {
        let _ = std::panic::catch_unwind(|| {
            // SAFETY: Foundation messages with their documented types: `AEReturnID` is `i16`
            // (`kAutoGenerateReturnID` = -1), `AETransactionID` is `i32` (`kAnyTransactionID`
            // = 0), `NSAppleEventSendNoReply` = 1, the timeout is in seconds.
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
                let reason: Retained<AnyObject> = msg_send![
                    class!(NSAppleEventDescriptor),
                    descriptorWithEnumCode: REALLY_LOG_OUT
                ];
                let _: () =
                    msg_send![&event, setParamDescriptor: &*reason, forKeyword: QUIT_REASON];
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quit_event(reason: Option<&[u8; 4]>) -> Option<(u32, u32, Option<u32>)> {
        Some((
            CORE_EVENT_CLASS,
            QUIT_APPLICATION,
            reason.map(four_char_code),
        ))
    }

    #[test]
    fn a_terminate_without_an_apple_event_quits_through_the_event_loop() {
        assert_eq!(
            decide(None, EntryState::CLEAR),
            TerminateDecision::QuitThroughTheEventLoop(Why::NotASessionEnd)
        );
    }

    #[test]
    fn a_quit_event_without_a_reason_quits_through_the_event_loop() {
        assert_eq!(
            decide(quit_event(None), EntryState::CLEAR),
            TerminateDecision::QuitThroughTheEventLoop(Why::NotASessionEnd)
        );
    }

    #[test]
    fn log_out_restart_and_shut_down_end_the_run_at_once() {
        for reason in [b"logo", b"rlgo", b"rrst", b"rest", b"rsdn", b"shut"] {
            assert_eq!(
                decide(quit_event(Some(reason)), EntryState::CLEAR),
                TerminateDecision::EndTheRunNow,
                "{}",
                String::from_utf8_lossy(reason)
            );
        }
    }

    #[test]
    fn other_quit_reasons_and_other_events_quit_through_the_event_loop() {
        assert_eq!(
            decide(quit_event(Some(b"quia")), EntryState::CLEAR),
            TerminateDecision::QuitThroughTheEventLoop(Why::NotASessionEnd)
        );
        assert_eq!(
            decide(
                Some((
                    CORE_EVENT_CLASS,
                    four_char_code(b"odoc"),
                    Some(four_char_code(b"rlgo"))
                )),
                EntryState::CLEAR
            ),
            TerminateDecision::QuitThroughTheEventLoop(Why::NotASessionEnd)
        );
    }

    #[test]
    fn a_session_end_inside_slint_or_the_engine_or_a_nested_loop_goes_through_the_event_loop() {
        let log_out = quit_event(Some(b"rlgo"));
        let cases = [
            (
                EntryState {
                    inside_slint_callback: true,
                    ..EntryState::CLEAR
                },
                Why::InsideSlintCallback,
            ),
            (
                EntryState {
                    engine_busy: true,
                    ..EntryState::CLEAR
                },
                Why::EngineBusy,
            ),
            (
                EntryState {
                    nested_run_loop: true,
                    ..EntryState::CLEAR
                },
                Why::NestedRunLoop,
            ),
        ];
        for (entry, why) in cases {
            assert_eq!(
                decide(log_out, entry),
                TerminateDecision::QuitThroughTheEventLoop(why),
                "{entry:?}"
            );
        }
    }

    #[test]
    fn a_terminate_while_the_run_is_ending_or_has_ended_does_not_end_it_again() {
        for post_run in [PostRun::Running, PostRun::Done] {
            let entry = EntryState {
                post_run,
                ..EntryState::CLEAR
            };
            assert_eq!(
                decide(quit_event(Some(b"rlgo")), entry),
                TerminateDecision::QuitThroughTheEventLoop(Why::AlreadyEnding)
            );
            assert_eq!(
                decide(None, entry),
                TerminateDecision::QuitThroughTheEventLoop(Why::AlreadyEnding)
            );
        }
    }

    #[test]
    fn four_character_codes_are_big_endian() {
        assert_eq!(CORE_EVENT_CLASS, 0x6165_7674);
        assert_eq!(four_char_code(b"rlgo"), 0x726c_676f);
    }
}
