//! A private directory for each run, where GLib and GStreamer write (download buffers among
//! them: copies of the media being played). It is removed when the run ends, and leftovers of
//! runs that were killed are removed at the next start.

use std::fs::{DirBuilder, OpenOptions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::media_isolation::{
    GLIB_USER_DIRECTORIES, apply, glib_user_directories, mercury_cache_dir,
};
use crate::profile::{LockError, ProfileLock};

/// Prefix of a run directory while it is being created, before it is locked and renamed.
const STAGING_PREFIX: &str = ".new-";
/// A staging directory this old belongs to a run that stopped while creating it.
const STALE_STAGING_AGE: Duration = Duration::from_secs(10 * 60);
/// At most this many entries of the area are examined per start-up.
const MAX_SWEEP_ENTRIES: usize = 4096;

/// For the browser process, once, right after the early environment changes in `main`:
/// removes what killed runs left behind, creates this run's directory and points GLib's user
/// directories into it, so that the content processes started later (where GStreamer runs)
/// inherit them. The directory is removed when the returned value is dropped.
///
/// If the directory cannot be created, the variables point at a directory that does not
/// exist: GStreamer then plays without download buffers instead of writing them elsewhere.
pub(crate) fn keep_media_buffers_private() -> Option<MediaBuffers> {
    let area = mercury_cache_dir(dirs::cache_dir()).join("media-buffers");
    sweep(&area, SystemTime::now(), STALE_STAGING_AGE);
    let (buffers, root) = match MediaBuffers::create(&area) {
        Ok(buffers) => {
            let root = buffers.root().to_path_buf();
            RUN_MEDIA_BUFFERS.record(&root);
            (Some(buffers), root)
        }
        Err(error) => {
            eprintln!(
                "mercury-browser: could not create a private media directory in {}: {error}; \
                 media will play without download buffers",
                area.display()
            );
            (None, area.join("unavailable"))
        }
    };
    // SAFETY: called from `main` before any thread exists (see the call site).
    unsafe { apply(glib_user_directories(&root)) };
    buffers
}

/// This run's media directory while it exists, so that exit paths that skip destructors (the
/// quit watchdogs) can still remove it.
static RUN_MEDIA_BUFFERS: RunDirectory = RunDirectory::new();

/// Deletes this run's media directory, if it has one. Safe to call more than once; for exits
/// that skip destructors.
pub(crate) fn remove_media_buffers() {
    if let Some(root) = RUN_MEDIA_BUFFERS.take() {
        remove_run_directory(&root);
    }
}

/// A directory path that can be taken out once, by whichever exit path comes first.
pub(crate) struct RunDirectory(std::sync::Mutex<Option<PathBuf>>);

impl RunDirectory {
    pub(crate) const fn new() -> Self {
        Self(std::sync::Mutex::new(None))
    }

    pub(crate) fn record(&self, root: &Path) {
        if let Ok(mut current) = self.0.lock() {
            *current = Some(root.to_path_buf());
        }
    }

    /// Takes the recorded path if it is `root`; true if it was.
    pub(crate) fn take_if(&self, root: &Path) -> bool {
        self.0.lock().is_ok_and(|mut current| {
            let matches = current.as_deref() == Some(root);
            if matches {
                *current = None;
            }
            matches
        })
    }

    pub(crate) fn take(&self) -> Option<PathBuf> {
        self.0.lock().ok().and_then(|mut current| current.take())
    }
}

/// The directory of one run, locked for as long as the run lasts and removed when dropped.
pub(crate) struct MediaBuffers {
    root: PathBuf,
    _lock: ProfileLock,
}

impl MediaBuffers {
    /// Creates a run directory in `area` (created 0700 if needed): `<process id>-<nanoseconds>`,
    /// mode 0700, holding `lock` and one 0700 subdirectory per GLib user directory. It is built
    /// under a staging name and renamed only once its lock is held, so a sweep never finds a
    /// run directory without a lock unless that run is being removed.
    pub(crate) fn create(area: &Path) -> io::Result<Self> {
        prepare_area(area)?;
        let nanoseconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let name = format!("{}-{nanoseconds}", std::process::id());
        let staging = area.join(format!("{STAGING_PREFIX}{name}"));
        private_directory().create(&staging)?;
        let built = (|| {
            for (_, child) in GLIB_USER_DIRECTORIES {
                private_directory().create(staging.join(child))?;
            }
            let lock = ProfileLock::acquire(&staging).map_err(|error| match error {
                LockError::Io(error) => error,
                LockError::InUse => io::Error::other("a new media directory was locked"),
            })?;
            let root = area.join(&name);
            std::fs::rename(&staging, &root)?;
            Ok(Self { root, _lock: lock })
        })();
        if built.is_err() {
            let _ = std::fs::remove_dir_all(&staging);
        }
        built
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }
}

impl Drop for MediaBuffers {
    fn drop(&mut self) {
        // The lock is released after this, when the fields drop.
        let _ = RUN_MEDIA_BUFFERS.take_if(&self.root);
        remove_run_directory(&self.root);
    }
}

fn remove_run_directory(root: &Path) {
    match std::fs::remove_dir_all(root) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => eprintln!(
            "mercury-browser: could not remove the media directory {}: {error}",
            root.display()
        ),
    }
}

