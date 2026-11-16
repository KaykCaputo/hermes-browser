//! Data that must end with the browsing session even though Servo saves it in the profile:
//! session cookies (no expiry) and the HTTP-authentication cache.
//!
//! The jar clean-up below reads `cookie_jar.json` as Servo 0.8.0 writes it: `CookieStorage`
//! (`servo-net-0.8.0/cookie_storage.rs`) serialised with `serde_json`, an object whose
//! `cookies_map` maps each host to a list of `ServoCookie`s (`cookie.rs`), each with a boolean
//! `persistent`. **This depends on Servo's serialised format and must be re-checked at every
//! Servo upgrade** (the fixture test `the_fixture_jar_was_written_by_the_servo_in_use` fails
//! when the Servo version changes, to force that check). A jar in any other shape is left
//! exactly as it is.

use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::exit_clearing::{EngineSwitches, ExitClearing, remove_site_data_files};

/// Servo's HTTP-authentication cache: user names and passwords, in plain text.
const SAVED_CREDENTIALS_FILE: &str = "auth_cache.json";
/// Servo's saved cookies.
const COOKIE_JAR_FILE: &str = "cookie_jar.json";

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
/// expires, so without this a "session" cookie would outlive the browser. A failure (Servo's
/// resource thread is gone and its calls panic) is logged; the jar clean-up after the engine
/// has exited still removes the session cookies from the saved file.
pub(crate) fn end_session_cookies(servo: &servo::Servo) {
    run_contained("ending the session cookies", || {
        let site_data = servo.site_data_manager();
        // Marks every session cookie as expired (`CookieStorage::clear_session_cookies`)...
        site_data.clear_session_cookies(None);
        // ...and listing the cookie sites removes expired cookies from the jar
        // (`CoreResourceMsg::ListCookies` → `remove_all_expired_cookies`), so they are not
        // written out, with their values, when Servo saves the jar.
        let _sites = site_data.site_data(servo::StorageType::Cookies);
    });
}

/// Runs `work`, turning a panic into an error log. Returns whether it completed.
pub(crate) fn run_contained(what: &str, work: impl FnOnce()) -> bool {
    match std::panic::catch_unwind(AssertUnwindSafe(work)) {
        Ok(()) => true,
        Err(_) => {
            log::error!("{what} failed (the engine's resource thread did not answer)");
            false
        }
    }
}

/// What `without_session_cookies` made of a saved jar.
#[derive(Debug, PartialEq)]
pub(crate) enum JarCleanup {
    /// No session cookie was found; the file needs no change.
    Unchanged,
    /// The jar without its session cookies.
    Cleaned(Value),
    /// Not the shape Servo 0.8.0 writes; leave the file alone.
    Unrecognised(&'static str),
}

/// Removes the non-persistent entries from a saved cookie jar, leaving everything else as it is.
pub(crate) fn without_session_cookies(jar: &Value) -> JarCleanup {
    let Some(hosts) = jar.get("cookies_map").and_then(Value::as_object) else {
        return JarCleanup::Unrecognised("no `cookies_map` object");
    };
    // Check the whole jar before changing anything.
    let mut session_cookie_found = false;
    for entries in hosts.values() {
        let Some(entries) = entries.as_array() else {
            return JarCleanup::Unrecognised("a host's cookies are not a list");
        };
        for entry in entries {
            match entry.get("persistent").and_then(Value::as_bool) {
                Some(persistent) => session_cookie_found |= !persistent,
                None => return JarCleanup::Unrecognised("a cookie has no boolean `persistent`"),
            }
        }
    }
    if !session_cookie_found {
        return JarCleanup::Unchanged;
    }

    let mut cleaned = jar.clone();
    if let Some(hosts) = cleaned
        .get_mut("cookies_map")
        .and_then(Value::as_object_mut)
    {
        hosts.retain(|_host, entries| {
            let Some(entries) = entries.as_array_mut() else {
                return true;
            };
            let before = entries.len();
            entries.retain(|entry| entry.get("persistent").and_then(Value::as_bool) == Some(true));
            // A host is dropped only when this removed its last cookie, so the jar does not
            // keep the name of a site visited only for the session.
            !(entries.is_empty() && before > 0)
        });
    }
    JarCleanup::Cleaned(cleaned)
}

/// Removes the session cookies from the jar Servo saved in `config_dir`, writing the result
/// privately and atomically. Leaves a missing, unreadable or unrecognised file untouched.
pub(crate) fn strip_session_cookies_from_saved_jar(config_dir: &Path) {
    let path = config_dir.join(COOKIE_JAR_FILE);
    let contents = match std::fs::read(&path) {
        Ok(contents) => contents,
        // A new profile has no jar yet; that is not worth a warning.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            log::debug!("{} does not exist yet", path.display());
            return;
        }
        Err(error) => {
            log::warn!(
                "left {} as it is: it could not be read ({error})",
                path.display()
            );
            return;
        }
    };
    let jar = match serde_json::from_slice::<Value>(&contents) {
        Ok(jar) => jar,
        Err(error) => {
            log::warn!("left {} as it is: it is not JSON ({error})", path.display());
            return;
        }
    };
    let cleaned = match without_session_cookies(&jar) {
        JarCleanup::Unchanged => return,
        JarCleanup::Cleaned(cleaned) => cleaned,
        JarCleanup::Unrecognised(reason) => {
            log::warn!(
                "left {} as it is: not the cookie jar format of Servo 0.8.0 ({reason})",
                path.display()
            );
            return;
        }
    };
    if let Err(error) = write_privately_and_atomically(&path, &cleaned) {
        log::warn!(
            "could not remove the session cookies from {}: {error}",
            path.display()
        );
    }
}

