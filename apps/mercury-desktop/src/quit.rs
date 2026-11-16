use std::time::{SystemTime, UNIX_EPOCH};

use mercury_ui::MainWindow;
use slint::platform::WindowEvent;
use slint::{ComponentHandle, Timer, TimerMode};

use crate::cli::{ExitBy, ExitRequest};

/// Ends the event loop, as the quit shortcut does; `main` then shuts the engine down in order.
pub(crate) fn request_quit() {
    if let Err(error) = slint::quit_event_loop() {
        log::warn!("could not quit: {error}");
    }
}

/// Starts the diagnostic `--exit-after` timer. The returned timer must be kept alive while the
/// event loop runs.
pub(crate) fn start_exit_request(window: &MainWindow, request: ExitRequest) -> Timer {
    let weak_window = window.as_weak();
    let timer = Timer::default();
    timer.start(TimerMode::SingleShot, request.after, move || {
        log::info!(
            "diagnostics: requesting a quit by {} at unix time {} ms",
            match request.by {
                ExitBy::CloseRequest => "close request",
                ExitBy::Quit => "quit",
            },
            unix_time_ms()
        );
        match request.by {
            ExitBy::Quit => request_quit(),
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
