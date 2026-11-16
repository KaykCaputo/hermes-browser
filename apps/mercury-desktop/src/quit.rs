use std::time::{SystemTime, UNIX_EPOCH};

use mercury_ui::MainWindow;
use slint::platform::WindowEvent;
use slint::{CloseRequestResponse, ComponentHandle, Timer, TimerMode};

use crate::cli::{ExitBy, ExitRequest};

/// Ends the event loop, as the quit shortcut does; `main` then shuts the engine down in order.
pub(crate) fn request_quit() {
    if let Err(error) = slint::quit_event_loop() {
        log::warn!("could not quit: {error}");
    }
}

/// Closing the window (its close button, or the close request `--exit-by close-request` sends)
/// quits like the quit shortcut. The window stays shown until the event loop has ended and the
/// engine's state has been taken: a window Slint hides by itself is suspended while the loop
/// exits (`suspend_all_hidden_windows`, i-slint-backend-winit-1.17.1/event_loop.rs:120-132,
/// 547-554), which tears rendering down before the engine has shut down.
pub(crate) fn quit_when_the_window_is_closed(window: &MainWindow) {
    window.window().on_close_requested(|| {
        let _slint = crate::callback_scope::InSlintCallback::enter();
        request_quit();
        CloseRequestResponse::KeepWindowShown
    });
}

/// Starts the diagnostic `--exit-after` timer. The returned timer must be kept alive while the
/// event loop runs.
pub(crate) fn start_exit_request(window: &MainWindow, request: ExitRequest) -> Timer {
    let weak_window = window.as_weak();
    let timer = Timer::default();
    timer.start(TimerMode::SingleShot, request.after, move || {
        let _slint = crate::callback_scope::InSlintCallback::enter();
        log::info!(
            "diagnostics: requesting a quit by {} at unix time {} ms",
            match request.by {
                ExitBy::CloseRequest => "close request",
                ExitBy::Quit => "quit",
                ExitBy::Terminate => "terminate request",
                ExitBy::LogOut => "log-out quit event",
            },
            unix_time_ms()
        );
        match request.by {
            ExitBy::Quit => request_quit(),
            #[cfg(target_os = "macos")]
            ExitBy::Terminate => crate::macos_terminate::send_terminate(),
            #[cfg(all(target_os = "macos", feature = "quit-diagnostics"))]
            ExitBy::LogOut => crate::macos_terminate::log_out_diagnostic::send_log_out_quit_event(),
            #[cfg(not(target_os = "macos"))]
            ExitBy::Terminate => log::warn!("diagnostics: this exit request needs macOS"),
            #[cfg(not(all(target_os = "macos", feature = "quit-diagnostics")))]
            ExitBy::LogOut => {
                log::warn!("diagnostics: this exit request needs a quit-diagnostics macOS build")
            }
            ExitBy::CloseRequest => {
                let Some(window) = weak_window.upgrade() else {
                    return;
                };
                // The winit backend makes this same call for the title bar's close button
                // (i-slint-backend-winit-1.17.1/event_loop.rs:240-244).
                if let Err(error) = window
                    .window()
                    .try_dispatch_event(WindowEvent::CloseRequested)
                {
                    log::warn!("diagnostics: the close request failed: {error}");
                }
            }
        }
    });
    timer
}

fn unix_time_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis())
}
