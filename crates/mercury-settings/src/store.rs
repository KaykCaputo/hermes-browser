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

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::atomic::{parse_temporary_name, write_private_atomic};
use crate::bounded;
use crate::error::SettingsError;
use crate::read::{Format, Inspection, Migration, Peek, inspect, peek};
use crate::session::{ClosedTab, MAX_RECOVERY_SESSIONS, SessionSnapshot, WindowSnapshot};
use crate::settings::{Repair, Settings};

/// The format version this crate writes into `settings.json`.
pub const SETTINGS_FORMAT_VERSION: u32 = 1;
/// The largest `settings.json` that is read; a larger one is treated as damaged (unless its
/// version, read first, is newer).
pub const MAX_SETTINGS_FILE_BYTES: u64 = 1024 * 1024;

/// How many `.damaged-<n>` names are tried before giving up.
const MAX_BACKUPS: u32 = 10_000;

const SETTINGS_FILE: &str = "settings.json";
const SESSION_FILE: &str = "session.json";
const PREVIOUS_FILE: &str = "session.previous.json";
const RECOVERY_FILE: &str = "session.recovery.json";
/// Every file this crate writes in a profile.
const OWN_FILES: [&str; 4] = [SETTINGS_FILE, SESSION_FILE, PREVIOUS_FILE, RECOVERY_FILE];

