//! The settings model, its defaults and its validation.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::bounded::{self, Unread};
use crate::search::{DEFAULT_ENGINE_ID, SearchEngine};

/// The smallest default page zoom, in percent.
pub const MIN_ZOOM_PERCENT: u16 = 30;
/// The largest default page zoom, in percent.
pub const MAX_ZOOM_PERCENT: u16 = 500;
/// The longest URL kept anywhere in settings or a session, in bytes.
pub const MAX_URL_BYTES: usize = 8 * 1024;
/// The longest BCP 47 language tag accepted, in bytes (RFC 5646 §4.4.1's recommended minimum
/// buffer).
pub const MAX_LANGUAGE_TAG_BYTES: usize = 35;
/// The longest download directory accepted, in bytes.
pub const MAX_PATH_BYTES: usize = 4096;
/// How many origins the "never save passwords" list keeps.
pub const MAX_NEVER_SAVE_ORIGINS: usize = 1000;
/// The longest entry in the "never save passwords" list, in bytes.
pub const MAX_ORIGIN_BYTES: usize = 300;

/// Everything the user can set. Every field has a default, so a file that lacks some of them,
/// or is empty, still loads.
///
/// Its `Deserialize` is bounded: the custom engines and the "never save" list keep at most their
/// limits and skip the rest unread, for any caller. Decoding never validates: call
/// [`validated`](Self::validated) (as [`crate::SettingsStore::load`] does).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// What a new window shows at start-up.
    pub startup: StartupBehavior,
    /// The home page; empty means none.
    pub homepage: String,
    /// Search engines and address-bar search.
    pub search: SearchSettings,
    /// Where downloads go.
    pub downloads: DownloadSettings,
    /// How the browser looks.
    pub appearance: AppearanceSettings,
    /// Privacy and security.
    pub privacy: PrivacySettings,
    /// The password manager.
    pub passwords: PasswordSettings,
    /// Web-platform features.
    pub web: WebSettings,
    /// The user-interface language as a BCP 47 tag; `None` follows the system.
    pub language: Option<String>,
}

/// What the browser shows when it starts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartupBehavior {
    /// A new tab page.
    #[default]
    NewTab,
    /// The home page.
    Homepage,
    /// The tabs of the previous session.
    RestoreSession,
}

/// Search settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchSettings {
    /// The id of the engine used for address-bar searches.
    pub default_engine: String,
    /// Engines the user added, after the built-in ones. Its `Deserialize` keeps at most
    /// [`MAX_CUSTOM_ENGINES`](crate::MAX_CUSTOM_ENGINES) and skips the rest unread, for any
    /// caller.
    #[serde(deserialize_with = "bounded::custom_engines")]
    pub custom_engines: Vec<SearchEngine>,
    /// Whether to fetch search suggestions while typing. Off by default because it sends every
    /// keystroke in the address bar to the search engine.
    pub suggestions: bool,
}

impl Default for SearchSettings {
    fn default() -> Self {
        Self {
            default_engine: DEFAULT_ENGINE_ID.to_owned(),
            custom_engines: Vec::new(),
            suggestions: false,
        }
    }
}

/// Download settings.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DownloadSettings {
    /// An absolute directory; `None` is the platform's Downloads folder.
    ///
    /// The check is syntactic only: the directory may not exist, may not be writable, or may
    /// have been replaced since. The application must check it again each time it saves a
    /// download there, and fall back to asking.
    pub directory: Option<PathBuf>,
    /// Whether to ask where to save each download.
    pub ask_where_to_save: bool,
}

/// The colour scheme.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    /// Follow the system.
    #[default]
    System,
    /// Always light.
    Light,
    /// Always dark.
    Dark,
}

/// Appearance settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppearanceSettings {
    /// The colour scheme.
    pub theme: Theme,
    /// The zoom of a page with no zoom of its own, in percent
    /// ([`MIN_ZOOM_PERCENT`]..=[`MAX_ZOOM_PERCENT`]).
    pub default_zoom_percent: u16,
    /// Whether the bookmarks bar is shown.
    pub show_bookmarks_bar: bool,
    /// Whether the toolbar has a home button.
    pub show_home_button: bool,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            theme: Theme::System,
            default_zoom_percent: 100,
            show_bookmarks_bar: true,
            show_home_button: false,
        }
    }
}

