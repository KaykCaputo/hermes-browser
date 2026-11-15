//! Loading and saving the files in the profile directory.
//!
//! Each file is one pretty-printed JSON object whose first member is `"format_version"`.
//! Loading reads at most a fixed number of bytes, refuses anything that is not a JSON object,
//! migrates older versions one step at a time, and never trusts a file written by a newer
//! version enough to overwrite it. A file that cannot be understood is moved aside to
//! `<name>.damaged-<n>` (the first free `n`) before anything replaces it.
//!
//! A version bump is needed for any change an older reader would misread or reject: a new enum
//! value (an older reader would call the file damaged), a renamed or retyped field. Adding a
//! field with a default needs none.

use std::fs::File;
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use crate::atomic::write_private_atomic;
use crate::error::SettingsError;
use crate::session::SessionSnapshot;
use crate::settings::{Repair, Settings};

/// The format version this crate writes into `settings.json`.
pub const SETTINGS_FORMAT_VERSION: u32 = 1;
/// The largest `settings.json` that is read; a larger one is treated as damaged.
pub const MAX_SETTINGS_FILE_BYTES: u64 = 1024 * 1024;

const VERSION_KEY: &str = "format_version";
/// How many `.damaged-<n>` names are tried before giving up.
const MAX_BACKUPS: u32 = 10_000;

/// The outcome of loading a file.
#[derive(Clone, Debug, PartialEq)]
pub enum Loaded<T> {
    /// No file: first run. Defaults.
    Missing(T),
    /// The file was read. `repairs` lists every value that had to be corrected.
    Ok {
        /// The value read, after repairs.
        value: T,
        /// What was corrected; empty for a valid file.
        repairs: Vec<Repair>,
    },
    /// The file could not be understood. It was moved aside to `backup`; defaults are used.
    Damaged {
        /// Defaults.
        value: T,
        /// Where the damaged file now is.
        backup: PathBuf,
    },
    /// Written by a newer version. Defaults are used and saving is refused.
    Newer {
        /// Defaults.
        value: T,
        /// The format version found in the file.
        found_version: u32,
    },
}

impl<T> Loaded<T> {
    /// The value to use, whatever the outcome.
    pub fn value(&self) -> &T {
        match self {
            Self::Missing(value)
            | Self::Ok { value, .. }
            | Self::Damaged { value, .. }
            | Self::Newer { value, .. } => value,
        }
    }

    /// The value to use, whatever the outcome.
    pub fn into_value(self) -> T {
        match self {
            Self::Missing(value)
            | Self::Ok { value, .. }
            | Self::Damaged { value, .. }
            | Self::Newer { value, .. } => value,
        }
    }
}

/// One step of a migration ladder: turns a document of version `n` into one of version `n + 1`.
pub(crate) type Migration = fn(&mut Map<String, Value>);

/// Version 0 is a document without `format_version` (hand-written, or from before versioning)
/// in the version 1 layout, so the step only has to exist.
fn settings_v0_to_v1(_document: &mut Map<String, Value>) {}

/// `SETTINGS_MIGRATIONS[n]` migrates version `n` to `n + 1`; its length is the current version.
const SETTINGS_MIGRATIONS: [Migration; SETTINGS_FORMAT_VERSION as usize] = [settings_v0_to_v1];

/// A file format: its name in messages, its size limit and its migration ladder.
pub(crate) struct Format {
    pub(crate) what: &'static str,
    pub(crate) max_bytes: u64,
    pub(crate) migrations: &'static [Migration],
}

impl Format {
    fn current_version(&self) -> u32 {
        u32::try_from(self.migrations.len()).unwrap_or(u32::MAX)
    }
}

const SETTINGS_FORMAT: Format = Format {
    what: "settings",
    max_bytes: MAX_SETTINGS_FILE_BYTES,
    migrations: &SETTINGS_MIGRATIONS,
};

/// What is on disk at a path, decoded as far as it goes.
#[derive(Debug)]
pub(crate) enum Inspection<T> {
    Missing,
    Damaged(String),
    Newer(u32),
    Current(T),
}

