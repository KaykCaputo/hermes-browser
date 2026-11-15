use std::fs::{DirBuilder, File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(target_os = "macos")]
pub(crate) const APP_DIR_NAME: &str = "Mercury Browser";
#[cfg(not(target_os = "macos"))]
pub(crate) const APP_DIR_NAME: &str = "mercury-browser";

const DEFAULT_PROFILE_NAME: &str = "Default";
const LOCK_FILE_NAME: &str = "lock";

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
            servo_config_dir: root.join("servo"),
            places_database: root.join("places.sqlite"),
            root,
        })
    }

    /// Creates `root` and `servo_config_dir`, readable by this user only: the profile holds
    /// cookies and, while the browser runs, HTTP credentials. Existing directories that are
    /// open to others are tightened.
    pub(crate) fn create(&self) -> io::Result<()> {
        create_private_directory(&self.root)?;
        create_private_directory(&self.servo_config_dir)?;
        let mut tightened = Vec::new();
        for directory in [&self.root, &self.servo_config_dir] {
            if make_private(directory)? {
                tightened.push(directory.display().to_string());
            }
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

/// Creates `path` and any missing parents with mode 0700 on Unix, set at creation.
fn create_private_directory(path: &Path) -> io::Result<()> {
    let mut builder = DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(path)
}

/// Removes group and other permissions from an existing directory. Returns whether it changed.
#[cfg(unix)]
fn make_private(path: &Path) -> io::Result<bool> {
    use std::os::unix::fs::PermissionsExt;

    let mode = std::fs::metadata(path)?.permissions().mode();
    if mode & 0o077 == 0 {
        return Ok(false);
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode & 0o700))?;
    Ok(true)
}

#[cfg(not(unix))]
fn make_private(_path: &Path) -> io::Result<bool> {
    Ok(false)
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

/// The profile this run uses. Keep it alive until the engine has shut down: it holds the
/// profile's lock, and a temporary profile is deleted when it is dropped.
pub(crate) struct ActiveProfile {
    pub paths: Option<ProfilePaths>,
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
}

/// Chooses, creates and locks the profile: `override_root` when given; otherwise a fresh
/// temporary directory for an automated run, so those never touch the user's data; otherwise
/// the user's default profile. If it cannot be created or written, the run keeps no site data.
/// If another Mercury Browser holds it, the run must not start.
pub(crate) fn open_profile(
    override_root: Option<&Path>,
    automated_run: bool,
) -> Result<ActiveProfile, ProfileInUse> {
    let mut temporary = None;
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
        match TemporaryProfile::create() {
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

    Ok(ActiveProfile {
        paths,
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
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
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
}

impl TemporaryProfile {
    fn create() -> io::Result<Self> {
        let nanoseconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let root = std::env::temp_dir().join(format!(
            "mercury-browser-profile-{}-{nanoseconds}",
            std::process::id()
        ));
        let mut builder = DirBuilder::new();
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        builder.create(&root)?;
        if let Ok(mut current) = TEMPORARY_PROFILE_ROOT.lock() {
            *current = Some(root.clone());
        }
        Ok(Self { root })
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

fn remove_directory(root: &Path) {
    match std::fs::remove_dir_all(root) {
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
    #[test]
    fn an_existing_profile_with_wider_permissions_is_tightened() {
        use std::os::unix::fs::PermissionsExt;

        let root = unique_temporary_directory("tighten");
        let paths = ProfilePaths::resolve(None, Some(&root)).expect("an override is enough");
        std::fs::create_dir_all(&paths.servo_config_dir).expect("the test directories");
        for directory in [&paths.root, &paths.servo_config_dir] {
            std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o755))
                .expect("the test permissions");
        }

        let result = paths.create();
        let modes = (mode(&paths.root), mode(&paths.servo_config_dir));
        let _ = std::fs::remove_dir_all(&root);

        result.expect("an existing profile should be accepted");
        assert_eq!(modes, (0o700, 0o700));
    }

    #[cfg(unix)]
    #[test]
    fn a_temporary_profile_is_private() {
        let profile = open_profile(None, true).expect("temporary profiles are never locked");
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
        let root = unique_temporary_directory("in-use");

        let first = open_profile(Some(&root), false).expect("the first run gets the profile");
        let second = open_profile(Some(&root), false);
        drop(first);
        let _ = std::fs::remove_dir_all(&root);

        assert_eq!(second.err(), Some(ProfileInUse { root }));
    }

    #[test]
    fn dropping_a_temporary_profile_removes_only_its_own_directory() {
        let first = TemporaryProfile::create().expect("a temporary profile");
        let second = TemporaryProfile::create().expect("another temporary profile");
        let (first_root, second_root) = (first.root().to_path_buf(), second.root().to_path_buf());

        drop(first);
        let second_kept = second_root.is_dir();
        drop(second);

        assert!(!first_root.exists());
        assert!(second_kept);
        assert!(!second_root.exists());
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
        use std::os::unix::fs::PermissionsExt;

        let root = unique_temporary_directory("read-only");
        let servo = root.join("servo");
        std::fs::create_dir_all(&servo).expect("the test directories");
        std::fs::set_permissions(&servo, std::fs::Permissions::from_mode(0o500))
            .expect("the test permissions");

        let profile = open_profile(Some(&root), false).expect("the profile is not in use");
        let used = profile.paths.is_some();
        drop(profile);
        let _ = std::fs::set_permissions(&servo, std::fs::Permissions::from_mode(0o700));
        let _ = std::fs::remove_dir_all(&root);

        assert!(!used);
    }

    #[test]
    fn an_automated_run_uses_a_temporary_profile_that_is_removed_afterwards() {
        let profile = open_profile(None, true).expect("temporary profiles are never locked");
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
        let root = unique_temporary_directory("explicit");

        let profile = open_profile(Some(&root), true).expect("the profile is not in use");
        assert_eq!(profile.servo_config_dir(), Some(root.join("servo")));
        drop(profile);
        let kept = root.join("servo").is_dir();
        let _ = std::fs::remove_dir_all(&root);

        assert!(kept);
    }
}
