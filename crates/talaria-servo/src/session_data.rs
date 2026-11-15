//! Data that must end with the browsing session even though Servo saves it in the profile:
//! session cookies (no expiry) and the HTTP-authentication cache.

use std::path::{Path, PathBuf};

/// Servo's HTTP-authentication cache: user names and passwords, in plain text.
const SAVED_CREDENTIALS_FILE: &str = "auth_cache.json";

/// Deletes the HTTP credentials Servo saved in `config_dir`.
pub(crate) fn remove_saved_credentials(config_dir: &Path) {
    let path = config_dir.join(SAVED_CREDENTIALS_FILE);
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => log::warn!("could not remove {}: {error}", path.display()),
    }
}

/// Ends the session cookies (those without an expiry) of a Servo instance that keeps its data
/// in a profile. Servo saves its whole cookie jar at exit and a cookie with no expiry never
/// expires, so without this a "session" cookie would outlive the browser.
pub(crate) fn end_session_cookies(servo: &servo::Servo) {
    let site_data = servo.site_data_manager();
    // Marks every session cookie as expired (`CookieStorage::clear_session_cookies`)...
    site_data.clear_session_cookies(None);
    // ...and listing the cookie sites removes expired cookies from the jar
    // (`CoreResourceMsg::ListCookies` → `remove_all_expired_cookies`), so they are not written
    // out, with their values, when Servo saves the jar.
    let _sites = site_data.site_data(servo::StorageType::Cookies);
}

/// Deletes the saved HTTP credentials when dropped. Every holder of a Servo handle also holds
/// one of these, dropped after its handle, so the last one goes after Servo has shut down and
/// written its files.
pub(crate) struct SavedCredentialsRemover {
    config_dir: PathBuf,
}

impl SavedCredentialsRemover {
    pub(crate) fn new(config_dir: PathBuf) -> Self {
        Self { config_dir }
    }
}

impl Drop for SavedCredentialsRemover {
    fn drop(&mut self) {
        remove_saved_credentials(&self.config_dir);
    }
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn config_dir_with_files(label: &str) -> PathBuf {
        let nanoseconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let directory = std::env::temp_dir().join(format!(
            "talaria-session-data-{label}-{}-{nanoseconds}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).expect("the test directory");
        for file in [SAVED_CREDENTIALS_FILE, "cookie_jar.json", "hsts_list.json"] {
            std::fs::write(directory.join(file), "{}").expect("the test file");
        }
        directory
    }

    fn remaining_files(directory: &Path) -> Vec<String> {
        let mut names = std::fs::read_dir(directory)
            .expect("the test directory")
            .map(|entry| {
                entry
                    .expect("a directory entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    #[test]
    fn saved_credentials_are_removed_and_nothing_else() {
        let directory = config_dir_with_files("remove");

        remove_saved_credentials(&directory);
        let remaining = remaining_files(&directory);
        let _ = std::fs::remove_dir_all(&directory);

        assert_eq!(remaining, vec!["cookie_jar.json", "hsts_list.json"]);
    }

    #[test]
    fn removing_credentials_that_are_not_there_is_harmless() {
        let directory = config_dir_with_files("absent");
        std::fs::remove_file(directory.join(SAVED_CREDENTIALS_FILE)).expect("the test file");

        remove_saved_credentials(&directory);
        let remaining = remaining_files(&directory);
        let _ = std::fs::remove_dir_all(&directory);

        assert_eq!(remaining, vec!["cookie_jar.json", "hsts_list.json"]);
    }

    #[test]
    fn the_remover_deletes_the_credentials_when_dropped() {
        let directory = config_dir_with_files("drop");
        let remover = SavedCredentialsRemover::new(directory.clone());

        let present_before = directory.join(SAVED_CREDENTIALS_FILE).exists();
        drop(remover);
        let present_after = directory.join(SAVED_CREDENTIALS_FILE).exists();
        let _ = std::fs::remove_dir_all(&directory);

        assert!(present_before);
        assert!(!present_after);
    }
}