/// Writes `value` to a new 0600 file next to `path`, then renames it over `path`.
fn write_privately_and_atomically(path: &Path, value: &Value) -> std::io::Result<()> {
    use std::io::Write;

    let contents = serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?;
    let temporary = path.with_extension(format!("json.tmp-{}", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let result = options.open(&temporary).and_then(|mut file| {
        file.write_all(&contents)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)
    });
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

/// Clean-up before the engine is built: the previous run may have been killed, or ended by the
/// shutdown watchdog, before it could end its session; its session cookies and credentials
/// must not come back.
///
/// If the user asked for cookies and site data to be cleared at quit, they are removed here too,
/// in case that quit never ran its clean-up.
pub(crate) fn clean_up_before_engine_start(config_dir: &Path, clearing: ExitClearing) {
    if clearing.cookies_and_site_data {
        remove_site_data_files(config_dir);
    }
    strip_session_cookies_from_saved_jar(config_dir);
    remove_saved_credentials(config_dir);
}

/// Clean-up that must run after the engine has exited and written its files: removes session
/// cookies from the saved jar and deletes the saved HTTP credentials. Every holder of a Servo
/// handle also holds one of these, dropped after its handle, so the last one goes after Servo
/// has shut down — the point after which nothing can write the files again.
///
/// If the user asked for cookies and site data to be cleared at quit (read from `switches` at
/// this moment, so the last choice counts), their files are removed here, when no engine can
/// write them again.
pub(crate) struct AfterEngineCleanup {
    config_dir: PathBuf,
    switches: EngineSwitches,
}

impl AfterEngineCleanup {
    pub(crate) fn new(config_dir: PathBuf, switches: EngineSwitches) -> Self {
        Self {
            config_dir,
            switches,
        }
    }
}

impl Drop for AfterEngineCleanup {
    fn drop(&mut self) {
        if self.switches.clear_on_exit().cookies_and_site_data {
            remove_site_data_files(&self.config_dir);
        }
        strip_session_cookies_from_saved_jar(&self.config_dir);
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

    const FIXTURE_JAR: &str = include_str!("../tests/fixtures/servo-0.8.0-cookie_jar.json");

    fn fixture() -> Value {
        serde_json::from_str(FIXTURE_JAR).expect("the fixture is JSON")
    }

    fn session_entry() -> Value {
        fixture()["cookies_map"]["httpbin.org"][0].clone()
    }

    fn persistent_entry() -> Value {
        fixture()["cookies_map"]["127.0.0.1"][0].clone()
    }

    /// The fixture is a jar written by Mercury's engine (a session cookie from one run, a
    /// persistent cookie from a loopback run; invented values). When Servo is upgraded this
    /// test fails: write a new jar with the new engine, replace the fixture, and check that
    /// `without_session_cookies` still understands it.
    #[test]
    fn the_fixture_jar_was_written_by_the_servo_in_use() {
        let lockfile = include_str!("../../../Cargo.lock");
        assert!(
            lockfile.contains("name = \"servo\"\nversion = \"0.8.0\"\n"),
            "Servo is no longer 0.8.0: re-check the cookie jar format and replace the fixture"
        );
        let jar = fixture();
        let cookies = jar["cookies_map"]
            .as_object()
            .expect("Servo's jar has a cookies_map object");
        for entry in cookies
            .values()
            .flat_map(|entries| entries.as_array().expect("each host has a list of cookies"))
        {
            assert!(
                entry["persistent"].is_boolean(),
                "a saved cookie no longer has a boolean `persistent` field: {entry}"
            );
        }
    }

    #[test]
    fn session_entries_are_removed_and_persistent_ones_kept() {
        let JarCleanup::Cleaned(cleaned) = without_session_cookies(&fixture()) else {
            panic!("the fixture has a session cookie");
        };

        let mut expected = fixture();
        expected["cookies_map"]
            .as_object_mut()
            .expect("an object")
            .remove("httpbin.org");
        assert_eq!(cleaned, expected);
        assert_eq!(cleaned["cookies_map"]["127.0.0.1"][0], persistent_entry());
    }

    #[test]
    fn a_host_keeps_its_persistent_cookies_and_unknown_fields() {
        let mut jar = fixture();
        jar["future_field"] = serde_json::json!({"kept": true});
        let mut session = session_entry();
        session["future_cookie_field"] = Value::from(7);
        let mut persistent = persistent_entry();
        persistent["future_cookie_field"] = Value::from(8);
        jar["cookies_map"]["127.0.0.1"] = Value::Array(vec![session, persistent.clone()]);

        let JarCleanup::Cleaned(cleaned) = without_session_cookies(&jar) else {
            panic!("the jar has session cookies");
        };

        assert_eq!(cleaned["future_field"], serde_json::json!({"kept": true}));
        assert_eq!(cleaned["version"], jar["version"]);
        assert_eq!(cleaned["max_per_host"], jar["max_per_host"]);
        assert_eq!(
            cleaned["cookies_map"]["127.0.0.1"],
            Value::Array(vec![persistent])
        );
        assert!(cleaned["cookies_map"].get("httpbin.org").is_none());
    }

    #[test]
    fn a_host_that_was_already_empty_is_left_alone() {
        let mut jar = fixture();
        jar["cookies_map"]["example.com"] = Value::Array(Vec::new());

        let JarCleanup::Cleaned(cleaned) = without_session_cookies(&jar) else {
            panic!("the jar has a session cookie");
        };

        assert_eq!(
            cleaned["cookies_map"]["example.com"],
            Value::Array(Vec::new())
        );
    }

    #[test]
    fn a_jar_without_session_cookies_is_unchanged() {
        let mut jar = fixture();
        jar["cookies_map"]
            .as_object_mut()
            .expect("an object")
            .remove("httpbin.org");

        assert_eq!(without_session_cookies(&jar), JarCleanup::Unchanged);
    }

    #[test]
    fn jars_in_another_shape_are_not_recognised() {
        let mut no_flag = session_entry();
        no_flag
            .as_object_mut()
            .expect("an object")
            .remove("persistent");
        let mut string_flag = session_entry();
        string_flag["persistent"] = Value::from("false");
        let shapes = [
            serde_json::json!([]),
            serde_json::json!({"version": 1}),
            serde_json::json!({"cookies_map": []}),
            serde_json::json!({"cookies_map": {"a.test": {}}}),
            serde_json::json!({"cookies_map": {"a.test": [1]}}),
            serde_json::json!({"cookies_map": {"a.test": [no_flag]}}),
            serde_json::json!({"cookies_map": {"a.test": [string_flag]}}),
        ];

        for shape in shapes {
            assert!(
                matches!(without_session_cookies(&shape), JarCleanup::Unrecognised(_)),
                "{shape}"
            );
        }
    }

    #[test]
    fn a_panic_while_clearing_is_contained() {
        assert!(!run_contained("a test", || panic!(
            "the resource thread is gone"
        )));
        assert!(run_contained("a test", || {}));
    }

    fn jar_directory(label: &str, contents: &str) -> PathBuf {
        let directory = config_dir_with_files(label);
        std::fs::write(directory.join(COOKIE_JAR_FILE), contents).expect("the test jar");
        directory
    }

    #[test]
    fn the_saved_jar_is_rewritten_without_session_cookies_and_privately() {
        let directory = jar_directory("strip", FIXTURE_JAR);

        strip_session_cookies_from_saved_jar(&directory);
        let saved: Value = serde_json::from_str(
            &std::fs::read_to_string(directory.join(COOKIE_JAR_FILE)).expect("the jar"),
        )
        .expect("JSON");
        #[cfg(unix)]
        let mode = {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(directory.join(COOKIE_JAR_FILE))
                .expect("the jar")
                .permissions()
                .mode()
                & 0o777
        };
        let leftovers = remaining_files(&directory);
        let _ = std::fs::remove_dir_all(&directory);

        let JarCleanup::Cleaned(expected) = without_session_cookies(&fixture()) else {
            panic!("the fixture has a session cookie");
        };
        assert_eq!(saved, expected);
        #[cfg(unix)]
        assert_eq!(mode, 0o600);
        assert_eq!(
            leftovers,
            vec!["auth_cache.json", "cookie_jar.json", "hsts_list.json"]
        );
    }

    #[test]
    fn a_jar_that_cannot_be_understood_is_left_byte_for_byte() {
        for contents in ["{\"cookies_map\": 5}", "not json", ""] {
            let directory = jar_directory("odd", contents);

            strip_session_cookies_from_saved_jar(&directory);
            let after = std::fs::read_to_string(directory.join(COOKIE_JAR_FILE));
            let leftovers = remaining_files(&directory);
            let _ = std::fs::remove_dir_all(&directory);

            assert_eq!(after.expect("the jar is still there"), contents);
            assert_eq!(
                leftovers,
                vec!["auth_cache.json", "cookie_jar.json", "hsts_list.json"]
            );
        }
    }

    #[test]
    fn a_jar_without_session_cookies_is_not_rewritten() {
        let mut jar = fixture();
        jar["cookies_map"]
            .as_object_mut()
            .expect("an object")
            .remove("httpbin.org");
        let contents = format!("{jar}  ");
        let directory = jar_directory("unchanged", &contents);

        strip_session_cookies_from_saved_jar(&directory);
        let after = std::fs::read_to_string(directory.join(COOKIE_JAR_FILE));
        let _ = std::fs::remove_dir_all(&directory);

        assert_eq!(after.expect("the jar"), contents);
    }

    #[test]
    fn a_missing_jar_is_harmless() {
        let directory = config_dir_with_files("missing");
        std::fs::remove_file(directory.join(COOKIE_JAR_FILE)).expect("the test jar");

        strip_session_cookies_from_saved_jar(&directory);
        let leftovers = remaining_files(&directory);
        let _ = std::fs::remove_dir_all(&directory);

        assert_eq!(leftovers, vec!["auth_cache.json", "hsts_list.json"]);
    }

    #[test]
    fn the_cleanup_strips_the_jar_and_removes_the_credentials_when_dropped() {
        let directory = jar_directory("cleanup", FIXTURE_JAR);

        drop(AfterEngineCleanup::new(
            directory.clone(),
            EngineSwitches::default(),
        ));
        let saved = std::fs::read_to_string(directory.join(COOKIE_JAR_FILE)).expect("the jar");
        let leftovers = remaining_files(&directory);
        let _ = std::fs::remove_dir_all(&directory);

        assert!(!saved.contains("mercury=1"));
        assert!(saved.contains("PERSISTVALUE42"));
        assert_eq!(leftovers, vec!["cookie_jar.json", "hsts_list.json"]);
    }

    #[test]
    fn before_the_engine_starts_session_cookies_and_credentials_are_removed() {
        let directory = jar_directory("before-start", FIXTURE_JAR);

        clean_up_before_engine_start(&directory, ExitClearing::default());
        let saved = std::fs::read_to_string(directory.join(COOKIE_JAR_FILE)).expect("the jar");
        let leftovers = remaining_files(&directory);
        let _ = std::fs::remove_dir_all(&directory);

        assert!(!saved.contains("mercury=1"));
        assert!(saved.contains("PERSISTVALUE42"));
        assert_eq!(leftovers, vec!["cookie_jar.json", "hsts_list.json"]);
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
        let remover = AfterEngineCleanup::new(directory.clone(), EngineSwitches::default());

        let present_before = directory.join(SAVED_CREDENTIALS_FILE).exists();
        drop(remover);
        let present_after = directory.join(SAVED_CREDENTIALS_FILE).exists();
        let _ = std::fs::remove_dir_all(&directory);

        assert!(present_before);
        assert!(!present_after);
    }

    const CLEAR_SITE_DATA: ExitClearing = ExitClearing {
        cookies_and_site_data: true,
        cache: false,
    };

    #[test]
    fn site_data_is_cleared_at_start_when_asked_in_case_the_last_quit_did_not() {
        let directory = crate::exit_clearing::tests::servo_directory_with_site_data();

        clean_up_before_engine_start(directory.path(), CLEAR_SITE_DATA);

        assert_eq!(
            crate::exit_clearing::tests::remaining(directory.path()),
            vec!["hsts_list.json"]
        );
    }

    #[test]
    fn site_data_is_kept_at_start_when_not_asked() {
        let directory = crate::exit_clearing::tests::servo_directory_with_site_data();

        clean_up_before_engine_start(directory.path(), ExitClearing::default());

        let left = crate::exit_clearing::tests::remaining(directory.path());
        assert!(left.contains(&"clientstorage/default_v1/reg.sqlite".to_owned()));
        assert!(left.contains(&"cookie_jar.json".to_owned()));
    }

    #[test]
    fn after_the_engine_the_last_choice_decides_what_is_removed() {
        let directory = crate::exit_clearing::tests::servo_directory_with_site_data();
        let switches = EngineSwitches::default();
        let cleanup = AfterEngineCleanup::new(directory.path().to_owned(), switches.clone());

        switches.set_clear_on_exit(CLEAR_SITE_DATA);
        drop(cleanup);

        assert_eq!(
            crate::exit_clearing::tests::remaining(directory.path()),
            vec!["hsts_list.json"]
        );
    }
}
