use mercury_ui::MainWindow;
use talaria_core::{BrowserController, CloseTabOutcome};

use crate::ui_sync::sync_ui;

/// Finishes closing a tab: the browser shows the remaining tabs, or quits when the tab was the
/// last one. Quitting ends the event loop, after which `main` shuts the engine down in order.
pub(crate) fn finish_closing_tab(
    window: &MainWindow,
    controller: &BrowserController,
    outcome: CloseTabOutcome,
) {
    match outcome {
        CloseTabOutcome::Closed => sync_ui(window, controller),
        CloseTabOutcome::LastTab => {
            log::info!("the last tab was closed; quitting");
            if let Err(error) = slint::quit_event_loop() {
                log::warn!("could not quit: {error}");
            }
        }
        CloseTabOutcome::NotFound => {}
    }
}
