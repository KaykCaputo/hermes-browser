use std::fs::{DirBuilder, File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(target_os = "macos")]
pub(crate) const APP_DIR_NAME: &str = "Mercury Browser";
#[cfg(not(target_os = "macos"))]
pub(crate) const APP_DIR_NAME: &str = "mercury-browser";

const DEFAULT_PROFILE_NAME: &str = "Default";
/// Mercury's directory inside the platform's cache directory (`~/Library/Caches` on macOS,
/// named by the bundle identifier as in `media_isolation`; `~/.cache` on Linux).
#[cfg(target_os = "macos")]
const APP_CACHE_DIR_NAME: &str = "io.github.thepigdestroyer.mercury-browser";
#[cfg(not(target_os = "macos"))]
const APP_CACHE_DIR_NAME: &str = "mercury-browser";
const HTTP_CACHE_DIRECTORY_NAME: &str = "http-cache";
const LOCK_FILE_NAME: &str = "lock";
const SERVO_DIRECTORY_NAME: &str = "servo";
/// The file that marks a directory as a Mercury Browser profile, and its first line.
const PROFILE_MARKER_NAME: &str = ".mercury-browser-profile";
const PROFILE_MARKER_FIRST_LINE: &str = "Mercury Browser profile";

/// Exit status when the profile is already in use by another Mercury Browser.
pub(crate) const EXIT_PROFILE_IN_USE: u8 = 4;

/// The profile is locked by another running Mercury Browser.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ProfileInUse {
    pub root: PathBuf,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ProfilePaths {
    pub root: PathBuf,
    pub servo_config_dir: PathBuf,
    pub places_database: PathBuf,
}

impl ProfilePaths {
    /// `<data_dir>/<app dir>/Default`, or `override_root` when given. Does not touch the disk.
    pub(crate) fn resolve(data_dir: Option<&Path>, override_root: Option<&Path>) -> Option<Self> {
        let root = match override_root {
            Some(root) => root.to_path_buf(),
            None => data_dir?.join(APP_DIR_NAME).join(DEFAULT_PROFILE_NAME),
        };
        Some(Self {
            servo_config_dir: root.join(SERVO_DIRECTORY_NAME),
            places_database: root.join("places.sqlite"),
            root,
        })
    }

    /// Creates the profile, or checks and reopens an existing one, readable by this user only:
    /// it holds cookies and, while the browser runs, HTTP credentials.
    ///
    /// A directory is an existing profile only if it holds `PROFILE_MARKER_NAME`, a regular file
    /// owned by this user whose first line is `PROFILE_MARKER_FIRST_LINE`; only then, or when
    /// this code has just created the directory, is any mode changed. Any other existing
    /// directory is used only if it is empty, private and this user's (the marker is then
    /// written); a directory holding other things, or open to others, or a symbolic link, is
    /// refused and left exactly as it is.
    pub(crate) fn create(&self) -> io::Result<()> {
        let root = match std::fs::symlink_metadata(&self.root) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                create_private_directory(&self.root)?;
                RootState::Created
            }
            Err(error) => return Err(error),
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(io::Error::other(
                    "it is a symbolic link; Mercury does not follow links to a profile \
                     (give the directory itself)",
                ));
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(io::Error::other("it is not a directory"));
            }
            Ok(_) if is_recognised_profile(&self.root) => RootState::Profile,
            Ok(metadata) => {
                if std::fs::read_dir(&self.root)?.next().is_some() {
                    return Err(io::Error::other(
                        "it already holds other files and is not a Mercury profile; a profile \
                         goes in a new or empty directory",
                    ));
                }
                if !is_owned_by_current_user(&metadata) {
                    return Err(io::Error::other("it belongs to another user"));
                }
                if is_open_to_others(&metadata) {
                    return Err(io::Error::other(
                        "other users can read it and it is not a Mercury profile; Mercury does \
                         not change its permissions (choose a directory that does not exist yet, \
                         or make it private with chmod 700)",
                    ));
                }
                RootState::Empty
            }
        };

        let mut tightened = Vec::new();
        if root == RootState::Profile && tighten_directory(&self.root)? {
            tightened.push(self.root.display().to_string());
        }
        match std::fs::symlink_metadata(&self.servo_config_dir) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                create_private_directory(&self.servo_config_dir)?;
            }
            Err(error) => return Err(error),
            // Only a recognised profile's own `servo` directory is adopted.
            Ok(metadata)
                if root == RootState::Profile
                    && metadata.is_dir()
                    && is_owned_by_current_user(&metadata) =>
            {
                if tighten_directory(&self.servo_config_dir)? {
                    tightened.push(self.servo_config_dir.display().to_string());
                }
            }
            Ok(_) => {
                return Err(io::Error::other(
                    "its `servo` entry is not a directory this profile owns",
                ));
            }
        }
        if root != RootState::Profile {
            write_profile_marker(&self.root)?;
        }
        if !tightened.is_empty() {
            eprintln!(
                "mercury-browser: restricted the profile to this user (it was readable by others): {}",
                tightened.join(", ")
            );
        }
        Ok(())
    }
}

/// How `ProfilePaths::create` found the profile root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RootState {
    /// Created just now.
    Created,
    /// Existed, empty, private and this user's.
    Empty,
    /// Existed and carries the profile marker.
    Profile,
}

/// Whether `root` holds the profile marker: a regular file (not a link) owned by this user
/// whose first line is `PROFILE_MARKER_FIRST_LINE`.
fn is_recognised_profile(root: &Path) -> bool {
    use std::io::Read;

    let Ok(file) = open_without_following(&root.join(PROFILE_MARKER_NAME), false) else {
        return false;
    };
    let Ok(metadata) = file.metadata() else {
        return false;
    };
    if !metadata.is_file() || !is_owned_by_current_user(&metadata) {
        return false;
    }
    let mut start = Vec::new();
    if file.take(256).read_to_end(&mut start).is_err() {
        return false;
    }
    String::from_utf8_lossy(&start).lines().next() == Some(PROFILE_MARKER_FIRST_LINE)
}

fn write_profile_marker(root: &Path) -> io::Result<()> {
    use std::io::Write;

    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut marker = options.open(root.join(PROFILE_MARKER_NAME))?;
    writeln!(marker, "{PROFILE_MARKER_FIRST_LINE}")?;
    writeln!(
        marker,
        "This directory is a Mercury Browser profile: cookies and site data. Do not share it."
    )
}

