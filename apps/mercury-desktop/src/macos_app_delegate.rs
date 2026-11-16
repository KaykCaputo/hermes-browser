//! Methods Mercury adds to the application delegate winit installs, at run time.
//!
//! winit 0.30 owns `NSApplication`'s delegate (`WinitApplicationDelegate`,
//! winit-0.30.13/src/platform_impl/macos/app_state.rs:47-73) and offers no way to extend it.
//! A delegate method winit does not implement can be added to that class with
//! `add_delegate_method`, for example:
//!
//! ```ignore
//! add_delegate_method(sel!(application:openURLs:), imp, c"v@:@@")?;
//! ```
//!
//! The rules that keep this contained: a method is added only if the delegate does not already
//! respond to its selector (so nothing of winit's is replaced); the function must match the
//! type encoding exactly and must not read the delegate's instance state (it receives winit's
//! object as `self` and must ignore it); it must not unwind into AppKit. After adding, the
//! delegate is assigned again, because AppKit looks up which optional methods a delegate
//! implements when it is assigned.

use std::ffi::CStr;

use objc2::MainThreadMarker;
use objc2::ffi::{class_addMethod, object_getClass};
use objc2::runtime::{AnyObject, Imp, NSObjectProtocol, Sel};
use objc2_app_kit::NSApplication;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum AddMethodError {
    NotMainThread,
    NoDelegate,
    /// The delegate already responds to the selector; nothing was changed.
    AlreadyImplemented,
    AddFailed,
}

/// Adds `imp` as `selector` (with the Objective-C type encoding `types`) to the class of
/// `NSApplication`'s delegate. Call on the main thread once the event loop (and with it winit's
/// delegate) exists.
pub(crate) fn add_delegate_method(
    selector: Sel,
    imp: Imp,
    types: &CStr,
) -> Result<(), AddMethodError> {
    let mtm = MainThreadMarker::new().ok_or(AddMethodError::NotMainThread)?;
    let app = NSApplication::sharedApplication(mtm);
    let delegate = app.delegate().ok_or(AddMethodError::NoDelegate)?;
    if delegate.respondsToSelector(selector) {
        return Err(AddMethodError::AlreadyImplemented);
    }
    let object: &AnyObject = delegate.as_ref();
    // SAFETY: `object` is a live object, so `object_getClass` returns its registered class;
    // the caller guarantees that `imp` matches `types` and reads no instance state.
    let added = unsafe {
        class_addMethod(
            object_getClass(object).cast_mut(),
            selector,
            imp,
            types.as_ptr(),
        )
    }
    .as_bool();
    if !added {
        return Err(AddMethodError::AddFailed);
    }
    // AppKit caches which optional delegate methods exist when the delegate is assigned.
    // The application holds its delegate weakly; `delegate` keeps it alive meanwhile.
    app.setDelegate(None);
    app.setDelegate(Some(&delegate));
    Ok(())
}