/// The outcome of loading a file.
#[derive(Clone, Debug, PartialEq)]
#[must_use = "a damaged or newer file needs telling the user, and repairs need saving"]
pub enum Loaded<T> {
    /// No file: first run. Defaults.
    Missing(T),
    /// The file was read. `repairs` lists every value that had to be corrected.
    Ok {
        /// The value read, after repairs.
        value: T,
        /// What was corrected; empty for a valid file. At most one repair per field.
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
        /// The format version found in the file; `u32::MAX` for any larger number.
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

/// `SETTINGS_MIGRATIONS[n]` migrates version `n` to `n + 1`; its length is the current version.
/// Version 0 is a document without `format_version` (hand-written, or from before versioning)
/// in the version 1 layout.
const SETTINGS_MIGRATIONS: [Migration; SETTINGS_FORMAT_VERSION as usize] = [Migration::Unchanged];

const SETTINGS_FORMAT: Format = Format {
    what: "settings",
    max_bytes: MAX_SETTINGS_FILE_BYTES,
    migrations: &SETTINGS_MIGRATIONS,
};

/// Moves a damaged file to the first free `<name>.damaged-<n>` beside it, never replacing an
/// earlier backup, and makes the backup private.
pub(crate) fn move_aside(path: &Path) -> Result<PathBuf, SettingsError> {
    move_aside_with(path, |from, to| std::fs::hard_link(from, to))
}

fn move_aside_with(
    path: &Path,
    link: impl Fn(&Path, &Path) -> io::Result<()>,
) -> Result<PathBuf, SettingsError> {
    let name = path.file_name().map_or_else(
        || "file".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    for n in 1..=MAX_BACKUPS {
        let backup = path.with_file_name(format!("{name}.damaged-{n}"));
        // A hard link fails if the name is taken, so an earlier backup can never be replaced,
        // even by a concurrent writer.
        match link(path, &backup) {
            Ok(()) => {
                if let Err(error) = std::fs::remove_file(path) {
                    log::warn!(
                        "kept a copy of damaged {} at {} but could not remove it: {error}",
                        path.display(),
                        backup.display()
                    );
                }
                make_private(&backup);
                return Ok(backup);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            // Hard links are not supported everywhere; fall back to a rename to a free name.
            Err(_) if std::fs::symlink_metadata(&backup).is_err() => {
                std::fs::rename(path, &backup)
                    .map_err(|error| SettingsError::io("move aside", path, error))?;
                make_private(&backup);
                return Ok(backup);
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

/// Mode 0600 for a backup, whatever the damaged file's own mode was. Only a regular file is
/// changed: a symlink's target is not the profile's to change.
fn make_private(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let is_file = std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_file());
        if is_file
            && let Err(error) =
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        {
            log::warn!("could not make {} private: {error}", path.display());
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Serialises `body` after `format_version` (and the `exited_cleanly` member, if given),
/// checks the size and writes it over `path` atomically, provided what is there may be
/// replaced. Only the version is read from the existing file: a missing file or one of this
/// version or older is replaced; a file from a newer version is an error; a file whose start
/// cannot be understood is moved aside first.
pub(crate) fn write_document<B: Serialize>(
    path: &Path,
    format: &Format,
    exited_cleanly: Option<bool>,
    body: &B,
) -> Result<(), SettingsError> {
    #[derive(Serialize)]
    struct Document<'a, B> {
        format_version: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        exited_cleanly: Option<bool>,
        #[serde(flatten)]
        body: &'a B,
    }

    let mut bytes = serde_json::to_vec_pretty(&Document {
        format_version: format.current_version(),
        exited_cleanly,
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

    match peek(path, format)? {
        Peek::Missing => {}
        Peek::Version(found) if found <= format.current_version() => {}
        Peek::Version(found) => {
            return Err(SettingsError::NewerVersion {
                path: path.to_owned(),
                found,
                supported: format.current_version(),
            });
        }
        Peek::Damaged(reason) => {
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

/// Reads and writes `<profile>/settings.json`.
#[derive(Clone, Debug)]
pub struct SettingsStore {
    path: PathBuf,
}

impl SettingsStore {
    /// A store for `<profile_dir>/settings.json`. Touches nothing on disk.
    pub fn new(profile_dir: &Path) -> Self {
        Self {
            path: profile_dir.join(SETTINGS_FILE),
        }
    }

    /// Reads the settings, validated (see [`Settings::validated`]).
    ///
    /// The version is read first: a file from a newer version is left untouched and reported
    /// as [`Loaded::Newer`], whatever its size. A file over [`MAX_SETTINGS_FILE_BYTES`], or one
    /// that is not a JSON object of the right shape, is moved aside and reported as
    /// [`Loaded::Damaged`]. Lists are read up to their limits and the rest skipped unbuilt, with
    /// one counted repair per list. A directory or other non-file at the path, or a file that
    /// cannot be read, is an error.
    pub fn load(&self) -> Result<Loaded<Settings>, SettingsError> {
        match inspect::<Settings>(&self.path, &SETTINGS_FORMAT)? {
            Inspection::Missing => Ok(Loaded::Missing(Settings::default())),
            Inspection::Current(settings, unread) => {
                let (value, repairs) = settings.validated_with(&unread);
                if !repairs.is_empty() {
                    let fields: Vec<&str> = repairs.iter().map(|repair| repair.field).collect();
                    log::info!(
                        "settings: {} repaired: {}",
                        repairs.len(),
                        fields.join(", ")
                    );
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
    /// written by a newer version. A file there whose start cannot be understood is moved
    /// aside first.
    pub fn save(&self, settings: &Settings) -> Result<(), SettingsError> {
        let (settings, _) = settings.clone().validated();
        write_document(&self.path, &SETTINGS_FORMAT, None, &settings)
    }
}

/// The format version this crate writes into the session files.
pub const SESSION_FORMAT_VERSION: u32 = 1;
/// The largest session file that is read; a larger one is treated as damaged (unless its
/// version, read first, is newer).
pub const MAX_SESSION_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// `SESSION_MIGRATIONS[n]` migrates version `n` to `n + 1`; its length is the current version.
/// Version 0 is a session document without `format_version`, in the version 1 layout.
const SESSION_MIGRATIONS: [Migration; SESSION_FORMAT_VERSION as usize] = [Migration::Unchanged];

const SESSION_FORMAT: Format = Format {
    what: "session",
    max_bytes: MAX_SESSION_FILE_BYTES,
    migrations: &SESSION_MIGRATIONS,
};

/// A session file as decoded: the snapshot's fields and the clean-exit mark, each list bounded
/// while it is read.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct SessionDocument {
    exited_cleanly: Option<bool>,
    #[serde(deserialize_with = "bounded::windows")]
    windows: Vec<WindowSnapshot>,
    #[serde(deserialize_with = "bounded::closed_tabs")]
    closed_tabs: Vec<ClosedTab>,
    saved_at_ms: i64,
}

impl SessionDocument {
    /// The snapshot, sanitized, and whether the run quit in order (no mark: it did not).
    fn into_previous(self) -> PreviousSession {
        PreviousSession {
            snapshot: SessionSnapshot {
                windows: self.windows,
                closed_tabs: self.closed_tabs,
                saved_at_ms: self.saved_at_ms,
            }
            .sanitized(),
            exited_cleanly: self.exited_cleanly.unwrap_or(false),
        }
    }
}

/// The format of `session.recovery.json`: `{"format_version": 1, "sessions": [...]}`, each
/// session in the layout of a session file without its clean-exit mark, newest first.
const RECOVERY_MIGRATIONS: [Migration; 1] = [Migration::Unchanged];

/// The largest recovery file that is read: room for [`MAX_RECOVERY_SESSIONS`] sessions of the
/// largest size.
const MAX_RECOVERY_FILE_BYTES: u64 = MAX_RECOVERY_SESSIONS as u64 * MAX_SESSION_FILE_BYTES;

const RECOVERY_FORMAT: Format = Format {
    what: "recovery list",
    max_bytes: MAX_RECOVERY_FILE_BYTES,
    migrations: &RECOVERY_MIGRATIONS,
};

/// The recovery file as decoded; at most [`MAX_RECOVERY_SESSIONS`] sessions are read, each with
/// its own tab budget.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RecoveryDocument {
    #[serde(deserialize_with = "bounded::recovery_sessions")]
    sessions: Vec<SessionSnapshot>,
}

#[derive(Serialize)]
struct RecoveryBody<'a> {
    sessions: &'a [SessionSnapshot],
}

/// URLs of pages that only start something new; a session with nothing else is not worth
/// keeping for recovery.
const NEW_TAB_PAGES: [&str; 3] = ["about:blank", "about:newtab", "about:home"];

/// Whether a sanitized session has anything worth restoring: a tab that is not a new-tab page.
fn worth_recovering(snapshot: &SessionSnapshot) -> bool {
    snapshot
        .windows
        .iter()
        .flat_map(|window| &window.tabs)
        .any(|tab| !NEW_TAB_PAGES.contains(&tab.url.as_str()))
}

/// Adds a crashed session to the recovery list, newest first. Returns whether the list
/// changed.
///
/// Not added: a session with nothing worth restoring, or one whose windows equal those of an
/// entry already held (when the closed tabs or the time saved differ, the restore would be the
/// same). When the list is full, the oldest entry is always kept — it is usually the session
/// the user most wants, from before a run of crashes — and the newest fill the rest, so the
/// entry dropped is the oldest of the others.
fn add_recovery(list: &mut Vec<SessionSnapshot>, crashed: &SessionSnapshot) -> bool {
    if !worth_recovering(crashed) || list.iter().any(|held| held.windows == crashed.windows) {
        return false;
    }
    list.insert(0, crashed.clone());
    if list.len() > MAX_RECOVERY_SESSIONS {
        list.remove(list.len() - 2);
    }
    true
}

/// A session found at start-up, as [`SessionStore::begin_run`] read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviousSession {
    /// The windows and tabs, sanitized.
    pub snapshot: SessionSnapshot,
    /// Whether that run quit in order. False after a crash or a kill.
    pub exited_cleanly: bool,
}

/// What [`SessionStore::begin_run`] found.
///
/// The two fields answer different questions:
///
/// - `recovery` is "restore after a crash": every session from a run that did not quit in
///   order and that the user has not yet restored or dismissed, newest first. Offer it
///   whenever it is not empty, whatever the start-up setting, until
///   [`SessionStore::resolve_recovery`] is called.
/// - `last` is "continue where you left off". Act on it only when `exited_cleanly` is true:
///   restore it if the start-up setting asks for the previous session, otherwise keep it for
///   "restore previous session". When `last` did not quit in order it is already the first
///   entry of `recovery` (or equal to an entry there, or had nothing worth restoring), and is
///   returned for information only; the caller never needs to treat it separately.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[must_use = "offer the recovery list and decide whether to restore the last session"]
pub struct StartupSessions {
    /// The session the last run saved, if one could be read.
    pub last: Option<PreviousSession>,
    /// Sessions from runs that did not quit in order, newest first, at most
    /// [`MAX_RECOVERY_SESSIONS`], each sanitized and with something worth restoring.
    pub recovery: Vec<SessionSnapshot>,
}

/// What the recovery file holds.
enum Slot {
    List(Vec<SessionSnapshot>),
    /// A file from a newer version: neither used nor replaced.
    Unusable,
}

/// Reads and writes the session files of a profile:
///
/// - `session.json`: the running session, saved by [`save`](Self::save);
/// - `session.previous.json`: a copy of the last run's session, made by
///   [`begin_run`](Self::begin_run), used if `session.json` is damaged or missing;
/// - `session.recovery.json`: the recovery list — sessions from runs that did not quit in
///   order, kept until [`resolve_recovery`](Self::resolve_recovery).
#[derive(Clone, Debug)]
pub struct SessionStore {
    path: PathBuf,
    previous_path: PathBuf,
    recovery_path: PathBuf,
}

impl SessionStore {
    /// A store for the session files of `profile_dir`. Touches nothing.
    pub fn new(profile_dir: &Path) -> Self {
        Self {
            path: profile_dir.join(SESSION_FILE),
            previous_path: profile_dir.join(PREVIOUS_FILE),
            recovery_path: profile_dir.join(RECOVERY_FILE),
        }
    }

    /// Call once at start-up, before any session save: returns the last run's session and the
    /// recovery list, and keeps copies so that both stay recoverable after this run starts
    /// saving.
    ///
    /// In order:
    ///
    /// 1. The recovery file is read. A damaged one is moved aside and counts as an empty list;
    ///    one from a newer version is left alone and never replaced.
    /// 2. `session.json` is read as `last`. If it is damaged (moved aside),
    ///    `session.previous.json` is offered in its place, marked as not having quit in order.
    ///    If it is from a newer version, it is left alone and `last` is `None`.
    /// 3. If `last` did not quit in order, it is added to the recovery list (see
    ///    [`MAX_RECOVERY_SESSIONS`] for what is added and what is dropped when the list is
    ///    full), and the list is written if it changed. With a newer-version recovery file, the
    ///    list returned holds only this crash, which is not kept on disk.
    /// 4. If `last` came from `session.json`, it is copied to `session.previous.json`.
    /// 5. If `last` quit in order, `session.json` is marked as running, so that a crash of this
    ///    run before its first save is still reported as a crash next time.
    ///
    /// Sessions returned are [sanitized](SessionSnapshot::sanitized). Failing to write the
    /// list, the copy or the mark is logged, not returned, so no session is lost to a full
    /// disk. Saving the running session at any time afterwards is safe: it never touches the
    /// recovery file. A directory or other non-file at a path, an unreadable file, or a damaged
    /// file that cannot be moved aside is an error.
    pub fn begin_run(&self) -> Result<StartupSessions, SettingsError> {
        let slot = self.read_slot()?;
        let (last, from_current) = self.read_last()?;
        let crashed = last
            .as_ref()
            .filter(|previous| !previous.exited_cleanly)
            .map(|previous| &previous.snapshot);

        let recovery = match slot {
            Slot::List(mut list) => {
                if let Some(crashed) = crashed
                    && add_recovery(&mut list, crashed)
                {
                    let kept = write_document(
                        &self.recovery_path,
                        &RECOVERY_FORMAT,
                        None,
                        &RecoveryBody { sessions: &list },
                    );
                    if let Err(error) = kept {
                        log::warn!("could not keep the crashed session for recovery: {error}");
                    }
                }
                list
            }
            Slot::Unusable => {
                let mut list = Vec::new();
                if let Some(crashed) = crashed {
                    add_recovery(&mut list, crashed);
                }
                list
            }
        };

        if let Some(previous) = last.as_ref().filter(|_| from_current) {
            let copied = write_document(
                &self.previous_path,
                &SESSION_FORMAT,
                Some(previous.exited_cleanly),
                &previous.snapshot,
            );
            if let Err(error) = copied {
                log::warn!("could not keep a copy of the previous session: {error}");
            }
            if previous.exited_cleanly {
                let running =
                    write_document(&self.path, &SESSION_FORMAT, Some(false), &previous.snapshot);
                if let Err(error) = running {
                    log::warn!("could not mark the session as running: {error}");
                }
            }
        }

        Ok(StartupSessions { last, recovery })
    }

    /// Clears the whole recovery list once the user has restored or dismissed it. Until then
    /// every [`begin_run`](Self::begin_run) offers it again.
    ///
    /// Nothing to clear is not an error. A recovery file from a newer version is not removed
    /// ([`SettingsError::NewerVersion`]); a damaged one is moved aside rather than deleted.
    pub fn resolve_recovery(&self) -> Result<(), SettingsError> {
        match peek(&self.recovery_path, &RECOVERY_FORMAT)? {
            Peek::Missing => Ok(()),
            Peek::Version(found) if found > RECOVERY_FORMAT.current_version() => {
                Err(SettingsError::NewerVersion {
                    path: self.recovery_path.clone(),
                    found,
                    supported: RECOVERY_FORMAT.current_version(),
                })
            }
            Peek::Damaged(_) => move_aside(&self.recovery_path).map(|_| ()),
            Peek::Version(_) => match std::fs::remove_file(&self.recovery_path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(SettingsError::io("remove", &self.recovery_path, error)),
            },
        }
    }

    /// Saves the running session. `exited_cleanly` is false while running and true only for
    /// the final save of an orderly quit (made even when nothing changed; see
    /// [`SaveThrottle`](crate::SaveThrottle)).
    ///
    /// The snapshot is written [sanitized](SessionSnapshot::sanitized), privately and
    /// atomically. Refused with [`SettingsError::NewerVersion`], changing nothing, when the file
    /// on disk was written by a newer version; a file there whose start cannot be understood is
    /// moved aside first. The recovery file is never touched.
    pub fn save(
        &self,
        snapshot: &SessionSnapshot,
        exited_cleanly: bool,
    ) -> Result<(), SettingsError> {
        write_document(
            &self.path,
            &SESSION_FORMAT,
            Some(exited_cleanly),
            &snapshot.clone().sanitized(),
        )
    }

    fn read_slot(&self) -> Result<Slot, SettingsError> {
        Ok(
            match inspect::<RecoveryDocument>(&self.recovery_path, &RECOVERY_FORMAT)? {
                Inspection::Missing => Slot::List(Vec::new()),
                Inspection::Newer(found) => {
                    log::warn!(
                        "{} was written by a newer version (format {found}); leaving it alone",
                        self.recovery_path.display()
                    );
                    Slot::Unusable
                }
                Inspection::Damaged(reason) => {
                    let backup = move_aside(&self.recovery_path)?;
                    log::warn!(
                        "{} could not be understood ({reason}); moved to {}",
                        self.recovery_path.display(),
                        backup.display()
                    );
                    Slot::List(Vec::new())
                }
                Inspection::Current(document, _) => Slot::List(
                    document
                        .sessions
                        .into_iter()
                        .map(SessionSnapshot::sanitized)
                        .filter(worth_recovering)
                        .collect(),
                ),
            },
        )
    }

    /// The last run's session and whether it came from `session.json` (rather than the
    /// previous copy).
    fn read_last(&self) -> Result<(Option<PreviousSession>, bool), SettingsError> {
        Ok(match self.read(&self.path)? {
            Read::Current(previous) => (Some(previous), true),
            Read::Missing | Read::Newer => (None, false),
            Read::Damaged => match self.read(&self.previous_path)? {
                Read::Current(previous) => {
                    log::warn!("offering the copy of the session made at the last start");
                    (
                        Some(PreviousSession {
                            exited_cleanly: false,
                            ..previous
                        }),
                        false,
                    )
                }
                Read::Missing | Read::Newer | Read::Damaged => (None, false),
            },
        })
    }

    /// Reads one session file; a damaged one is moved aside.
    fn read(&self, path: &Path) -> Result<Read, SettingsError> {
        Ok(match inspect::<SessionDocument>(path, &SESSION_FORMAT)? {
            Inspection::Missing => Read::Missing,
            Inspection::Newer(found) => {
                log::warn!(
                    "{} was written by a newer version (format {found}); leaving it alone",
                    path.display()
                );
                Read::Newer
            }
            Inspection::Damaged(reason) => {
                let backup = move_aside(path)?;
                log::warn!(
                    "{} could not be understood ({reason}); moved to {}",
                    path.display(),
                    backup.display()
                );
                Read::Damaged
            }
            Inspection::Current(document, unread) => {
                if unread.total() > 0 {
                    log::warn!(
                        "{}: {} items beyond the limits were not read",
                        path.display(),
                        unread.total()
                    );
                }
                Read::Current(document.into_previous())
            }
        })
    }
}

/// One session file, read.
enum Read {
    Missing,
    Newer,
    Damaged,
    Current(PreviousSession),
}

/// Removes temporary files left in `profile_dir` by writes of this crate's own files that never
/// finished (a crash between creating and renaming), and returns how many it removed.
///
/// Only regular files named exactly as [`write_private_atomic`] names temporaries of
/// `settings.json`, `session.json`, `session.previous.json` and `session.recovery.json`
/// (`.<file>.<pid>-<n>.tmp`) are removed, and not those of the current process.
///
/// **Call it only while holding the profile's single-instance lock.** A temporary file of
/// another process that is still running is a write in progress: removing it makes that
/// write fail. A process id cannot tell a live process from a dead one (ids are reused), so
/// only the lock guarantees that no other process is writing in this profile.
pub fn sweep_stale_temporaries(profile_dir: &Path) -> io::Result<u64> {
    let own = std::process::id();
    let mut removed = 0;
    for entry in std::fs::read_dir(profile_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some((target, pid)) = name.to_str().and_then(parse_temporary_name) else {
            continue;
        };
        if pid == own || !OWN_FILES.contains(&target) {
            continue;
        }
        let path = entry.path();
        if !std::fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.is_file()) {
            continue;
        }
        match std::fs::remove_file(&path) {
            Ok(()) => removed += 1,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, Value};

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
        migrations: &[
            Migration::Transform(invented_v0_to_v1),
            Migration::Transform(mark_v1_to_v2),
        ],
    };

    fn inspect_text(text: &str, format: &Format) -> Inspection<Settings> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, text).unwrap();
        inspect(&path, format).unwrap()
    }

    #[test]
    fn the_ladders_have_one_step_per_version() {
        assert_eq!(SETTINGS_FORMAT.current_version(), SETTINGS_FORMAT_VERSION);
        assert_eq!(SESSION_FORMAT.current_version(), SESSION_FORMAT_VERSION);
        assert_eq!(RECOVERY_FORMAT.current_version(), 1);
    }

    #[test]
    fn a_hand_written_version_0_document_climbs_every_step() {
        let inspection = inspect_text(
            r#"{"format_version": 0, "zoom": 150, "search_engine": "duckduckgo"}"#,
            &INVENTED,
        );

        let Inspection::Current(settings, _) = inspection else {
            panic!("expected a migrated document, got {inspection:?}");
        };
        assert_eq!(settings.appearance.default_zoom_percent, 150);
        assert_eq!(settings.search.default_engine, "duckduckgo");
        assert_eq!(settings.homepage, "about:blank", "the second step ran too");
    }

    #[test]
    fn a_middle_version_runs_only_the_steps_above_it() {
        let inspection = inspect_text(r#"{"format_version": 1, "zoom": 150}"#, &INVENTED);

        let Inspection::Current(settings, _) = inspection else {
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

    #[test]
    fn a_migration_is_bounded_too() {
        let bomb = format!(
            r#"{{"format_version": 0, "padding": [{}0]}}"#,
            "0,".repeat(bounded::MAX_TREE_NODES)
        );
        let inspection = inspect_text(&bomb, &INVENTED);
        assert!(
            matches!(inspection, Inspection::Damaged(_)),
            "{inspection:?}"
        );
    }

    #[test]
    fn the_migration_path_still_bounds_lists() {
        let engines = vec!["{}"; crate::MAX_CUSTOM_ENGINES + 7].join(",");
        let inspection = inspect_text(
            &format!(r#"{{"format_version": 1, "search": {{"custom_engines": [{engines}]}}}}"#),
            &INVENTED,
        );
        let Inspection::Current(settings, unread) = inspection else {
            panic!("expected a migrated document, got {inspection:?}");
        };
        assert_eq!(
            settings.search.custom_engines.len(),
            crate::MAX_CUSTOM_ENGINES
        );
        assert_eq!(unread.get(bounded::CUSTOM_ENGINES), 7);
    }

    #[test]
    fn without_hard_links_a_damaged_file_is_renamed_to_a_free_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, b"damaged").unwrap();
        let earlier = dir.path().join("settings.json.damaged-1");
        std::fs::write(&earlier, b"an earlier backup").unwrap();

        let backup = move_aside_with(&path, |_, _| {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "no hard links here",
            ))
        })
        .unwrap();

        assert_eq!(backup, dir.path().join("settings.json.damaged-2"));
        assert_eq!(std::fs::read(&backup).unwrap(), b"damaged");
        assert_eq!(std::fs::read(&earlier).unwrap(), b"an earlier backup");
        assert!(!path.exists());
    }
}