/// Opens `path` without following a symbolic link at its last component.
fn open_without_following(path: &Path, directory: bool) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        let flags = if directory {
            libc::O_NOFOLLOW | libc::O_DIRECTORY
        } else {
            libc::O_NOFOLLOW
        };
        std::os::unix::fs::OpenOptionsExt::custom_flags(&mut options, flags);
    }
    #[cfg(not(unix))]
    let _ = directory;
    options.open(path)
}

/// Removes group and other permissions from a directory of this user, through an open handle
/// so that nothing can be swapped in between the check and the change. Returns whether it
/// changed anything.
fn tighten_directory(path: &Path) -> io::Result<bool> {
    let directory = open_without_following(path, true)?;
    let metadata = directory.metadata()?;
    if !metadata.is_dir() || !is_owned_by_current_user(&metadata) {
        return Err(io::Error::other(format!(
            "{} is not a directory of this user",
            path.display()
        )));
    }
    if !is_open_to_others(&metadata) {
        return Ok(false);
    }
    make_private(&directory, &metadata)?;
    Ok(true)
}

/// Creates `path` and any missing parents with mode 0700 on Unix, set at creation.
fn create_private_directory(path: &Path) -> io::Result<()> {
    let mut builder = DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(path)
}

#[cfg(unix)]
fn is_open_to_others(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;

    metadata.permissions().mode() & 0o077 != 0
}

#[cfg(not(unix))]
fn is_open_to_others(_metadata: &std::fs::Metadata) -> bool {
    false
}

#[cfg(unix)]
fn is_owned_by_current_user(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;

    metadata.uid() == current_user()
}

#[cfg(not(unix))]
fn is_owned_by_current_user(_metadata: &std::fs::Metadata) -> bool {
    true
}

#[cfg(unix)]
fn make_private(directory: &File, metadata: &std::fs::Metadata) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    // `File::set_permissions` is `fchmod` on the open handle.
    let mode = metadata.permissions().mode();
    directory.set_permissions(std::fs::Permissions::from_mode(mode & 0o700))
}

#[cfg(not(unix))]
fn make_private(_directory: &File, _metadata: &std::fs::Metadata) -> io::Result<()> {
    Ok(())
}

/// Fails unless a file can be created and removed in `directory`.
fn check_writable(directory: &Path) -> io::Result<()> {
    let probe = directory.join(format!(".write-probe-{}", std::process::id()));
    File::create_new(&probe)?;
    std::fs::remove_file(&probe)
}

/// A relative `--profile-dir` is taken relative to the current directory, once, at start-up.
fn absolute_profile_root(root: &Path) -> io::Result<PathBuf> {
    std::path::absolute(root)
}

/// Where a profile's HTTP disk cache goes. The user's default profile keeps it with the
/// platform's caches (`~/Library/Caches/<bundle id>/Default/http-cache`), where backups skip
/// it; a profile given with `--profile-dir`, a temporary profile, or a default profile on a
/// system with no cache directory keeps it inside the profile (`<profile>/http-cache`), so a
/// temporary profile's cache is removed with it.
fn http_cache_dir(root: &Path, default_profile_cache_root: Option<&Path>) -> PathBuf {
    match default_profile_cache_root {
        Some(cache_root) => cache_root
            .join(APP_CACHE_DIR_NAME)
            .join(DEFAULT_PROFILE_NAME)
            .join(HTTP_CACHE_DIRECTORY_NAME),
        None => root.join(HTTP_CACHE_DIRECTORY_NAME),
    }
}

/// Creates the HTTP cache directory (mode 0700), or checks an existing one: a real directory
/// of this user, made private if it was not, and writable. A symbolic link is refused.
fn prepare_http_cache_dir(directory: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(directory) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            create_private_directory(directory)?;
        }
        Err(error) => return Err(error),
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(io::Error::other(
                "it is not a directory (or is a symbolic link)",
            ));
        }
        Ok(_) => {
            tighten_directory(directory)?;
        }
    }
    check_writable(directory)
}

/// The profile this run uses. Keep it alive until the engine has shut down: it holds the
/// profile's lock, and a temporary profile is deleted when it is dropped.
pub(crate) struct ActiveProfile {
    pub paths: Option<ProfilePaths>,
    /// The HTTP disk cache's directory, if it could be prepared.
    http_cache_dir: Option<PathBuf>,
    _lock: Option<ProfileLock>,
    _temporary: Option<TemporaryProfile>,
}

impl ActiveProfile {
    /// Where Servo should keep its data, if anywhere.
    pub(crate) fn servo_config_dir(&self) -> Option<PathBuf> {
        self.paths
            .as_ref()
            .map(|paths| paths.servo_config_dir.clone())
    }

    /// Where Servo should keep its HTTP disk cache, if anywhere.
    pub(crate) fn http_cache_dir(&self) -> Option<PathBuf> {
        self.http_cache_dir.clone()
    }
}

/// Chooses, creates and locks the profile: `override_root` when given; otherwise a fresh
/// temporary directory for an automated run, so those never touch the user's data; otherwise
/// the user's default profile. If it cannot be created or written, the run keeps no site data.
/// If another Mercury Browser holds it, the run must not start.
pub(crate) fn open_profile(
    override_root: Option<&Path>,
    automated_run: bool,
    temporary_area: &TemporaryProfileArea,
) -> Result<ActiveProfile, ProfileInUse> {
    // Leftovers of killed runs hold their cookies; remove them before anything else.
    let swept = sweep_stale_temporary_profiles(&temporary_area.directory, &temporary_area.sweep);
    if swept > 0 {
        eprintln!("mercury-browser: removed {swept} temporary profile(s) left by earlier runs");
    }
    let mut temporary = None;
    let mut default_profile_cache_root = None;
    // Servo's logger is not installed yet, so errors go to standard error directly.
    let paths = if let Some(root) = override_root {
        match absolute_profile_root(root) {
            Ok(root) => ProfilePaths::resolve(None, Some(&root)),
            Err(error) => {
                report_unusable(root, &error);
                None
            }
        }
    } else if automated_run {
        match TemporaryProfile::create(&temporary_area.directory) {
            Ok(profile) => {
                let paths = ProfilePaths::resolve(None, Some(profile.root()));
                temporary = Some(profile);
                paths
            }
            Err(error) => {
                eprintln!("mercury-browser: could not create a temporary profile: {error}");
                None
            }
        }
    } else {
        let paths = ProfilePaths::resolve(dirs::data_dir().as_deref(), None);
        default_profile_cache_root = dirs::cache_dir();
        if paths.is_none() {
            eprintln!(
                "mercury-browser: no data directory is known for this user; site data will not be kept"
            );
        }
        paths
    };

    let paths = paths.and_then(|paths| match paths.create() {
        Ok(()) => Some(paths),
        Err(error) => {
            report_unusable(&paths.root, &error);
            None
        }
    });

    // A temporary profile is unique to this run and needs no lock.
    let mut lock = None;
    let paths = match paths {
        Some(paths) if temporary.is_none() => match ProfileLock::acquire(&paths.root) {
            Ok(acquired) => {
                lock = Some(acquired);
                Some(paths)
            }
            Err(LockError::InUse) => return Err(ProfileInUse { root: paths.root }),
            Err(LockError::Io(error)) => {
                report_unusable(&paths.root, &error);
                None
            }
        },
        paths => paths,
    };

    let paths = paths.and_then(|paths| match check_writable(&paths.servo_config_dir) {
        Ok(()) => Some(paths),
        Err(error) => {
            report_unusable(&paths.servo_config_dir, &error);
            None
        }
    });

    let http_cache_dir = paths.as_ref().and_then(|paths| {
        let directory = http_cache_dir(&paths.root, default_profile_cache_root.as_deref());
        match prepare_http_cache_dir(&directory) {
            Ok(()) => Some(directory),
            Err(error) => {
                eprintln!(
                    "mercury-browser: could not use the HTTP cache directory {}: {error}; \
                     pages will be cached in memory only",
                    directory.display()
                );
                None
            }
        }
    });

    Ok(ActiveProfile {
        paths,
        http_cache_dir,
        _lock: lock,
        _temporary: temporary,
    })
}