fn private_directory() -> DirBuilder {
    let mut builder = DirBuilder::new();
    builder.mode(0o700);
    builder
}

fn current_user() -> u32 {
    // SAFETY: `geteuid` has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

/// Creates `area` if needed and makes sure it is a real directory of this user, mode 0700.
fn prepare_area(area: &Path) -> io::Result<()> {
    std::fs::create_dir_all(area)?;
    let metadata = std::fs::symlink_metadata(area)?;
    if !metadata.file_type().is_dir() || metadata.uid() != current_user() {
        return Err(io::Error::other(
            "not a directory of this user (or a symbolic link)",
        ));
    }
    if metadata.mode() & 0o777 != 0o700 {
        std::fs::set_permissions(area, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Whether `name` is `<digits>-<digits>`, as `MediaBuffers::create` names run directories.
fn is_run_name(name: &str) -> bool {
    let all_digits =
        |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    name.split_once('-')
        .is_some_and(|(process_id, nanoseconds)| all_digits(process_id) && all_digits(nanoseconds))
}

/// A real directory of this user with mode 0700 (never a symbolic link).
fn is_private_directory(path: &Path) -> Option<std::fs::Metadata> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    (metadata.file_type().is_dir()
        && metadata.uid() == current_user()
        && metadata.mode() & 0o777 == 0o700)
        .then_some(metadata)
}

/// Removes leftovers in `area` of runs that ended without removing them. Returns how many.
///
/// A run directory (`<digits>-<digits>`, a private directory of this user) is removed if its
/// `lock` can be locked without waiting (its run is gone; a live run holds it) or is missing (a
/// removal stopped half-way). A staging directory (`.new-<digits>-<digits>`) is removed once it
/// is `staging_age` old. Anything else, including symbolic links, is left alone, and failures
/// are skipped. Process ids in names are not trusted; the lock decides.
pub(crate) fn sweep(area: &Path, now: SystemTime, staging_age: Duration) -> usize {
    let Ok(entries) = std::fs::read_dir(area) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.take(MAX_SWEEP_ENTRIES).flatten() {
        let path = entry.path();
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let remove = if is_run_name(&name) {
            is_private_directory(&path).is_some() && is_unlocked(&path)
        } else if let Some(run_name) = name.strip_prefix(STAGING_PREFIX) {
            is_run_name(run_name)
                && is_private_directory(&path)
                    .and_then(|metadata| metadata.modified().ok())
                    .and_then(|modified| now.duration_since(modified).ok())
                    .is_some_and(|age| age >= staging_age)
        } else {
            false
        };
        if remove && std::fs::remove_dir_all(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// Whether no live run holds the directory's lock. The test lock is released again when this
/// returns, before the caller removes the directory; that is safe because a run only ever locks
/// the directory it has just created, never an existing one.
fn is_unlocked(root: &Path) -> bool {
    match OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(root.join("lock"))
    {
        Ok(lock_file) => lock_file.try_lock().is_ok(),
        Err(error) => error.kind() == io::ErrorKind::NotFound,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_run_directory_is_given_once() {
        let directory = super::RunDirectory::new();
        directory.record(std::path::Path::new("/a/run"));

        assert_eq!(directory.take(), Some(std::path::PathBuf::from("/a/run")));
        assert_eq!(directory.take(), None);
    }

    #[test]
    fn a_run_directory_is_taken_only_by_its_own_owner() {
        let directory = super::RunDirectory::new();
        directory.record(std::path::Path::new("/a/run"));

        assert!(!directory.take_if(std::path::Path::new("/another/run")));
        assert!(directory.take_if(std::path::Path::new("/a/run")));
        assert_eq!(directory.take(), None);
    }

    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn scratch(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "mercury-media-buffers-test-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn a_run_directory_is_private_locked_and_removed_when_dropped() {
        let area = scratch("create").join("media-buffers");
        let buffers = MediaBuffers::create(&area).unwrap();
        let root = buffers.root().to_path_buf();
        assert_eq!(root.parent(), Some(area.as_path()));
        assert_eq!(mode(&area), 0o700);
        assert_eq!(mode(&root), 0o700);
        for child in ["cache", "data", "config", "runtime"] {
            assert_eq!(mode(&root.join(child)), 0o700, "{child}");
        }
        assert!(matches!(
            ProfileLock::acquire(&root),
            Err(crate::profile::LockError::InUse)
        ));
        std::fs::write(root.join("cache").join("<unknown>-ABCDEF"), b"media").unwrap();
        drop(buffers);
        assert!(!root.exists());
        std::fs::remove_dir_all(area.parent().unwrap()).unwrap();
    }

    #[test]
    fn two_runs_coexist_and_a_sweep_leaves_live_runs_alone() {
        let area = scratch("coexist");
        let first = MediaBuffers::create(&area).unwrap();
        let second = MediaBuffers::create(&area).unwrap();
        assert_ne!(first.root(), second.root());
        assert_eq!(sweep(&area, SystemTime::now(), Duration::ZERO), 0);
        assert!(first.root().exists() && second.root().exists());
        drop((first, second));
        std::fs::remove_dir_all(&area).unwrap();
    }

    #[test]
    fn a_sweep_removes_what_killed_runs_left_and_nothing_else() {
        let area = scratch("sweep");
        // A killed run: its directory is complete but nobody holds its lock.
        let killed = area.join("123-456");
        std::fs::create_dir(&killed).unwrap();
        std::fs::set_permissions(&killed, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(killed.join("lock"), b"").unwrap();
        std::fs::create_dir(killed.join("cache")).unwrap();
        std::fs::write(killed.join("cache").join("<unknown>-QWERTY"), b"media").unwrap();
        // A run that stopped while being removed: the lock file is already gone.
        let half_removed = area.join("124-457");
        std::fs::create_dir(&half_removed).unwrap();
        std::fs::set_permissions(&half_removed, std::fs::Permissions::from_mode(0o700)).unwrap();
        // Things that are not run directories.
        let foreign = area.join("photos");
        std::fs::create_dir(&foreign).unwrap();
        let open_to_others = area.join("125-458");
        std::fs::create_dir(&open_to_others).unwrap();
        std::fs::set_permissions(&open_to_others, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink(&foreign, area.join("126-459")).unwrap();
        // A staging directory: removed only once it is old.
        let staging = area.join(".new-127-460");
        std::fs::create_dir(&staging).unwrap();
        std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o700)).unwrap();

        let now = SystemTime::now();
        assert_eq!(sweep(&area, now, Duration::from_secs(600)), 2);
        assert!(!killed.exists() && !half_removed.exists());
        assert!(foreign.exists() && open_to_others.exists() && staging.exists());
        assert!(std::fs::symlink_metadata(area.join("126-459")).is_ok());

        assert_eq!(
            sweep(
                &area,
                now + Duration::from_secs(601),
                Duration::from_secs(600)
            ),
            1
        );
        assert!(!staging.exists());
        std::fs::remove_dir_all(&area).unwrap();
    }
}