/// Privacy and security settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PrivacySettings {
    /// Block ads and trackers.
    pub block_ads_and_trackers: bool,
    /// Load pages over HTTPS only.
    pub https_only: bool,
    /// Block pop-up windows.
    pub block_popups: bool,
    /// Send the Global Privacy Control signal.
    pub global_privacy_control: bool,
    /// What is cleared when the browser quits.
    pub clear_on_exit: ClearOnExit,
}

impl Default for PrivacySettings {
    fn default() -> Self {
        Self {
            block_ads_and_trackers: true,
            https_only: false,
            block_popups: true,
            global_privacy_control: true,
            clear_on_exit: ClearOnExit::default(),
        }
    }
}

/// Data cleared when the browser quits. Nothing by default.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClearOnExit {
    /// Browsing history.
    pub history: bool,
    /// Cookies and site data.
    pub cookies: bool,
    /// The cache.
    pub cache: bool,
}

/// Password-manager settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PasswordSettings {
    /// Offer to save passwords.
    pub offer_to_save: bool,
    /// Origins (`scheme://host[:port]`) for which saving was declined for good. Its
    /// `Deserialize` keeps at most [`MAX_NEVER_SAVE_ORIGINS`] and skips the rest unread, for
    /// any caller.
    #[serde(deserialize_with = "bounded::never_save_origins")]
    pub never_save_origins: Vec<String>,
}

impl Default for PasswordSettings {
    fn default() -> Self {
        Self {
            offer_to_save: true,
            never_save_origins: Vec::new(),
        }
    }
}

/// Web-platform settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WebSettings {
    /// Turn on web features the engine marks as experimental.
    pub experimental_web_features: bool,
    /// Draw `<canvas>` on the GPU.
    pub gpu_canvas: bool,
}

impl Default for WebSettings {
    fn default() -> Self {
        Self {
            experimental_web_features: true,
            gpu_canvas: true,
        }
    }
}

/// One value that [`Settings::validated`] changed, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repair {
    /// The dotted path of the field, for example `appearance.default_zoom_percent`.
    pub field: &'static str,
    /// What was wrong and what was done about it.
    pub reason: String,
}

/// The longest piece of text from a file that a repair message quotes, in bytes.
pub const MAX_QUOTED_BYTES: usize = 40;

/// `text` quoted for a message, cut to [`MAX_QUOTED_BYTES`].
pub(crate) fn quoted(text: &str) -> String {
    if text.len() <= MAX_QUOTED_BYTES {
        format!("{text:?}")
    } else {
        format!("{:?}…", truncated(text, MAX_QUOTED_BYTES))
    }
}

impl Settings {
    /// Repairs out-of-range or inconsistent values and reports each repair.
    ///
    /// In field order: a home page that [`homepage_url`] refuses is cleared, and one it accepts
    /// is stored in the form it returns; unusable custom search engines are dropped (see
    /// [`SearchSettings::engines`]) and a default engine that does not exist becomes
    /// [`DEFAULT_ENGINE_ID`]; a download directory that is not absolute, not UTF-8 or longer
    /// than [`MAX_PATH_BYTES`] becomes `None`; a zoom outside
    /// [`MIN_ZOOM_PERCENT`]..=[`MAX_ZOOM_PERCENT`] becomes 100; "never save" entries are put
    /// through [`canonical_origin`], those it refuses and duplicates are dropped, and the list
    /// is cut at [`MAX_NEVER_SAVE_ORIGINS`]; a language that [`is_plausible_language_tag`]
    /// refuses becomes `None`. Each list gives at most one repair, with counts. Validating the
    /// result again repairs nothing.
    pub fn validated(self) -> (Self, Vec<Repair>) {
        self.validated_with(&Unread::default())
    }

