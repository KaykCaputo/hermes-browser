use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(target_os = "macos")]
pub(crate) const APP_DIR_NAME: &str = "Mercury Browser";
#[cfg(not(target_os = "macos"))]
pub(crate) const APP_DIR_NAME: &str = "mercury-browser";

const DEFAULT_PROFILE_NAME: &str = "Default";

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

    /// Creates `root` and `servo_config_dir`.
    pub(crate) fn create(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.root)?;
        std::fs::create_dir_all(&self.servo_config_dir)
    }
}

/// The profile this run uses. Keep it alive until the engine has shut down: a temporary
/// profile is deleted when it is dropped.
pub(crate) struct ActiveProfile {
    pub paths: Option<ProfilePaths>,
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

/// Chooses and creates the profile: `override_root` when given; otherwise a fresh temporary
/// directory for a screenshot run, so automated runs never touch the user's data; otherwise
/// the user's default profile. If it cannot be created the run keeps no site data.
pub(crate) fn open_profile(override_root: Option<&Path>, screenshot_run: bool) -> ActiveProfile {
    let mut temporary = None;
    let paths = if let Some(root) = override_root {
        ProfilePaths::resolve(None, Some(root))
    } else if screenshot_run {
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

    // Servo's logger is not installed yet, so errors go to standard error directly.
    let paths = paths.and_then(|paths| match paths.create() {
        Ok(()) => Some(paths),
        Err(error) => {
            eprintln!(
                "mercury-browser: could not create the profile at {}: {error}; site data will not be kept",
                paths.root.display()
            );
            None
        }
    });

    ActiveProfile {
        paths,
        _temporary: temporary,
    }
}

/// The temporary profile's root while one exists, so that exit paths that skip destructors
/// can still remove it.
static TEMPORARY_PROFILE_ROOT: Mutex<Option<PathBuf>> = Mutex::new(None);

/// A profile directory under the system's temporary directory, removed when dropped.
struct TemporaryProfile {
    root: PathBuf,
}

impl TemporaryProfile {
    fn create() -> std::io::Result<Self> {
        let nanoseconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let root = std::env::temp_dir().join(format!(
            "mercury-browser-profile-{}-{nanoseconds}",
            std::process::id()
        ));
        std::fs::create_dir(&root)?;
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
        remove_temporary_profile();
    }
}

/// Deletes the temporary profile, if this run created one. Safe to call more than once; call
/// it before `std::process::exit`, which skips destructors.
pub(crate) fn remove_temporary_profile() {
    let root = TEMPORARY_PROFILE_ROOT
        .lock()
        .ok()
        .and_then(|mut current| current.take());
    if let Some(root) = root
        && let Err(error) = std::fs::remove_dir_all(&root)
    {
        eprintln!(
            "mercury-browser: could not remove the temporary profile at {}: {error}",
            root.display()
        );
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

    #[test]
    fn a_screenshot_run_uses_a_temporary_profile_that_is_removed_afterwards() {
        let profile = open_profile(None, true);
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

        let profile = open_profile(Some(&root), true);
        assert_eq!(profile.servo_config_dir(), Some(root.join("servo")));
        drop(profile);
        let kept = root.join("servo").is_dir();
        let _ = std::fs::remove_dir_all(&root);

        assert!(kept);
    }
}