fn report_unusable(path: &Path, error: &io::Error) {
    eprintln!(
        "mercury-browser: could not use the profile at {}: {error}; site data will not be kept",
        path.display()
    );
}

/// An exclusive advisory lock on `<profile>/lock`, held for the life of the run. Servo reads
/// the profile's files once at start and rewrites them whole at exit, so two browsers on one
/// profile would overwrite each other's data.
pub(crate) struct ProfileLock {
    _file: File,
}

#[derive(Debug)]
pub(crate) enum LockError {
    InUse,
    Io(io::Error),
}

impl ProfileLock {
    pub(crate) fn acquire(root: &Path) -> Result<Self, LockError> {
        let mut options = OpenOptions::new();
        options.create(true).truncate(false).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let file = options
            .open(root.join(LOCK_FILE_NAME))
            .map_err(LockError::Io)?;
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file }),
            Err(TryLockError::WouldBlock) => Err(LockError::InUse),
            Err(TryLockError::Error(error)) => Err(LockError::Io(error)),
        }
    }
}

/// The temporary profile's root while one exists, so that exit paths that skip destructors
/// can still remove it.
static TEMPORARY_PROFILE_ROOT: Mutex<Option<PathBuf>> = Mutex::new(None);

/// A private profile directory under the system's temporary directory, removed when dropped.
struct TemporaryProfile {
    root: PathBuf,
    /// Held for the profile's lifetime, so a later run's sweep can tell it is in use; released
    /// only after the directory has been removed (fields drop after `Drop::drop`).
    _lock: ProfileLock,
}

impl TemporaryProfile {
    fn create(directory: &Path) -> io::Result<Self> {
        let nanoseconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let root = directory.join(format!(
            "{TEMPORARY_PROFILE_PREFIX}{}-{nanoseconds}",
            std::process::id()
        ));
        let mut builder = DirBuilder::new();
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        builder.create(&root)?;
        if let Err(error) = write_profile_marker(&root) {
            remove_directory(&root);
            return Err(error);
        }
        let lock = match ProfileLock::acquire(&root) {
            Ok(lock) => lock,
            Err(error) => {
                remove_directory(&root);
                return Err(match error {
                    LockError::Io(error) => error,
                    LockError::InUse => io::Error::other("a new temporary profile was locked"),
                });
            }
        };
        if let Ok(mut current) = TEMPORARY_PROFILE_ROOT.lock() {
            *current = Some(root.clone());
        }
        Ok(Self { root, _lock: lock })
    }

    fn root(&self) -> &Path {
        &self.root
    }
}

impl Drop for TemporaryProfile {
    fn drop(&mut self) {
        if let Ok(mut current) = TEMPORARY_PROFILE_ROOT.lock()
            && current.as_ref() == Some(&self.root)
        {
            *current = None;
        }
        remove_directory(&self.root);
    }
}

/// Name prefix of temporary profiles; the full name is `<prefix><process id>-<nanoseconds>`.
const TEMPORARY_PROFILE_PREFIX: &str = "mercury-browser-profile-";
/// Leftover temporary profiles younger than this are never swept, so a run that has just
/// created its directory and not yet locked it is safe.
const STALE_TEMPORARY_PROFILE_AGE: Duration = Duration::from_secs(10 * 60);
/// At most this many entries of the temporary directory are examined per start-up.
const MAX_SWEEP_ENTRIES: usize = 4096;
/// Overrides `STALE_TEMPORARY_PROFILE_AGE`, in seconds (used to test the sweep without waiting).
const STALE_AGE_VARIABLE: &str = "MERCURY_STALE_PROFILE_AGE_SECONDS";

/// Where temporary profiles are created, and the rules for sweeping their leftovers there.
#[derive(Clone, Debug)]
pub(crate) struct TemporaryProfileArea {
    pub directory: PathBuf,
    pub sweep: SweepRules,
}

impl TemporaryProfileArea {
    /// The system temporary directory, with this run's sweep rules.
    pub(crate) fn for_this_run() -> Self {
        Self {
            directory: std::env::temp_dir(),
            sweep: SweepRules::for_this_run(),
        }
    }
}

/// What makes a leftover temporary profile safe to remove.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SweepRules {
    pub now: SystemTime,
    pub min_age: Duration,
    /// The user whose leftovers may be removed.
    pub owner: u32,
    pub max_entries: usize,
}

/// Whether `name` is exactly what `TemporaryProfile::create` generates.
fn is_temporary_profile_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix(TEMPORARY_PROFILE_PREFIX) else {
        return false;
    };
    let Some((process_id, nanoseconds)) = rest.split_once('-') else {
        return false;
    };
    let all_digits =
        |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    all_digits(process_id) && all_digits(nanoseconds)
}