/// Reads at most `max + 1` bytes of a regular file. `None` when there is no file.
fn read_limited(path: &Path, max: u64) -> Result<Option<Result<Vec<u8>, u64>>, SettingsError> {
    // Checked before opening: opening a FIFO for reading would block until a writer appears.
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(SettingsError::io("inspect", path, error)),
    };
    if !metadata.is_file() {
        let kind = if metadata.is_dir() {
            io::ErrorKind::IsADirectory
        } else {
            io::ErrorKind::InvalidInput
        };
        return Err(SettingsError::io(
            "read",
            path,
            io::Error::new(kind, "not a regular file"),
        ));
    }
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(SettingsError::io("open", path, error)),
    };
    let mut bytes = Vec::new();
    file.take(max.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| SettingsError::io("read", path, error))?;
    let length = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if length > max {
        return Ok(Some(Err(length)));
    }
    Ok(Some(Ok(bytes)))
}

/// Reads, checks the version of, migrates and decodes the file at `path`.
pub(crate) fn inspect<T>(
    path: &Path,
    format: &Format,
    decode: impl FnOnce(Map<String, Value>) -> Result<T, String>,
) -> Result<Inspection<T>, SettingsError> {
    let bytes = match read_limited(path, format.max_bytes)? {
        None => return Ok(Inspection::Missing),
        Some(Err(_)) => {
            return Ok(Inspection::Damaged(format!(
                "larger than {} bytes",
                format.max_bytes
            )));
        }
        Some(Ok(bytes)) => bytes,
    };
    let document = match serde_json::from_slice::<Value>(&bytes) {
        Ok(Value::Object(document)) => document,
        Ok(_) => return Ok(Inspection::Damaged("not a JSON object".to_owned())),
        Err(error) => return Ok(Inspection::Damaged(format!("not JSON: {error}"))),
    };
    Ok(migrate(document, format.migrations).map_or_else(
        |problem| problem,
        |document| match decode(document) {
            Ok(value) => Inspection::Current(value),
            Err(reason) => Inspection::Damaged(reason),
        },
    ))
}

/// Takes `format_version` out of `document` and runs the ladder from that version up.
pub(crate) fn migrate<T>(
    mut document: Map<String, Value>,
    migrations: &[Migration],
) -> Result<Map<String, Value>, Inspection<T>> {
    let version = match document.remove(VERSION_KEY) {
        None => 0,
        Some(value) => match value.as_u64().and_then(|value| u32::try_from(value).ok()) {
            Some(version) => version,
            None => {
                return Err(Inspection::Damaged(format!(
                    "{VERSION_KEY} is not a version number"
                )));
            }
        },
    };
    let Some(steps) = migrations.get(version as usize..) else {
        return Err(Inspection::Newer(version));
    };
    for step in steps {
        step(&mut document);
    }
    Ok(document)
}

