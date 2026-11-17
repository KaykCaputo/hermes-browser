//! Browsing data the user asked to have cleared when the browser quits, and the switches the
//! browser can change while the engine runs.

use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;

use servo::StorageType;

use crate::session_data::run_contained;

/// What is cleared when the browser quits, and again at the next start in case that quit never
/// ran its clean-up (a crash, a kill, the shutdown watchdog). Nothing by default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExitClearing {
    /// Every cookie, and all site storage: local and session storage, IndexedDB and Cache
    /// Storage.
    pub cookies_and_site_data: bool,
    /// The HTTP cache. Servo 0.8.0 keeps it in memory only (Mercury sets no
    /// `network_http_disk_cache`), so it never outlives the run; it is cleared anyway, so this
    /// stays true if a disk cache is ever turned on.
    pub cache: bool,
}

impl ExitClearing {
    pub(crate) const fn clears_anything(self) -> bool {
        self.cookies_and_site_data || self.cache
    }
}

/// Settings the browser can change while the engine runs. Clones share one state, which the
/// engine reads when it acts: the content blocker at every load, the clean-up at quit. So a
/// change never waits for the engine, and the value used at quit is the last one set.
#[derive(Clone, Debug)]
pub struct EngineSwitches {
    state: Rc<SwitchState>,
}

#[derive(Debug)]
struct SwitchState {
    content_blocking: Cell<bool>,
    clear_on_exit: Cell<ExitClearing>,
}

impl EngineSwitches {
    pub fn new(content_blocking: bool, clear_on_exit: ExitClearing) -> Self {
        Self {
            state: Rc::new(SwitchState {
                content_blocking: Cell::new(content_blocking),
                clear_on_exit: Cell::new(clear_on_exit),
            }),
        }
    }

    /// Turns ad and tracker blocking on or off for every load from now on.
    pub fn set_content_blocking(&self, enabled: bool) {
        if self.state.content_blocking.replace(enabled) != enabled {
            log::info!(
                "ad and tracker blocking {}",
                if enabled { "enabled" } else { "disabled" }
            );
        }
    }

    pub fn content_blocking(&self) -> bool {
        self.state.content_blocking.get()
    }

    /// What is cleared when the engine shuts down.
    pub fn set_clear_on_exit(&self, clearing: ExitClearing) {
        self.state.clear_on_exit.set(clearing);
    }

    pub fn clear_on_exit(&self) -> ExitClearing {
        self.state.clear_on_exit.get()
    }
}

impl Default for EngineSwitches {
    /// Blocking on, nothing cleared: the browser's defaults.
    fn default() -> Self {
        Self::new(true, ExitClearing::default())
    }
}

/// Clears what `clearing` names while Servo still runs, so that the files it writes at exit
/// no longer hold it. A failure is logged (see `run_contained`).
pub(crate) fn clear_at_exit(servo: &servo::Servo, clearing: ExitClearing) {
    if clearing.cookies_and_site_data {
        run_contained("clearing cookies and site data at exit", || {
            let site_data = servo.site_data_manager();
            site_data.clear_cookies(None);
            let storage = StorageType::Local | StorageType::Session;
            let sites = site_data
                .site_data(storage)
                .into_iter()
                .map(|site| site.name())
                .collect::<Vec<_>>();
            let sites = sites.iter().map(String::as_str).collect::<Vec<_>>();
            site_data.clear_site_data(&sites, storage);
            log::info!(
                "cleared cookies and the storage of {} site(s) at exit",
                sites.len()
            );
        });
    }
    if clearing.cache {
        run_contained("clearing the cache at exit", || {
            servo.network_manager().clear_cache();
            log::info!("cleared the HTTP cache at exit");
        });
    }
}

/// The files and directories in Servo's profile directory that hold cookies and site data
/// (Servo 0.8.0: `servo-net` `cookie_storage`, `servo-storage` `webstorage`, `client_storage`
/// for IndexedDB and `cache_storage`). The storage roots go whole: Servo recreates them with
/// `create_dir_all` when it starts. Re-check at every Servo upgrade (a test fails then).
const SITE_DATA_FILES: [&str; 2] = ["cookie_jar.json", "localstorage.json"];
const SITE_DATA_DIRECTORIES: [&str; 3] = ["webstorage", "clientstorage", "cachestorage"];

