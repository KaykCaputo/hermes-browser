//! Menu items whose actions other lanes provide. Each item stays disabled until its lane sets
//! the flag below to `true` and fills in the function, which the menus call through
//! `MenuCommand`. Nothing else in the menus needs to change.

use mercury_ui::MainWindow;

/// Settings lane: "Settings…" (Cmd+,), in the application menu and the browser menu.
pub(crate) const SETTINGS_AVAILABLE: bool = false;
/// Session lane: File ▸ "Reopen Closed Tab" (Shift+Cmd+T). When the lane adds a
/// `ShortcutAction` for it, change the item's command to `MenuCommand::Shortcut(…)` in
/// `model.rs`; the table test then checks that `shortcuts.rs` owns the key.
pub(crate) const REOPEN_CLOSED_TAB_AVAILABLE: bool = false;
/// Session lane: History ▸ "Restore Previous Session".
pub(crate) const RESTORE_PREVIOUS_SESSION_AVAILABLE: bool = false;
/// History lane: History ▸ "Show All History" (Cmd+Y).
pub(crate) const SHOW_HISTORY_AVAILABLE: bool = false;
/// History lane: History ▸ "Clear Browsing Data…" (Shift+Cmd+Delete).
pub(crate) const CLEAR_BROWSING_DATA_AVAILABLE: bool = false;

/// Settings lane: `window.invoke_show_settings();`
pub(crate) fn show_settings(_window: &MainWindow) {}

/// Session lane: reopen the most recently closed tab.
pub(crate) fn reopen_closed_tab(_window: &MainWindow) {}

/// Session lane: `restore-previous-session`.
pub(crate) fn restore_previous_session(_window: &MainWindow) {}

/// History lane: show the history window.
pub(crate) fn show_history(_window: &MainWindow) {}

/// History lane: show the "Clear browsing data" dialog.
pub(crate) fn clear_browsing_data(_window: &MainWindow) {}