/// Removes temporary profiles left behind by runs that were killed. Returns how many.
///
/// An entry of `directory` is removed only if all of these hold: its name is exactly one this
/// code generates; it is a real directory (not a symbolic link) owned by `rules.owner` with mode
/// 0700; it was last modified at least `rules.min_age` before `rules.now`; and no live run owns
/// it: an exclusive lock on its `lock` file can be taken without waiting, or that file is gone
/// (a removal that stopped after everything else; the lock file is always removed last). A
/// `lock` entry that is a symbolic link or cannot be opened keeps the leftover. Process ids in names
/// are not trusted: they repeat, and are not unique across containers sharing the directory.
/// At most `rules.max_entries` entries are examined. Failures are skipped, never fatal.
#[cfg(unix)]
pub(crate) fn sweep_stale_temporary_profiles(directory: &Path, rules: &SweepRules) -> usize {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return 0;
    };
    entries
        .take(rules.max_entries)
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(is_temporary_profile_name)
        })
        .filter(|entry| remove_if_stale(&entry.path(), rules))
        .count()
}

#[cfg(not(unix))]
pub(crate) fn sweep_stale_temporary_profiles(_directory: &Path, _rules: &SweepRules) -> usize {
    0
}

#[cfg(unix)]
fn remove_if_stale(root: &Path, rules: &SweepRules) -> bool {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    let Ok(metadata) = std::fs::symlink_metadata(root) else {
        return false;
    };
    let old_enough = metadata
        .modified()
        .ok()
        .and_then(|modified| rules.now.duration_since(modified).ok())
        .is_some_and(|age| age >= rules.min_age);
    if !metadata.file_type().is_dir()
        || metadata.uid() != rules.owner
        || metadata.mode() & 0o777 != 0o700
        || !old_enough
    {
        return false;
    }
    // Opening never creates the file and never follows a symbolic link. A missing lock file
    // means an earlier removal got that far (it removes the lock file last): nothing holds it.
    let lock_file = match OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(root.join(LOCK_FILE_NAME))
    {
        Ok(lock_file) => {
            if lock_file.try_lock().is_err() {
                return false;
            }
            Some(lock_file)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(_) => return false,
    };
    // The lock, if any, is held until the directory is gone.
    let removed = remove_temporary_profile_directory(root).is_ok();
    drop(lock_file);
    removed
}

impl SweepRules {
    /// The rules for this run: the current user, now, and the stated age (or its override).
    pub(crate) fn for_this_run() -> Self {
        let min_age = std::env::var(STALE_AGE_VARIABLE)
            .ok()
            .and_then(|seconds| seconds.parse::<u64>().ok())
            .map_or(STALE_TEMPORARY_PROFILE_AGE, Duration::from_secs);
        Self {
            now: SystemTime::now(),
            min_age,
            owner: current_user(),
            max_entries: MAX_SWEEP_ENTRIES,
        }
    }
}

#[cfg(unix)]
fn current_user() -> u32 {
    // SAFETY: `geteuid` has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

#[cfg(not(unix))]
fn current_user() -> u32 {
    0
}

/// Deletes the temporary profile, if this run created one. Safe to call more than once; call
/// it before `std::process::exit`, which skips destructors.
pub(crate) fn remove_temporary_profile() {
    let root = TEMPORARY_PROFILE_ROOT
        .lock()
        .ok()
        .and_then(|mut current| current.take());
    if let Some(root) = root {
        remove_directory(&root);
    }
}

/// Removes a temporary profile directory, its `lock` file last: if the removal stops part-way
/// (the shutdown watchdog's exit, a crash), what is left either still has its lock file, or
/// has none and is recognised by the sweep as unlocked. Symbolic links are removed, never
/// followed (`DirEntry::file_type` and `remove_dir_all` do not follow them).
fn remove_temporary_profile_directory(root: &Path) -> io::Result<()> {
    let mut first_error = None;
    for entry in std::fs::read_dir(root)? {
        let result = entry.and_then(|entry| {
            if entry.file_name() == LOCK_FILE_NAME {
                return Ok(());
            }
            if entry.file_type()?.is_dir() {
                std::fs::remove_dir_all(entry.path())
            } else {
                std::fs::remove_file(entry.path())
            }
        });
        if let Err(error) = result {
            first_error.get_or_insert(error);
        }
    }
    if let Some(error) = first_error {
        return Err(error);
    }
    match std::fs::remove_file(root.join(LOCK_FILE_NAME)) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    std::fs::remove_dir(root)
}

fn remove_directory(root: &Path) {
    match remove_temporary_profile_directory(root) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => eprintln!(
            "mercury-browser: could not remove the temporary profile at {}: {error}",
            root.display()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_temporary_directory(label: &str) -> PathBuf {
        let nanoseconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        std::env::temp_dir().join(format!(
            "mercury-profile-test-{label}-{}-{nanoseconds}",
            std::process::id()
        ))
    }

    /// A temporary-profile area of the test's own: nothing outside it is swept or created.
    struct TestArea(TemporaryProfileArea);

    impl TestArea {
        fn new(label: &str) -> Self {
            let directory = unique_temporary_directory(&format!("area-{label}"));
            std::fs::create_dir_all(&directory).expect("the test area");
            Self(TemporaryProfileArea {
                directory,
                sweep: SweepRules::for_this_run(),
            })
        }

        fn get(&self) -> &TemporaryProfileArea {
            &self.0
        }
    }

    impl Drop for TestArea {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0.directory);
        }
    }

    #[test]
    fn resolves_under_the_data_directory() {
        let paths = ProfilePaths::resolve(Some(Path::new("/data")), None)
            .expect("a data directory should give a profile");

        let root = Path::new("/data").join(APP_DIR_NAME).join("Default");
        assert_eq!(
            paths,
            ProfilePaths {
                servo_config_dir: root.join("servo"),
                places_database: root.join("places.sqlite"),
                root,
            }
        );
    }

    #[test]
    fn override_root_wins() {
        let paths = ProfilePaths::resolve(Some(Path::new("/data")), Some(Path::new("/x")))
            .expect("an override should give a profile");

        assert_eq!(paths.root, Path::new("/x"));
        assert_eq!(paths.servo_config_dir, Path::new("/x/servo"));
        assert_eq!(paths.places_database, Path::new("/x/places.sqlite"));
    }

    #[test]
    fn no_data_directory_and_no_override_is_none() {
        assert_eq!(ProfilePaths::resolve(None, None), None);
    }

    #[test]
    fn create_makes_both_directories() {
        let root = unique_temporary_directory("create");
        let paths = ProfilePaths::resolve(None, Some(&root)).expect("an override is enough");

        let result = paths.create();
        let created = paths.root.is_dir() && paths.servo_config_dir.is_dir();
        let _ = std::fs::remove_dir_all(&root);

        result.expect("the profile directories should be created");
        assert!(created);
    }

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;

        std::fs::metadata(path)
            .expect("the directory should exist")
            .permissions()
            .mode()
            & 0o777
    }

    #[cfg(unix)]
    #[test]
    fn create_makes_both_directories_private() {
        let root = unique_temporary_directory("private");
        let paths = ProfilePaths::resolve(None, Some(&root)).expect("an override is enough");

        let result = paths.create();
        let modes = (mode(&paths.root), mode(&paths.servo_config_dir));
        let _ = std::fs::remove_dir_all(&root);

        result.expect("the profile directories should be created");
        assert_eq!(modes, (0o700, 0o700));
    }

    #[cfg(unix)]
    fn set_mode(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;

        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
            .expect("the test permissions");
    }

    #[cfg(unix)]
    #[test]
    fn a_shared_directory_that_is_not_a_profile_is_refused_and_left_alone() {
        let root = unique_temporary_directory("shared");
        std::fs::create_dir_all(&root).expect("the test directory");
        set_mode(&root, 0o755);
        let paths = ProfilePaths::resolve(None, Some(&root)).expect("an override is enough");

        let result = paths.create();
        let mode_after = mode(&root);
        let servo_created = paths.servo_config_dir.exists();
        let _ = std::fs::remove_dir_all(&root);

        assert!(result.is_err());
        assert_eq!(mode_after, 0o755);
        assert!(!servo_created);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_servo_directory_is_never_changed() {
        let base = unique_temporary_directory("servo-symlink");
        let root = base.join("profile");
        let elsewhere = base.join("elsewhere");
        std::fs::create_dir_all(&root).expect("the test directories");
        std::fs::create_dir_all(&elsewhere).expect("the test directories");
        set_mode(&root, 0o700);
        set_mode(&elsewhere, 0o755);
        std::os::unix::fs::symlink(&elsewhere, root.join("servo")).expect("the test symlink");
        let paths = ProfilePaths::resolve(None, Some(&root)).expect("an override is enough");

        let result = paths.create();
        let elsewhere_mode = mode(&elsewhere);
        let _ = std::fs::remove_dir_all(&base);

        assert!(result.is_err());
        assert_eq!(elsewhere_mode, 0o755);
    }

    /// Every entry under `root` (itself included) with its mode, sorted, symlinks not followed.
    #[cfg(unix)]
    fn snapshot(root: &Path) -> Vec<(PathBuf, u32)> {
        use std::os::unix::fs::PermissionsExt;

        let mut entries = Vec::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(path) = pending.pop() {
            let metadata = std::fs::symlink_metadata(&path).expect("an entry");
            if metadata.is_dir() {
                for entry in std::fs::read_dir(&path).expect("a directory") {
                    pending.push(entry.expect("an entry").path());
                }
            }
            entries.push((path, metadata.permissions().mode() & 0o7777));
        }
        entries.sort();
        entries
    }

    fn marker_first_line(root: &Path) -> Option<String> {
        std::fs::read_to_string(root.join(PROFILE_MARKER_NAME))
            .ok()
            .and_then(|contents| contents.lines().next().map(str::to_owned))
    }

    #[cfg(unix)]
    #[test]
    fn a_directory_holding_an_unrelated_servo_directory_and_lock_file_is_refused_and_unchanged() {
        let area = TestArea::new("a_directory_holding_an_unrelated_servo_d");
        let root = unique_temporary_directory("unrelated");
        std::fs::create_dir_all(root.join("servo").join("components")).expect("a checkout");
        std::fs::write(root.join("servo").join("README.md"), "Servo").expect("a file");
        std::fs::write(root.join("lock"), "not ours").expect("a file");
        set_mode(&root.join("servo"), 0o755);
        set_mode(&root, 0o755);
        let before = snapshot(&root);

        let profile =
            open_profile(Some(&root), false, area.get()).expect("the directory is not locked");
        let used = profile.paths.is_some();
        drop(profile);
        let after = snapshot(&root);
        let _ = std::fs::remove_dir_all(&root);

        assert!(!used);
        assert_eq!(after, before);
    }

    #[cfg(unix)]
    #[test]
    fn a_private_directory_holding_other_things_is_refused() {
        let root = unique_temporary_directory("private-full");
        std::fs::create_dir_all(&root).expect("the test directory");
        std::fs::write(root.join("notes.txt"), "mine").expect("a file");
        set_mode(&root, 0o700);
        let before = snapshot(&root);
        let paths = ProfilePaths::resolve(None, Some(&root)).expect("an override is enough");

        let result = paths.create();
        let after = snapshot(&root);
        let _ = std::fs::remove_dir_all(&root);

        assert!(result.is_err());
        assert_eq!(after, before);
    }

    #[cfg(unix)]
    #[test]
    fn an_empty_existing_directory_is_used_and_marked() {
        let root = unique_temporary_directory("empty");
        std::fs::create_dir_all(&root).expect("the test directory");
        set_mode(&root, 0o700);
        let paths = ProfilePaths::resolve(None, Some(&root)).expect("an override is enough");

        let result = paths.create();
        let marker = marker_first_line(&root);
        let servo_mode = mode(&paths.servo_config_dir);
        let _ = std::fs::remove_dir_all(&root);

        result.expect("an empty private directory is a fine profile");
        assert_eq!(marker.as_deref(), Some(PROFILE_MARKER_FIRST_LINE));
        assert_eq!(servo_mode, 0o700);
    }

    #[cfg(unix)]
    #[test]
    fn a_new_profile_is_marked() {
        let root = unique_temporary_directory("new-marked");
        let paths = ProfilePaths::resolve(None, Some(&root)).expect("an override is enough");

        let result = paths.create();
        let marker = marker_first_line(&root);
        let _ = std::fs::remove_dir_all(&root);

        result.expect("a new directory is a fine profile");
        assert_eq!(marker.as_deref(), Some(PROFILE_MARKER_FIRST_LINE));
    }

    #[cfg(unix)]
    #[test]
    fn a_recognised_profile_with_a_too_open_mode_is_tightened() {
        let root = unique_temporary_directory("recognised");
        let paths = ProfilePaths::resolve(None, Some(&root)).expect("an override is enough");
        paths.create().expect("a new profile");
        set_mode(&root, 0o755);
        set_mode(&paths.servo_config_dir, 0o755);

        let result = paths.create();
        let modes = (mode(&root), mode(&paths.servo_config_dir));
        let _ = std::fs::remove_dir_all(&root);

        result.expect("an existing profile is accepted");
        assert_eq!(modes, (0o700, 0o700));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_to_a_profile_is_refused_and_the_profile_left_alone() {
        let base = unique_temporary_directory("symlink-profile");
        let target = base.join("profile");
        let paths = ProfilePaths::resolve(None, Some(&target)).expect("an override is enough");
        paths.create().expect("a real profile");
        set_mode(&target, 0o755);
        let link = base.join("link");
        std::os::unix::fs::symlink(&target, &link).expect("the test symlink");
        let before = snapshot(&target);

        let result = ProfilePaths::resolve(None, Some(&link))
            .expect("an override is enough")
            .create();
        let after = snapshot(&target);
        let _ = std::fs::remove_dir_all(&base);

        assert!(result.is_err());
        assert_eq!(after, before);
    }

    #[cfg(unix)]
    #[test]
    fn a_profile_whose_servo_entry_is_a_symlink_is_refused() {
        let base = unique_temporary_directory("servo-link");
        let root = base.join("profile");
        let paths = ProfilePaths::resolve(None, Some(&root)).expect("an override is enough");
        paths.create().expect("a real profile");
        std::fs::remove_dir(&paths.servo_config_dir).expect("the servo directory");
        let elsewhere = base.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).expect("the test directory");
        set_mode(&elsewhere, 0o755);
        std::os::unix::fs::symlink(&elsewhere, &paths.servo_config_dir).expect("the symlink");

        let result = paths.create();
        let elsewhere_mode = mode(&elsewhere);
        let _ = std::fs::remove_dir_all(&base);

        assert!(result.is_err());
        assert_eq!(elsewhere_mode, 0o755);
    }

    #[cfg(unix)]
    #[test]
    fn a_marker_with_other_contents_is_not_recognised() {
        let root = unique_temporary_directory("bad-marker");
        std::fs::create_dir_all(root.join("servo")).expect("the test directories");
        std::fs::write(root.join(PROFILE_MARKER_NAME), "something else\n").expect("a file");
        set_mode(&root, 0o755);
        let before = snapshot(&root);
        let paths = ProfilePaths::resolve(None, Some(&root)).expect("an override is enough");

        let result = paths.create();
        let after = snapshot(&root);
        let _ = std::fs::remove_dir_all(&root);

        assert!(result.is_err());
        assert_eq!(after, before);
    }

    #[cfg(unix)]
    #[test]
    fn a_temporary_profile_is_private() {
        let area = TestArea::new("a_temporary_profile_is_private");
        let profile =
            open_profile(None, true, area.get()).expect("temporary profiles are never locked");
        let paths = profile.paths.as_ref().expect("a temporary profile");

        assert_eq!(mode(&paths.root), 0o700);
        assert_eq!(mode(&paths.servo_config_dir), 0o700);
    }

    #[test]
    fn a_second_lock_on_the_same_profile_is_refused() {
        let root = unique_temporary_directory("lock");
        std::fs::create_dir_all(&root).expect("the test directory");

        let first = ProfileLock::acquire(&root);
        let second = ProfileLock::acquire(&root);
        let _ = std::fs::remove_dir_all(&root);

        assert!(first.is_ok());
        assert!(matches!(second, Err(LockError::InUse)));
    }

    #[test]
    fn the_lock_is_released_on_drop() {
        let root = unique_temporary_directory("unlock");
        std::fs::create_dir_all(&root).expect("the test directory");

        let first = ProfileLock::acquire(&root).expect("the first lock");
        drop(first);
        let again = ProfileLock::acquire(&root);
        let _ = std::fs::remove_dir_all(&root);

        assert!(again.is_ok());
    }

    #[test]
    fn a_profile_in_use_is_refused() {
        let area = TestArea::new("a_profile_in_use_is_refused");
        let root = unique_temporary_directory("in-use");

        let first =
            open_profile(Some(&root), false, area.get()).expect("the first run gets the profile");
        let second = open_profile(Some(&root), false, area.get());
        drop(first);
        let _ = std::fs::remove_dir_all(&root);

        assert_eq!(second.err(), Some(ProfileInUse { root }));
    }

    #[test]
    fn dropping_a_temporary_profile_removes_only_its_own_directory() {
        let area = TestArea::new("dropping_a_temporary_profile_removes_onl");
        let first = TemporaryProfile::create(&area.get().directory).expect("a temporary profile");
        let second =
            TemporaryProfile::create(&area.get().directory).expect("another temporary profile");
        let (first_root, second_root) = (first.root().to_path_buf(), second.root().to_path_buf());

        drop(first);
        let second_kept = second_root.is_dir();
        drop(second);

        assert!(!first_root.exists());
        assert!(second_kept);
        assert!(!second_root.exists());
    }

    #[cfg(unix)]
    mod sweep {
        use std::os::unix::fs::MetadataExt;

        use super::*;

        fn base(label: &str) -> PathBuf {
            let base = unique_temporary_directory(&format!("sweep-{label}"));
            std::fs::create_dir_all(&base).expect("the test directory");
            base
        }

        /// A leftover exactly as a killed run leaves it: private, with its `lock` file.
        fn leftover(base: &Path, name: &str) -> PathBuf {
            let root = base.join(name);
            create_private_directory(&root.join("servo")).expect("the leftover");
            std::fs::write(root.join("servo").join("cookie_jar.json"), "{}").expect("a file");
            std::fs::write(root.join("lock"), "").expect("the lock file");
            set_mode(&root, 0o700);
            root
        }

        fn owner(path: &Path) -> u32 {
            std::fs::symlink_metadata(path).expect("the entry").uid()
        }

        /// Rules under which every leftover in `base` is old enough.
        fn later(base: &Path) -> SweepRules {
            SweepRules {
                now: SystemTime::now() + Duration::from_secs(3600),
                min_age: Duration::from_secs(600),
                owner: owner(base),
                max_entries: 100,
            }
        }

        const NAME: &str = "mercury-browser-profile-123-456";

        #[test]
        fn a_stale_unlocked_leftover_is_removed() {
            let base = base("stale");
            let root = leftover(&base, NAME);

            let removed = sweep_stale_temporary_profiles(&base, &later(&base));
            let exists = root.exists();
            let _ = std::fs::remove_dir_all(&base);

            assert_eq!(removed, 1);
            assert!(!exists);
        }

        #[test]
        fn names_that_this_code_does_not_generate_are_left_alone() {
            let base = base("names");
            let names = [
                "mercury-browser-profile-123",
                "mercury-browser-profile-123-456x",
                "mercury-browser-profile--456",
                "mercury-browser-profile-12a-456",
                "Mercury-browser-profile-123-456",
                "something-else",
            ];
            for name in names {
                leftover(&base, name);
            }

            let removed = sweep_stale_temporary_profiles(&base, &later(&base));
            let remaining = names.iter().filter(|name| base.join(name).exists()).count();
            let _ = std::fs::remove_dir_all(&base);

            assert_eq!(removed, 0);
            assert_eq!(remaining, names.len());
        }

        #[test]
        fn a_symbolic_link_is_left_alone_and_its_target_kept() {
            let base = base("symlink");
            let target = leftover(&base, "target");
            std::os::unix::fs::symlink(&target, base.join(NAME)).expect("the test symlink");

            let removed = sweep_stale_temporary_profiles(&base, &later(&base));
            let kept = target.join("servo").join("cookie_jar.json").exists()
                && std::fs::symlink_metadata(base.join(NAME)).is_ok();
            let _ = std::fs::remove_dir_all(&base);

            assert_eq!(removed, 0);
            assert!(kept);
        }

        #[test]
        fn another_users_leftover_is_left_alone() {
            let base = base("owner");
            let root = leftover(&base, NAME);
            let rules = SweepRules {
                owner: owner(&base).wrapping_add(1),
                ..later(&base)
            };

            let removed = sweep_stale_temporary_profiles(&base, &rules);
            let exists = root.exists();
            let _ = std::fs::remove_dir_all(&base);

            assert_eq!(removed, 0);
            assert!(exists);
        }

        #[test]
        fn a_directory_that_is_not_private_is_left_alone() {
            let base = base("mode");
            let root = leftover(&base, NAME);
            set_mode(&root, 0o755);

            let removed = sweep_stale_temporary_profiles(&base, &later(&base));
            let exists = root.exists();
            let _ = std::fs::remove_dir_all(&base);

            assert_eq!(removed, 0);
            assert!(exists);
        }

        #[test]
        fn a_recent_leftover_is_left_alone() {
            let base = base("young");
            let root = leftover(&base, NAME);
            let rules = SweepRules {
                now: SystemTime::now(),
                ..later(&base)
            };

            let removed = sweep_stale_temporary_profiles(&base, &rules);
            let exists = root.exists();
            let _ = std::fs::remove_dir_all(&base);

            assert_eq!(removed, 0);
            assert!(exists);
        }

        #[test]
        fn a_profile_whose_lock_is_held_is_left_alone() {
            let base = base("locked");
            let root = leftover(&base, NAME);
            let lock = ProfileLock::acquire(&root).expect("a live run's lock");

            let removed = sweep_stale_temporary_profiles(&base, &later(&base));
            let exists = root.exists();
            drop(lock);
            let _ = std::fs::remove_dir_all(&base);

            assert_eq!(removed, 0);
            assert!(exists);
        }

        #[test]
        fn a_leftover_whose_lock_file_is_already_gone_is_removed() {
            let base = base("no-lock");
            let root = leftover(&base, NAME);
            std::fs::remove_file(root.join("lock")).expect("the lock file");

            let removed = sweep_stale_temporary_profiles(&base, &later(&base));
            let exists = root.exists();
            let _ = std::fs::remove_dir_all(&base);

            assert_eq!(removed, 1);
            assert!(!exists);
        }

        #[test]
        fn a_leftover_whose_lock_file_is_a_symbolic_link_is_left_alone() {
            let base = base("linked-lock");
            let root = leftover(&base, NAME);
            std::fs::remove_file(root.join("lock")).expect("the lock file");
            std::fs::write(base.join("elsewhere"), "").expect("a file");
            std::os::unix::fs::symlink(base.join("elsewhere"), root.join("lock"))
                .expect("the test symlink");

            let removed = sweep_stale_temporary_profiles(&base, &later(&base));
            let kept = root.exists() && base.join("elsewhere").exists();
            let _ = std::fs::remove_dir_all(&base);

            assert_eq!(removed, 0);
            assert!(kept);
        }

        #[test]
        fn a_symbolic_link_inside_a_leftover_is_removed_and_its_target_kept() {
            let base = base("inner-link");
            let target = base.join("outside.txt");
            std::fs::write(&target, "keep me").expect("a file");
            let root = leftover(&base, NAME);
            std::os::unix::fs::symlink(&target, root.join("servo").join("link"))
                .expect("the test symlink");
            std::os::unix::fs::symlink(&base, root.join("link-to-a-directory"))
                .expect("the test symlink");

            let removed = sweep_stale_temporary_profiles(&base, &later(&base));
            let gone = !root.exists();
            let target_contents = std::fs::read_to_string(&target);
            let _ = std::fs::remove_dir_all(&base);

            assert_eq!(removed, 1);
            assert!(gone);
            assert_eq!(target_contents.expect("the target is intact"), "keep me");
        }

        #[test]
        fn removal_takes_the_lock_file_last() {
            // SAFETY: `geteuid` has no preconditions and cannot fail.
            if unsafe { libc::geteuid() } == 0 {
                eprintln!(
                    "skipped: permissions do not stop root from removing (effective user id 0)"
                );
                return;
            }
            let base = base("lock-last");
            let root = leftover(&base, NAME);
            // A directory whose contents cannot be removed stops the removal part-way.
            let stuck = root.join("stuck");
            std::fs::create_dir_all(&stuck).expect("a directory");
            std::fs::write(stuck.join("file"), "").expect("a file");
            set_mode(&stuck, 0o500);

            let result = remove_temporary_profile_directory(&root);
            let lock_kept = root.join("lock").exists();
            set_mode(&stuck, 0o700);
            let _ = std::fs::remove_dir_all(&base);

            assert!(result.is_err());
            assert!(lock_kept, "the lock file must be the last thing removed");
        }

        #[test]
        fn opening_a_profile_sweeps_only_its_own_area() {
            let area = TestArea::new("sweeps-own");
            let leftover_root = leftover(&area.get().directory, NAME);
            let mut area_now = area.get().clone();
            area_now.sweep.min_age = Duration::ZERO;
            area_now.sweep.now = SystemTime::now() + Duration::from_secs(1);

            let profile = open_profile(None, true, &area_now).expect("a temporary profile");
            let leftover_gone = !leftover_root.exists();
            drop(profile);

            assert!(leftover_gone);
        }

        #[test]
        fn the_sweep_examines_a_bounded_number_of_entries() {
            let base = base("bounded");
            leftover(&base, NAME);
            leftover(&base, "mercury-browser-profile-123-789");
            let rules = SweepRules {
                max_entries: 1,
                ..later(&base)
            };

            let removed = sweep_stale_temporary_profiles(&base, &rules);
            let _ = std::fs::remove_dir_all(&base);

            assert_eq!(removed, 1);
        }

        #[test]
        fn a_missing_directory_is_harmless() {
            let base = unique_temporary_directory("sweep-missing");

            assert_eq!(
                sweep_stale_temporary_profiles(&base, &later(&std::env::temp_dir())),
                0
            );
        }

        #[test]
        fn a_temporary_profile_is_locked_while_it_exists() {
            let area = TestArea::new("a_temporary_profile_is_locked_while_it_e");
            let profile =
                TemporaryProfile::create(&area.get().directory).expect("a temporary profile");

            let second = ProfileLock::acquire(profile.root());

            assert!(matches!(second, Err(LockError::InUse)));
            assert!(is_temporary_profile_name(
                &profile
                    .root()
                    .file_name()
                    .expect("a name")
                    .to_string_lossy()
            ));
        }
    }

    #[test]
    fn a_relative_profile_directory_is_made_absolute() {
        let root = absolute_profile_root(Path::new("relative/profile"))
            .expect("the current directory is known");

        assert!(root.is_absolute());
        assert!(root.ends_with("relative/profile"));
    }

    #[cfg(unix)]
    #[test]
    fn an_unwritable_profile_directory_is_not_used() {
        let area = TestArea::new("an_unwritable_profile_directory_is_not_u");
        use std::os::unix::fs::PermissionsExt;

        // SAFETY: `geteuid` has no preconditions and cannot fail.
        if unsafe { libc::geteuid() } == 0 {
            eprintln!("skipped: permissions do not stop root from writing (effective user id 0)");
            return;
        }

        let root = unique_temporary_directory("read-only");
        let servo = root.join("servo");
        ProfilePaths::resolve(None, Some(&root))
            .expect("an override is enough")
            .create()
            .expect("a new profile");
        std::fs::set_permissions(&servo, std::fs::Permissions::from_mode(0o500))
            .expect("the test permissions");

        let profile =
            open_profile(Some(&root), false, area.get()).expect("the profile is not in use");
        let used = profile.paths.is_some();
        drop(profile);
        let _ = std::fs::set_permissions(&servo, std::fs::Permissions::from_mode(0o700));
        let _ = std::fs::remove_dir_all(&root);

        assert!(!used);
    }

    #[test]
    fn an_automated_run_uses_a_temporary_profile_that_is_removed_afterwards() {
        let area = TestArea::new("an_automated_run_uses_a_temporary_profil");
        let profile =
            open_profile(None, true, area.get()).expect("temporary profiles are never locked");
        let paths = profile
            .paths
            .as_ref()
            .expect("a temporary profile should be created");
        let root = paths.root.clone();

        assert!(root.starts_with(std::env::temp_dir()));
        assert!(paths.servo_config_dir.is_dir());
        drop(profile);
        assert!(!root.exists());
    }

    #[test]
    fn an_explicit_profile_directory_is_kept() {
        let area = TestArea::new("an_explicit_profile_directory_is_kept");
        let root = unique_temporary_directory("explicit");

        let profile =
            open_profile(Some(&root), true, area.get()).expect("the profile is not in use");
        assert_eq!(profile.servo_config_dir(), Some(root.join("servo")));
        drop(profile);
        let kept = root.join("servo").is_dir();
        let _ = std::fs::remove_dir_all(&root);

        assert!(kept);
    }

    #[test]
    fn the_default_profiles_http_cache_is_with_the_platforms_caches() {
        let directory = http_cache_dir(
            Path::new("/data/Mercury/Default"),
            Some(Path::new("/caches")),
        );

        assert_eq!(
            directory,
            Path::new("/caches")
                .join(APP_CACHE_DIR_NAME)
                .join("Default")
                .join("http-cache")
        );
        #[cfg(target_os = "macos")]
        assert_eq!(
            directory,
            Path::new("/caches/io.github.thepigdestroyer.mercury-browser/Default/http-cache")
        );
    }

    #[test]
    fn other_profiles_keep_their_http_cache_inside() {
        assert_eq!(
            http_cache_dir(Path::new("/x"), None),
            Path::new("/x/http-cache")
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_explicit_profile_gets_a_private_http_cache_inside_it() {
        let area = TestArea::new("an_explicit_profile_gets_a_private_http");
        let root = unique_temporary_directory("http-cache");

        let profile =
            open_profile(Some(&root), true, area.get()).expect("the profile is not in use");
        let directory = profile.http_cache_dir();
        let private = directory.as_deref().map(mode);
        drop(profile);
        let _ = std::fs::remove_dir_all(&root);

        assert_eq!(directory, Some(root.join("http-cache")));
        assert_eq!(private, Some(0o700));
    }

    #[test]
    fn a_temporary_profiles_http_cache_is_removed_with_it() {
        let area = TestArea::new("a_temporary_profiles_http_cache_is_remo");
        let profile =
            open_profile(None, true, area.get()).expect("temporary profiles are never locked");
        let root = profile
            .paths
            .as_ref()
            .expect("a temporary profile")
            .root
            .clone();
        let directory = profile.http_cache_dir().expect("a cache directory");
        std::fs::write(directory.join("cache.sqlite3"), "cached").expect("a cache file");

        assert_eq!(directory, root.join("http-cache"));
        drop(profile);
        assert!(!directory.exists());
        assert!(!root.exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_http_cache_is_not_used() {
        let base = unique_temporary_directory("http-cache-symlink");
        let root = base.join("profile");
        let elsewhere = base.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).expect("the test directories");
        ProfilePaths::resolve(None, Some(&root))
            .expect("an override is enough")
            .create()
            .expect("a profile");
        std::os::unix::fs::symlink(&elsewhere, root.join("http-cache")).expect("a link");
        let area = TestArea::new("a_symlinked_http_cache_is_not_used");

        let profile = open_profile(Some(&root), false, area.get()).expect("not in use");
        let directory = profile.http_cache_dir();
        drop(profile);
        let _ = std::fs::remove_dir_all(&base);

        assert_eq!(directory, None);
    }

    #[test]
    fn without_a_profile_there_is_no_http_cache() {
        let area = TestArea::new("without_a_profile_there_is_no_http_cache");
        let file = unique_temporary_directory("not-a-directory");
        std::fs::write(&file, "").expect("a file");

        let profile = open_profile(Some(&file), true, area.get()).expect("not in use");
        let directory = profile.http_cache_dir();
        drop(profile);
        let _ = std::fs::remove_file(&file);

        assert_eq!(directory, None);
    }
}