    /// [`validated`](Self::validated), also counting the list items a bounded decode skipped.
    pub(crate) fn validated_with(mut self, unread: &Unread) -> (Self, Vec<Repair>) {
        let mut repairs = Vec::new();

        match homepage_url(&self.homepage) {
            None => {
                repairs.push(Repair {
                    field: "homepage",
                    reason: format!(
                        "not an http, https, file or about URL without credentials, of at most \
                         {MAX_URL_BYTES} bytes; cleared"
                    ),
                });
                self.homepage.clear();
            }
            Some(canonical) if canonical != self.homepage => {
                repairs.push(Repair {
                    field: "homepage",
                    reason: "written in its canonical form".to_owned(),
                });
                self.homepage = canonical;
            }
            Some(_) => {}
        }

        let (usable, dropped) = crate::search::usable_custom_engines(
            &self.search.custom_engines,
            unread.get(bounded::CUSTOM_ENGINES),
        );
        repairs.extend(dropped);
        self.search.custom_engines = usable;
        if !self
            .search
            .engines()
            .iter()
            .any(|engine| engine.id == self.search.default_engine)
        {
            repairs.push(Repair {
                field: "search.default_engine",
                reason: format!(
                    "no engine has the id {}; using {DEFAULT_ENGINE_ID:?}",
                    quoted(&self.search.default_engine)
                ),
            });
            self.search.default_engine = DEFAULT_ENGINE_ID.to_owned();
        }

        if let Some(directory) = &self.downloads.directory {
            let problem = match directory.to_str() {
                None => Some("is not valid UTF-8"),
                Some(text) if text.len() > MAX_PATH_BYTES => Some("is too long"),
                Some(_) if !directory.is_absolute() => Some("is not absolute"),
                Some(_) => None,
            };
            if let Some(problem) = problem {
                repairs.push(Repair {
                    field: "downloads.directory",
                    reason: format!("the directory {problem}; using the Downloads folder"),
                });
                self.downloads.directory = None;
            }
        }

        let zoom = self.appearance.default_zoom_percent;
        if !(MIN_ZOOM_PERCENT..=MAX_ZOOM_PERCENT).contains(&zoom) {
            repairs.push(Repair {
                field: "appearance.default_zoom_percent",
                reason: format!(
                    "{zoom} is outside {MIN_ZOOM_PERCENT}-{MAX_ZOOM_PERCENT}; using 100"
                ),
            });
            self.appearance.default_zoom_percent = 100;
        }

        let (origins, origin_repair) = canonical_origins(
            std::mem::take(&mut self.passwords.never_save_origins),
            unread.get(bounded::NEVER_SAVE_ORIGINS),
        );
        self.passwords.never_save_origins = origins;
        repairs.extend(origin_repair);

        if let Some(language) = &self.language
            && !is_plausible_language_tag(language)
        {
            repairs.push(Repair {
                field: "language",
                reason: format!(
                    "{} is not a BCP 47 language tag; following the system",
                    quoted(language)
                ),
            });
            self.language = None;
        }

        (self, repairs)
    }
}

/// The form in which `input` is stored as the home page, or `None` if it is not acceptable.
///
/// Acceptable: empty (no home page), or an `http`, `https`, `file` or `about` URL without a user
/// name or password, at most [`MAX_URL_BYTES`] long before and after parsing. The form returned
/// is the parsed URL's text (so tabs and newlines the parser ignores are gone), which is what
/// the browser would open.
pub fn homepage_url(input: &str) -> Option<String> {
    if input.is_empty() {
        return Some(String::new());
    }
    if input.len() > MAX_URL_BYTES {
        return None;
    }
    let parsed = url::Url::parse(input).ok()?;
    let acceptable = matches!(parsed.scheme(), "http" | "https" | "file" | "about")
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed.as_str().len() <= MAX_URL_BYTES;
    acceptable.then(|| parsed.into())
}

