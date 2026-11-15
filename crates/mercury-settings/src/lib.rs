//! Mercury Browser's settings and saved session.
//!
//! A library only: typed models with safe defaults, validation of whatever is on disk, private
//! files replaced atomically, search engines and their keywords, and the snapshot that restores
//! tabs after a restart or a crash. It never reads the clock, never generates random values and
//! starts no threads; methods that need the time take `now_ms` (milliseconds since the Unix
//! epoch) from the caller.

#![deny(missing_docs)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

mod atomic;
mod bounded;
mod error;
mod read;
mod search;
mod session;
mod settings;
mod store;

pub use atomic::write_private_atomic;
pub use error::SettingsError;
pub use search::{
    DEFAULT_ENGINE_ID, MAX_CUSTOM_ENGINES, MAX_ENGINE_ID_BYTES, MAX_ENGINE_NAME_BYTES,
    MAX_KEYWORD_BYTES, MAX_TEMPLATE_BYTES, SEARCH_TERMS_PLACEHOLDER, SearchEngine, builtin_engines,
};
pub use session::{
    ClosedTab, MAX_CLOSED_TABS, MAX_RECOVERY_SESSIONS, MAX_RESTORED_TABS, MAX_TITLE_BYTES,
    MAX_WINDOW_COORDINATE, MAX_WINDOW_SIDE, MIN_WINDOW_SIDE, SaveThrottle, SaveToken,
    SessionSnapshot, TabSnapshot, WindowBounds, WindowSnapshot,
};
pub use settings::{
    AppearanceSettings, ClearOnExit, DownloadSettings, MAX_LANGUAGE_TAG_BYTES,
    MAX_NEVER_SAVE_ORIGINS, MAX_ORIGIN_BYTES, MAX_PATH_BYTES, MAX_QUOTED_BYTES, MAX_URL_BYTES,
    MAX_ZOOM_PERCENT, MIN_ZOOM_PERCENT, PasswordSettings, PrivacySettings, Repair, SearchSettings,
    Settings, StartupBehavior, Theme, WebSettings, canonical_origin, homepage_url,
    is_plausible_language_tag,
};
pub use store::{
    Loaded, MAX_SESSION_FILE_BYTES, MAX_SETTINGS_FILE_BYTES, PreviousSession,
    SESSION_FORMAT_VERSION, SETTINGS_FORMAT_VERSION, SessionStore, SettingsStore, StartupSessions,
    sweep_stale_temporaries,
};
