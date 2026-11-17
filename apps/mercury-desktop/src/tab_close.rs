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
        CloseTabOutcome::LastTab => quit_after_last_tab(),
        CloseTabOutcome::NotFound => {}
    }
}

/// Quits because the last tab was closed. Ending the event loop lets `main` shut the engine down.
pub(crate) fn quit_after_last_tab() {
    log::info!("the last tab was closed; quitting");
    crate::quit::request_quit();
}