/// The parsed text of `url` if a session may hold it for a tab: an `http`, `https`, `file` or
/// `about` URL of at most [`MAX_URL_BYTES`] before and after parsing, with any user name and
/// password removed. Only the user-info part is removed: tokens in the query string or the
/// fragment are kept.
pub(crate) fn session_url(url: &str) -> Option<String> {
    if url.len() > MAX_URL_BYTES {
        return None;
    }
    let mut parsed = url::Url::parse(url).ok()?;
    if !matches!(parsed.scheme(), "http" | "https" | "file" | "about") {
        return None;
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        // Only URLs with a host carry credentials, and those accept these calls.
        parsed.set_username("").ok()?;
        parsed.set_password(None).ok()?;
    }
    (parsed.as_str().len() <= MAX_URL_BYTES).then(|| parsed.into())
}

/// At most `max` bytes of `text`, cut at a character boundary.
pub(crate) fn truncated(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Whether `tag` is shaped like a BCP 47 language tag: a primary language subtag of 2 to 8
/// letters, then subtags of 1 to 8 letters or digits, separated by `-`, at most
/// [`MAX_LANGUAGE_TAG_BYTES`] in all. It does not check the subtags against the registry.
pub fn is_plausible_language_tag(tag: &str) -> bool {
    if tag.len() > MAX_LANGUAGE_TAG_BYTES {
        return false;
    }
    let mut subtags = tag.split('-');
    let primary_ok = subtags.next().is_some_and(|primary| {
        (2..=8).contains(&primary.len()) && primary.bytes().all(|byte| byte.is_ascii_alphabetic())
    });
    primary_ok
        && subtags.all(|subtag| {
            (1..=8).contains(&subtag.len())
                && subtag.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
}

/// The "never save" list with every entry an http(s) origin in canonical form
/// (`https://example.org`, `http://localhost:8080`), duplicates removed and the length capped,
/// and at most one repair describing what changed.
fn canonical_origins(entries: Vec<String>, unread: u64) -> (Vec<String>, Option<Repair>) {
    let mut origins: Vec<String> = Vec::new();
    let mut refused = 0_u64;
    let mut first_refused = None;
    let mut duplicates = 0_u64;
    let mut rewritten = 0_u64;
    let mut over_limit = unread;

    for entry in entries {
        let Some(origin) = canonical_origin(&entry) else {
            refused += 1;
            first_refused.get_or_insert_with(|| quoted(&entry));
            continue;
        };
        if origins.contains(&origin) {
            duplicates += 1;
            continue;
        }
        if origins.len() == MAX_NEVER_SAVE_ORIGINS {
            over_limit += 1;
            continue;
        }
        if origin != entry {
            rewritten += 1;
        }
        origins.push(origin);
    }

    let mut parts = Vec::new();
    if refused > 0 {
        parts.push(format!(
            "dropped {refused} not of the form scheme://host[:port] (the first: {})",
            first_refused.unwrap_or_default()
        ));
    }
    if duplicates > 0 {
        parts.push(format!("dropped {duplicates} listed twice"));
    }
    if rewritten > 0 {
        parts.push(format!("wrote {rewritten} in canonical form"));
    }
    if over_limit > 0 {
        parts.push(format!(
            "dropped {over_limit} beyond the first {MAX_NEVER_SAVE_ORIGINS}"
        ));
    }
    let repair = (!parts.is_empty()).then(|| Repair {
        field: bounded::NEVER_SAVE_ORIGINS,
        reason: parts.join("; "),
    });
    (origins, repair)
}

/// The canonical form of an entry in the "never save passwords" list, or `None` if it is not
/// an `http` or `https` origin: scheme, host and optional port only (a lone `/` path is
/// accepted), no user name, password, path, query or fragment, at most [`MAX_ORIGIN_BYTES`].
/// The form returned is the ASCII serialisation of the origin (`https://Bank.Example:443/` →
/// `https://bank.example`).
pub fn canonical_origin(entry: &str) -> Option<String> {
    if entry.len() > MAX_ORIGIN_BYTES {
        return None;
    }
    let url = url::Url::parse(entry).ok()?;
    let plain = matches!(url.scheme(), "http" | "https")
        && url.username().is_empty()
        && url.password().is_none()
        && url.path() == "/"
        && url.query().is_none()
        && url.fragment().is_none()
        && url.host().is_some();
    plain.then(|| url.origin().ascii_serialization())
}