/// Moves a damaged file to the first free `<name>.damaged-<n>` beside it, never replacing an
/// earlier backup.
pub(crate) fn move_aside(path: &Path) -> Result<PathBuf, SettingsError> {
    let name = path.file_name().map_or_else(
        || "file".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    for n in 1..=MAX_BACKUPS {
        let backup = path.with_file_name(format!("{name}.damaged-{n}"));
        // A hard link fails if the name is taken, so an earlier backup can never be replaced,
        // even by a concurrent writer.
        match std::fs::hard_link(path, &backup) {
            Ok(()) => {
                if let Err(error) = std::fs::remove_file(path) {
                    log::warn!(
                        "kept a copy of damaged {} at {} but could not remove it: {error}",
                        path.display(),
                        backup.display()
                    );
                }
                return Ok(backup);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            // Hard links are not supported everywhere; fall back to a rename to a free name.
            Err(_) if std::fs::symlink_metadata(&backup).is_err() => {
                return std::fs::rename(path, &backup)
                    .map(|()| backup)
                    .map_err(|error| SettingsError::io("move aside", path, error));
            }
            Err(_) => {}
        }
    }
    Err(SettingsError::io(
        "move aside",
        path,
        io::Error::new(io::ErrorKind::AlreadyExists, "no free backup name"),
    ))
}

/// Serialises `body` after `format_version` (and any members in `header`), checks the size and
/// writes it over `path` atomically, provided what is there may be replaced: a missing file or
/// one of this version or older is replaced; a file from a newer version is an error; a file
/// that cannot be understood is moved aside first.
pub(crate) fn write_document<B: Serialize>(
    path: &Path,
    format: &Format,
    header: Option<(&'static str, bool)>,
    body: &B,
) -> Result<(), SettingsError> {
    #[derive(Serialize)]
    struct Document<'a, B> {
        format_version: u32,
        #[serde(flatten)]
        header: Map<String, Value>,
        #[serde(flatten)]
        body: &'a B,
    }

    let mut header_members = Map::new();
    if let Some((key, value)) = header {
        header_members.insert(key.to_owned(), Value::Bool(value));
    }
    let mut bytes = serde_json::to_vec_pretty(&Document {
        format_version: format.current_version(),
        header: header_members,
        body,
    })
    .map_err(|source| SettingsError::Serialize {
        what: format.what,
        source,
    })?;
    bytes.push(b'\n');
    let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if size > format.max_bytes {
        return Err(SettingsError::TooLarge {
            what: format.what,
            size,
            limit: format.max_bytes,
        });
    }

    match inspect(path, format, |_| Ok(()))? {
        Inspection::Missing | Inspection::Current(()) => {}
        Inspection::Newer(found) => {
            return Err(SettingsError::NewerVersion {
                path: path.to_owned(),
                found,
                supported: format.current_version(),
            });
        }
        Inspection::Damaged(reason) => {
            let backup = move_aside(path)?;
            log::warn!(
                "{} could not be understood ({reason}); kept at {} before replacing it",
                path.display(),
                backup.display()
            );
        }
    }

    write_private_atomic(path, &bytes).map_err(|error| SettingsError::io("write", path, error))
}

/// Decodes a migrated document into `T`.
pub(crate) fn decode<T: DeserializeOwned>(document: Map<String, Value>) -> Result<T, String> {
    serde_json::from_value(Value::Object(document)).map_err(|error| error.to_string())
}

/// Reads and writes `<profile>/settings.json`.
#[derive(Clone, Debug)]
pub struct SettingsStore {
    path: PathBuf,
}

impl SettingsStore {
    /// A store for `<profile_dir>/settings.json`. Touches nothing on disk.
    pub fn new(profile_dir: &Path) -> Self {
        Self {
            path: profile_dir.join("settings.json"),
        }
    }

    /// Reads the settings, validated (see [`Settings::validated`]).
    ///
    /// A file over [`MAX_SETTINGS_FILE_BYTES`], or one that is not a JSON object of the right
    /// shape, is moved aside and reported as [`Loaded::Damaged`]. A file from a newer version is
    /// left untouched and reported as [`Loaded::Newer`]. A directory or other non-file at the
    /// path, or a file that cannot be read, is an error.
    pub fn load(&self) -> Result<Loaded<Settings>, SettingsError> {
        match inspect(&self.path, &SETTINGS_FORMAT, decode::<Settings>)? {
            Inspection::Missing => Ok(Loaded::Missing(Settings::default())),
            Inspection::Current(settings) => {
                let (value, repairs) = settings.validated();
                for repair in &repairs {
                    log::info!("settings: {}: {}", repair.field, repair.reason);
                }
                Ok(Loaded::Ok { value, repairs })
            }
            Inspection::Newer(found_version) => {
                log::warn!(
                    "{} was written by a newer version (format {found_version}); using defaults \
                     and not saving",
                    self.path.display()
                );
                Ok(Loaded::Newer {
                    value: Settings::default(),
                    found_version,
                })
            }
            Inspection::Damaged(reason) => {
                let backup = move_aside(&self.path)?;
                log::warn!(
                    "{} could not be understood ({reason}); moved to {}; using defaults",
                    self.path.display(),
                    backup.display()
                );
                Ok(Loaded::Damaged {
                    value: Settings::default(),
                    backup,
                })
            }
        }
    }

    /// Writes the settings, validated, privately and atomically.
    ///
    /// Refused with [`SettingsError::NewerVersion`], changing nothing, when the file on disk was
    /// written by a newer version. A file there that cannot be understood is moved aside first.
    pub fn save(&self, settings: &Settings) -> Result<(), SettingsError> {
        let (settings, _) = settings.clone().validated();
        write_document(&self.path, &SETTINGS_FORMAT, None, &settings)
    }
}

/// The format version this crate writes into `session.json`.
pub const SESSION_FORMAT_VERSION: u32 = 1;
/// The largest `session.json` that is read; a larger one is treated as damaged.
pub const MAX_SESSION_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// The session of the previous run, as [`SessionStore::begin_run`] found it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviousSession {
    /// The windows and tabs, sanitized.
    pub snapshot: SessionSnapshot,
    /// Whether that run quit in order. False after a crash or a kill.
    pub exited_cleanly: bool,
}

/// Reads and writes `<profile>/session.json` and keeps `<profile>/session.previous.json`.
#[derive(Clone, Debug)]
pub struct SessionStore {
    path: PathBuf,
    previous_path: PathBuf,
}

impl SessionStore {
    /// A store for `<profile_dir>/session.json` and `session.previous.json`. Touches nothing.
    pub fn new(profile_dir: &Path) -> Self {
        Self {
            path: profile_dir.join("session.json"),
            previous_path: profile_dir.join("session.previous.json"),
        }
    }

    /// Call once at start-up: returns the last run's session, if any, and keeps a copy of it
    /// as the "previous" file so it stays recoverable after this run starts saving.
    ///
    /// The session returned is [sanitized](SessionSnapshot::sanitized). If the last run quit in
    /// order, `session.json` is marked as running again, so that a crash of this run before its
    /// first save is still reported as a crash next time. A damaged file is moved aside like
    /// damaged settings and gives `None`; a file from a newer version is left untouched and
    /// gives `None`, and [`save`](Self::save) will refuse to replace it. Failing to write the
    /// "previous" copy or the running mark is logged, not returned, so the session is never
    /// lost to a full disk. A directory or other non-file at the path, an unreadable file, or a
    /// damaged file that cannot be moved aside is an error.
    pub fn begin_run(&self) -> Result<Option<PreviousSession>, SettingsError> {
        let (snapshot, exited_cleanly) = match inspect(&self.path, &SESSION_FORMAT, decode_session)?
        {
            Inspection::Missing => return Ok(None),
            Inspection::Newer(found) => {
                log::warn!(
                    "{} was written by a newer version (format {found}); not restoring it \
                         and not saving over it",
                    self.path.display()
                );
                return Ok(None);
            }
            Inspection::Damaged(reason) => {
                let backup = move_aside(&self.path)?;
                log::warn!(
                    "{} could not be understood ({reason}); moved to {}",
                    self.path.display(),
                    backup.display()
                );
                return Ok(None);
            }
            Inspection::Current(found) => found,
        };
        let snapshot = snapshot.sanitized();

        let previous = write_document(
            &self.previous_path,
            &SESSION_FORMAT,
            Some((EXITED_CLEANLY_KEY, exited_cleanly)),
            &snapshot,
        );
        if let Err(error) = previous {
            log::warn!("could not keep a copy of the previous session: {error}");
        }
        if exited_cleanly {
            let running = write_document(
                &self.path,
                &SESSION_FORMAT,
                Some((EXITED_CLEANLY_KEY, false)),
                &snapshot,
            );
            if let Err(error) = running {
                log::warn!("could not mark the session as running: {error}");
            }
        }

        Ok(Some(PreviousSession {
            snapshot,
            exited_cleanly,
        }))
    }

    /// Saves the running session. `exited_cleanly` is false while running and true only for
    /// the final save of an orderly quit.
    ///
    /// The snapshot is written [sanitized](SessionSnapshot::sanitized), privately and
    /// atomically. Refused with [`SettingsError::NewerVersion`], changing nothing, when the file
    /// on disk was written by a newer version; a file there that cannot be understood is moved
    /// aside first.
    pub fn save(
        &self,
        snapshot: &SessionSnapshot,
        exited_cleanly: bool,
    ) -> Result<(), SettingsError> {
        write_document(
            &self.path,
            &SESSION_FORMAT,
            Some((EXITED_CLEANLY_KEY, exited_cleanly)),
            &snapshot.clone().sanitized(),
        )
    }
}

const EXITED_CLEANLY_KEY: &str = "exited_cleanly";

/// Version 0 is a session document without `format_version`, in the version 1 layout.
fn session_v0_to_v1(_document: &mut Map<String, Value>) {}

/// `SESSION_MIGRATIONS[n]` migrates version `n` to `n + 1`; its length is the current version.
const SESSION_MIGRATIONS: [Migration; SESSION_FORMAT_VERSION as usize] = [session_v0_to_v1];

const SESSION_FORMAT: Format = Format {
    what: "session",
    max_bytes: MAX_SESSION_FILE_BYTES,
    migrations: &SESSION_MIGRATIONS,
};

/// The snapshot and the clean-exit mark; a missing mark means the run did not quit in order.
fn decode_session(mut document: Map<String, Value>) -> Result<(SessionSnapshot, bool), String> {
    let exited_cleanly = match document.remove(EXITED_CLEANLY_KEY) {
        None => false,
        Some(Value::Bool(clean)) => clean,
        Some(_) => return Err(format!("{EXITED_CLEANLY_KEY} is not true or false")),
    };
    Ok((decode::<SessionSnapshot>(document)?, exited_cleanly))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A made-up history: version 0 kept the zoom at the top level as `zoom` and named the
    /// engine `search_engine`; version 1 is the real layout. The ladder must carry both across.
    fn invented_v0_to_v1(document: &mut Map<String, Value>) {
        if let Some(zoom) = document.remove("zoom") {
            document.insert(
                "appearance".to_owned(),
                serde_json::json!({ "default_zoom_percent": zoom }),
            );
        }
        if let Some(engine) = document.remove("search_engine") {
            document.insert(
                "search".to_owned(),
                serde_json::json!({ "default_engine": engine }),
            );
        }
    }

    fn mark_v1_to_v2(document: &mut Map<String, Value>) {
        document.insert("homepage".to_owned(), Value::from("about:blank"));
    }

    const INVENTED: Format = Format {
        what: "settings",
        max_bytes: MAX_SETTINGS_FILE_BYTES,
        migrations: &[invented_v0_to_v1, mark_v1_to_v2],
    };

    fn inspect_text(text: &str, format: &Format) -> Inspection<Settings> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, text).unwrap();
        inspect(&path, format, decode::<Settings>).unwrap()
    }

    #[test]
    fn the_ladders_have_one_step_per_version() {
        assert_eq!(SETTINGS_FORMAT.current_version(), SETTINGS_FORMAT_VERSION);
        assert_eq!(SESSION_FORMAT.current_version(), SESSION_FORMAT_VERSION);
    }

    #[test]
    fn a_hand_written_version_0_document_climbs_every_step() {
        let inspection = inspect_text(
            r#"{"format_version": 0, "zoom": 150, "search_engine": "duckduckgo"}"#,
            &INVENTED,
        );

        let Inspection::Current(settings) = inspection else {
            panic!("expected a migrated document, got {inspection:?}");
        };
        assert_eq!(settings.appearance.default_zoom_percent, 150);
        assert_eq!(settings.search.default_engine, "duckduckgo");
        assert_eq!(settings.homepage, "about:blank", "the second step ran too");
    }

    #[test]
    fn a_middle_version_runs_only_the_steps_above_it() {
        let inspection = inspect_text(r#"{"format_version": 1, "zoom": 150}"#, &INVENTED);

        let Inspection::Current(settings) = inspection else {
            panic!("expected a migrated document, got {inspection:?}");
        };
        assert_eq!(
            settings.appearance.default_zoom_percent, 100,
            "the 0 -> 1 step must not run on a version 1 document"
        );
        assert_eq!(settings.homepage, "about:blank");
    }

    #[test]
    fn a_version_beyond_the_ladder_is_newer() {
        let inspection = inspect_text(r#"{"format_version": 3}"#, &INVENTED);
        assert!(matches!(inspection, Inspection::Newer(3)), "{inspection:?}");
    }
}