/// Removes cookies and site data from Servo's profile directory. Only ever call it while no
/// Servo is running on that directory: before the engine starts, or after it has gone.
/// Symbolic links are removed, never followed. Failures are logged.
pub(crate) fn remove_site_data_files(config_dir: &Path) {
    let mut removed = 0;
    for name in SITE_DATA_FILES.iter().chain(&SITE_DATA_DIRECTORIES) {
        let path = config_dir.join(name);
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        // A symbolic link is a file to `remove_file`; `remove_dir_all` does not follow links
        // inside the tree either.
        let result = if metadata.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        match result {
            Ok(()) => removed += 1,
            Err(error) => log::warn!("could not remove {}: {error}", path.display()),
        }
    }
    if removed > 0 {
        log::info!("removed {removed} stores of cookies and site data");
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn nothing_is_cleared_by_default() {
        assert!(!ExitClearing::default().clears_anything());
        assert!(
            ExitClearing {
                cache: true,
                ..ExitClearing::default()
            }
            .clears_anything()
        );
        assert!(
            ExitClearing {
                cookies_and_site_data: true,
                ..ExitClearing::default()
            }
            .clears_anything()
        );
    }

    #[test]
    fn clones_of_the_switches_share_their_values() {
        let switches = EngineSwitches::default();
        let held_by_the_engine = switches.clone();
        assert!(held_by_the_engine.content_blocking());

        switches.set_content_blocking(false);
        switches.set_clear_on_exit(ExitClearing {
            cookies_and_site_data: true,
            cache: false,
        });

        assert!(!held_by_the_engine.content_blocking());
        assert!(held_by_the_engine.clear_on_exit().cookies_and_site_data);
    }

    pub(crate) fn servo_directory_with_site_data() -> tempfile::TempDir {
        let directory = tempfile::tempdir().expect("a directory");
        let root = directory.path();
        for file in [
            "cookie_jar.json",
            "localstorage.json",
            "hsts_list.json",
            "auth_cache.json",
        ] {
            std::fs::write(root.join(file), "{}").expect("a file");
        }
        for nested in [
            "webstorage/0f6a/data",
            "clientstorage/default_v1/https+++a.test/idb",
            "cachestorage/default_v1/https+++a.test",
        ] {
            std::fs::create_dir_all(root.join(nested)).expect("a directory");
            std::fs::write(root.join(nested).join("x.sqlite"), "data").expect("a file");
        }
        std::fs::write(root.join("clientstorage/default_v1/reg.sqlite"), "r").expect("a file");
        directory
    }

    pub(crate) fn remaining(root: &Path) -> Vec<String> {
        let mut found = Vec::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(directory) = pending.pop() {
            for entry in std::fs::read_dir(&directory).expect("a directory") {
                let path = entry.expect("an entry").path();
                let name = path
                    .strip_prefix(root)
                    .expect("inside")
                    .to_string_lossy()
                    .into_owned();
                if path.is_dir() {
                    pending.push(path);
                }
                found.push(name);
            }
        }
        found.sort();
        found
    }

    #[test]
    fn removing_site_data_keeps_hsts_and_credentials_only() {
        let directory = servo_directory_with_site_data();

        remove_site_data_files(directory.path());

        assert_eq!(
            remaining(directory.path()),
            vec!["auth_cache.json", "hsts_list.json"]
        );
    }

    #[test]
    fn removing_site_data_that_is_not_there_is_harmless() {
        let directory = tempfile::tempdir().expect("a directory");

        remove_site_data_files(directory.path());

        assert!(remaining(directory.path()).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_storage_root_is_unlinked_not_followed() {
        let outside = tempfile::tempdir().expect("a directory");
        std::fs::create_dir_all(outside.path().join("default_v1")).expect("a directory");
        std::fs::write(outside.path().join("default_v1/keep.txt"), "mine").expect("a file");
        let directory = tempfile::tempdir().expect("a directory");
        std::os::unix::fs::symlink(outside.path(), directory.path().join("clientstorage"))
            .expect("a link");

        remove_site_data_files(directory.path());

        assert!(outside.path().join("default_v1/keep.txt").exists());
        assert!(std::fs::symlink_metadata(directory.path().join("clientstorage")).is_err());
    }

    /// The store names come from Servo 0.8.0's sources (see `SITE_DATA_DIRECTORIES`). When
    /// Servo is upgraded this test fails: check where the new version keeps cookies, web
    /// storage, IndexedDB and Cache Storage, update the lists, then this version number.
    #[test]
    fn the_site_data_layout_was_read_from_the_servo_in_use() {
        let lockfile = include_str!("../../../Cargo.lock");
        assert!(
            lockfile.contains("name = \"servo\"\nversion = \"0.8.0\"\n"),
            "Servo is no longer 0.8.0: re-check where it keeps site data"
        );
    }
}
