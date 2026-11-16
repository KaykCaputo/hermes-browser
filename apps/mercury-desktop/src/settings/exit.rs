//! What happens to the settings and the user's data when the browser quits.

use mercury_settings::Settings;

/// The history lane's hook: clears the browsing history when the user asked for that at quit
/// (`privacy.clear_on_exit.history`). Called once by `SettingsHandle::finish_run`, after the
/// event loop has ended and before the engine shuts down. Cookies, site data and the cache are
/// cleared by the engine itself as it shuts down (`talaria_servo::ExitClearing`).
pub(crate) fn clear_history_on_exit(_settings: &Settings) {
    log::info!("clearing history on exit is not available yet");
}
