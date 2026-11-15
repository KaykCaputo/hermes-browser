//! Mercury Browser's settings and saved session.
//!
//! A library only: typed models with safe defaults, validation of whatever is on disk, private
//! files replaced atomically, search engines and their keywords, and the snapshot that restores
//! tabs after a restart or a crash. It never reads the clock, never generates random values and
//! starts no threads; methods that need the time take `now_ms` (milliseconds since the Unix
//! epoch) from the caller.
//!
//! # The contract for the application
//!
//! The guarantees of this crate hold only if the application wiring it in does the following.
//! Check the wiring code against this list.
//!
//! 1. **Take the profile's single-instance lock first, and keep it** for the life of the
//!    process, before creating a [`SettingsStore`] or a [`SessionStore`].
//! 2. **Sweep right after taking the lock**: call [`sweep_stale_temporaries`]. Never call it
//!    without holding the lock.
//! 3. **Call [`SessionStore::begin_run`] exactly once per start**, before any session save.
//! 4. **Call [`SettingsStore::load`] before any settings save.**
//! 5. **Offer [`StartupSessions::recovery`] whenever it is not empty**, whatever the start-up
//!    setting. Call [`SessionStore::resolve_recovery`] only after the user has restored or
//!    dismissed what it offers.
//! 6. **Restore a `last` that exited cleanly only when the start-up setting says so**
//!    ([`StartupBehavior::RestoreSession`]); otherwise keep it for "restore previous session".
//!    A `last` that did not exit cleanly is already in the recovery list.
//! 7. **Session saves may continue while recoveries are pending**: a save never touches the
//!    recovery list.
//! 8. **Save through the throttle**: call [`SaveThrottle::mark_dirty`] on every change; when
//!    [`SaveThrottle::should_save`] gives a token, write with `exited_cleanly: false` (off the
//!    UI thread if you like), and hand the token to [`SaveThrottle::saved`] only if the write
//!    succeeded.
//! 9. **On quit**, wait for any save in flight, then write once more with
//!    `exited_cleanly: true` — even when nothing is dirty, because `begin_run` marked the file
//!    as running — and then call [`SaveThrottle::flushed`]. A worker's save landing after that
//!    final save would mark the run as crashed.
//! 10. **New-tab and internal pages are `about:` URLs** (for example `about:newtab`), or they
//!     are dropped on restore.
//! 11. **On a [`Loaded::Ok`] with repairs, save that file once. On [`Loaded::Newer`], or a
//!     [`SettingsError::NewerVersion`] from a save, never save that file again in this run**,
//!     and tell the user once.
//! 12. **Validate user input** with [`homepage_url`], [`is_plausible_language_tag`],
//!     [`canonical_origin`] and [`SearchEngine::validate`] before saving it.
//! 13. **Check the download directory again at each use**: [`DownloadSettings::directory`] is
//!     checked only for its form.

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
