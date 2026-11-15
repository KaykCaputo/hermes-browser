use std::fs;
use std::path::{Path, PathBuf};

use mercury_settings::{
    Loaded, MAX_CUSTOM_ENGINES, MAX_ENGINE_NAME_BYTES, MAX_KEYWORD_BYTES, MAX_NEVER_SAVE_ORIGINS,
    MAX_SETTINGS_FILE_BYTES, MAX_TEMPLATE_BYTES, MAX_URL_BYTES, SETTINGS_FORMAT_VERSION,
    SearchEngine, Settings, SettingsError, SettingsStore,
};

fn profile() -> tempfile::TempDir {
    tempfile::tempdir().expect("a temporary directory")
}

fn settings_path(dir: &Path) -> PathBuf {
    dir.join("settings.json")
}

fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .expect("the profile should be readable")
        .map(|entry| {
            entry
                .expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

/// Seeds `settings.json` with `bytes`, loads it, and checks it was treated as damaged: defaults,
/// the original moved to the backup and readable there, nothing at the original path.
fn assert_damaged(bytes: &[u8]) -> PathBuf {
    let dir = profile();
    let path = settings_path(dir.path());
    fs::write(&path, bytes).expect("seed the file");

    let loaded = SettingsStore::new(dir.path()).load().expect("load");

    let Loaded::Damaged { value, backup } = loaded else {
        panic!("expected Damaged, got {loaded:?}");
    };
    assert_eq!(value, Settings::default());
    assert_eq!(
        backup.parent(),
        Some(dir.path()),
        "the backup sits beside it"
    );
    assert_eq!(fs::read(&backup).expect("the backup is readable"), bytes);
    assert!(!path.exists(), "the damaged file was moved, not copied");
    backup
}

#[test]
fn no_file_is_a_first_run() {
    let dir = profile();

    let loaded = SettingsStore::new(dir.path()).load().expect("load");

    assert_eq!(loaded, Loaded::Missing(Settings::default()));
    assert!(names_in(dir.path()).is_empty(), "loading writes nothing");
}

#[test]
fn text_that_is_not_json_is_damaged() {
    assert_damaged(b"this is not json");
}

#[test]
fn a_top_level_that_is_not_an_object_is_damaged() {
    assert_damaged(b"[1, 2, 3]");
    assert_damaged(b"\"settings\"");
    assert_damaged(b"null");
}

#[test]
fn a_truncated_file_is_damaged() {
    let dir = profile();
    SettingsStore::new(dir.path())
        .save(&Settings::default())
        .expect("save");
    let whole = fs::read(settings_path(dir.path())).expect("read");

    assert_damaged(&whole[..whole.len() / 2]);
    assert_damaged(b"");
}

#[test]
fn a_field_of_the_wrong_type_damages_the_whole_file_rather_than_half_applying_it() {
    assert_damaged(
        br#"{"homepage": "https://example.org/", "appearance": {"default_zoom_percent": "big"}}"#,
    );
    assert_damaged(br#"{"startup": "sometimes"}"#);
    assert_damaged(br#"{"appearance": {"default_zoom_percent": 70000}}"#);
}

#[test]
fn a_bad_format_version_is_damaged() {
    assert_damaged(br#"{"format_version": "one"}"#);
    assert_damaged(br#"{"format_version": -1}"#);
    assert_damaged(br#"{"format_version": 1.5}"#);
    assert_damaged(br#"{"format_version": null}"#);
    assert_damaged(br#"{"homepage": "", "format_version": "late and wrong"}"#);
}

#[test]
fn a_version_too_large_for_the_integer_type_is_newer() {
    for text in [
        r#"{"format_version": 99999999999, "homepage": "x"}"#,
        r#"{"homepage": "x", "format_version": 99999999999999999999999999}"#,
    ] {
        let dir = profile();
        let path = settings_path(dir.path());
        fs::write(&path, text).expect("seed");
        let store = SettingsStore::new(dir.path());

        assert_eq!(
            store.load().expect("load"),
            Loaded::Newer {
                value: Settings::default(),
                found_version: u32::MAX
            },
            "{text}"
        );
        assert!(store.save(&Settings::default()).is_err());
        assert_eq!(fs::read_to_string(&path).expect("untouched"), text);
    }
}

#[test]
fn a_version_that_is_not_the_first_member_is_still_found() {
    let dir = profile();
    let path = settings_path(dir.path());
    let text = format!(
        r#"{{"homepage": "about:blank", "format_version": {}}}"#,
        SETTINGS_FORMAT_VERSION + 1
    );
    fs::write(&path, &text).expect("seed");
    let store = SettingsStore::new(dir.path());

    assert!(matches!(store.load().expect("load"), Loaded::Newer { .. }));
    assert!(matches!(
        store.save(&Settings::default()),
        Err(SettingsError::NewerVersion { .. })
    ));
    assert_eq!(fs::read_to_string(&path).expect("untouched"), text);
}

#[test]
fn a_newer_file_larger_than_the_limit_is_left_alone() {
    let dir = profile();
    let path = settings_path(dir.path());
    let limit = usize::try_from(MAX_SETTINGS_FILE_BYTES).expect("fits");
    let mut newer = format!(
        r#"{{"format_version": {}, "padding": ""#,
        SETTINGS_FORMAT_VERSION + 1
    )
    .into_bytes();
    newer.resize(limit * 2, b'p');
    newer.extend_from_slice(b"\"}");
    fs::write(&path, &newer).expect("seed");
    let store = SettingsStore::new(dir.path());

    assert!(matches!(
        store.load().expect("load"),
        Loaded::Newer { found_version, .. } if found_version == SETTINGS_FORMAT_VERSION + 1
    ));
    assert!(matches!(
        store.save(&Settings::default()),
        Err(SettingsError::NewerVersion { .. })
    ));
    assert_eq!(fs::read(&path).expect("untouched"), newer);
    assert_eq!(names_in(dir.path()), vec!["settings.json"]);
}

#[test]
fn a_deeply_nested_document_neither_crashes_nor_costs_much() {
    let depth = 100_000;
    // Where a value is expected, nesting is refused at once.
    let mut deep = String::from(r#"{"format_version": 1, "homepage": "#);
    deep.push_str(&"[".repeat(depth));
    deep.push_str(&"]".repeat(depth));
    deep.push('}');
    assert_damaged(deep.as_bytes());

    // In an unknown member it is skipped without recursion, before and after the version.
    for version_first in [true, false] {
        let mut unknown = String::from("{");
        if version_first {
            unknown.push_str(r#""format_version": 1, "#);
        }
        unknown.push_str(r#""unknown": "#);
        unknown.push_str(&"{\"a\":".repeat(depth));
        unknown.push('1');
        unknown.push_str(&"}".repeat(depth));
        unknown.push_str(r#", "homepage": "about:blank"}"#);
        let dir = profile();
        fs::write(settings_path(dir.path()), &unknown).expect("seed");

        let loaded = SettingsStore::new(dir.path()).load().expect("load");

        let Loaded::Ok { value, .. } = loaded else {
            panic!("expected Ok, got {loaded:?}");
        };
        assert_eq!(value.homepage, "about:blank");
    }
}

#[test]
fn duplicate_keys_damage_the_file() {
    assert_damaged(
        br#"{"format_version": 1, "homepage": "about:blank", "homepage": "https://evil.example/"}"#,
    );
    assert_damaged(br#"{"format_version": 1, "appearance": {"theme": "dark", "theme": "light"}}"#);
}

#[cfg(unix)]
#[test]
fn a_backup_is_private_whatever_the_damaged_file_was() {
    use std::os::unix::fs::PermissionsExt as _;

    let dir = profile();
    let path = settings_path(dir.path());
    fs::write(&path, "damaged").expect("seed");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("make it public");

    let Loaded::Damaged { backup, .. } = SettingsStore::new(dir.path()).load().expect("load")
    else {
        panic!("expected Damaged");
    };

    let mode = fs::metadata(&backup)
        .expect("the backup")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn a_file_over_the_size_limit_is_damaged_even_if_it_is_valid_json() {
    let limit = usize::try_from(MAX_SETTINGS_FILE_BYTES).expect("fits");
    let mut big = b"{\"homepage\": \"about:blank\"".to_vec();
    big.resize(limit, b' ');
    big.push(b'}');
    assert_eq!(big.len(), limit + 1);

    assert_damaged(&big);
}

#[test]
fn a_file_at_the_size_limit_is_read() {
    let limit = usize::try_from(MAX_SETTINGS_FILE_BYTES).expect("fits");
    let mut big = b"{\"homepage\": \"about:blank\"".to_vec();
    big.resize(limit - 1, b' ');
    big.push(b'}');
    let dir = profile();
    fs::write(settings_path(dir.path()), &big).expect("seed");

    let loaded = SettingsStore::new(dir.path()).load().expect("load");

    assert!(matches!(loaded, Loaded::Ok { .. }), "{loaded:?}");
}

#[test]
fn every_damaged_file_gets_its_own_backup() {
    let dir = profile();
    let path = settings_path(dir.path());
    let store = SettingsStore::new(dir.path());
    let squatter = dir.path().join("settings.json.damaged-1");
    fs::write(&squatter, b"an earlier backup").expect("seed an earlier backup");

    let mut backups = Vec::new();
    for attempt in ["first damage", "second damage", "third damage"] {
        fs::write(&path, attempt).expect("damage the file");
        match store.load().expect("load") {
            Loaded::Damaged { backup, .. } => backups.push(backup),
            other => panic!("expected Damaged, got {other:?}"),
        }
    }

    assert_eq!(
        fs::read(&squatter).expect("still there"),
        b"an earlier backup"
    );
    let contents: Vec<Vec<u8>> = backups
        .iter()
        .map(|backup| fs::read(backup).expect("each backup is readable"))
        .collect();
    assert_eq!(
        contents,
        vec![
            b"first damage".to_vec(),
            b"second damage".to_vec(),
            b"third damage".to_vec()
        ]
    );
    let mut distinct = backups.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(distinct.len(), 3);
    assert!(!backups.contains(&squatter));
}

#[test]
fn a_newer_file_is_left_alone_and_saving_over_it_is_refused() {
    let dir = profile();
    let path = settings_path(dir.path());
    let newer = format!(
        r#"{{"format_version": {}, "homepage": "https://from.the.future/", "hologram": true}}"#,
        SETTINGS_FORMAT_VERSION + 1
    );
    fs::write(&path, &newer).expect("seed");
    let store = SettingsStore::new(dir.path());

    let loaded = store.load().expect("load");

    assert_eq!(
        loaded,
        Loaded::Newer {
            value: Settings::default(),
            found_version: SETTINGS_FORMAT_VERSION + 1
        }
    );
    assert_eq!(fs::read_to_string(&path).expect("read"), newer);

    let saved = store.save(&Settings::default());

    assert!(
        matches!(
            saved,
            Err(SettingsError::NewerVersion { found, supported, .. })
                if found == SETTINGS_FORMAT_VERSION + 1 && supported == SETTINGS_FORMAT_VERSION
        ),
        "{saved:?}"
    );
    assert_eq!(fs::read_to_string(&path).expect("read"), newer);
    assert_eq!(names_in(dir.path()), vec!["settings.json"]);
}

#[test]
fn an_unversioned_or_version_0_file_is_migrated() {
    for text in [
        r#"{"homepage": "https://example.org/"}"#,
        r#"{"format_version": 0, "homepage": "https://example.org/"}"#,
    ] {
        let dir = profile();
        let path = settings_path(dir.path());
        fs::write(&path, text).expect("seed");
        let store = SettingsStore::new(dir.path());

        let loaded = store.load().expect("load");

        let expected = Settings {
            homepage: "https://example.org/".to_owned(),
            ..Settings::default()
        };
        assert_eq!(
            loaded,
            Loaded::Ok {
                value: expected.clone(),
                repairs: Vec::new()
            }
        );
        store.save(&expected).expect("save");
        let written: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).expect("read")).expect("json");
        assert_eq!(written["format_version"], SETTINGS_FORMAT_VERSION);
    }
}

#[test]
fn repairs_are_reported_on_load() {
    let dir = profile();
    fs::write(
        settings_path(dir.path()),
        r#"{"format_version": 1, "homepage": "javascript:alert(1)"}"#,
    )
    .expect("seed");

    let loaded = SettingsStore::new(dir.path()).load().expect("load");

    let Loaded::Ok { value, repairs } = loaded else {
        panic!("expected Ok, got {loaded:?}");
    };
    assert_eq!(value.homepage, "");
    assert_eq!(repairs.len(), 1);
    assert_eq!(repairs[0].field, "homepage");
}

#[test]
fn a_directory_at_the_path_is_an_error() {
    let dir = profile();
    fs::create_dir(settings_path(dir.path())).expect("put a directory in the way");
    let store = SettingsStore::new(dir.path());

    assert!(matches!(store.load(), Err(SettingsError::Io { .. })));
    assert!(matches!(
        store.save(&Settings::default()),
        Err(SettingsError::Io { .. })
    ));
    assert!(settings_path(dir.path()).is_dir());
}

#[cfg(unix)]
#[test]
fn an_unreadable_file_is_an_error() {
    use std::os::unix::fs::PermissionsExt as _;

    let dir = profile();
    let path = settings_path(dir.path());
    fs::write(&path, "{}").expect("seed");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).expect("lock it");
    if fs::read(&path).is_ok() {
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("unlock it");
        eprintln!("skipped: permissions are not enforced here (running as root?)");
        return;
    }

    let loaded = SettingsStore::new(dir.path()).load();

    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("unlock it");
    assert!(
        matches!(loaded, Err(SettingsError::Io { .. })),
        "{loaded:?}"
    );
    assert_eq!(fs::read(&path).expect("untouched"), b"{}");
}

#[cfg(unix)]
#[test]
fn a_fifo_at_the_path_is_an_error_not_a_hang() {
    let dir = profile();
    let path = settings_path(dir.path());
    let status = std::process::Command::new("mkfifo")
        .arg(&path)
        .status()
        .expect("run mkfifo");
    assert!(status.success());

    let loaded = SettingsStore::new(dir.path()).load();

    assert!(
        matches!(loaded, Err(SettingsError::Io { .. })),
        "{loaded:?}"
    );
}

#[cfg(unix)]
#[test]
fn the_saved_file_is_private() {
    use std::os::unix::fs::PermissionsExt as _;

    let dir = profile();
    SettingsStore::new(dir.path())
        .save(&Settings::default())
        .expect("save");

    let mode = fs::metadata(settings_path(dir.path()))
        .expect("the file")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn saving_over_a_damaged_file_keeps_a_copy_of_it() {
    let dir = profile();
    let path = settings_path(dir.path());
    fs::write(&path, "garbage that was never loaded").expect("seed");

    SettingsStore::new(dir.path())
        .save(&Settings::default())
        .expect("save");

    assert_eq!(
        names_in(dir.path()),
        vec!["settings.json", "settings.json.damaged-1"]
    );
    assert_eq!(
        fs::read(dir.path().join("settings.json.damaged-1")).expect("the copy"),
        b"garbage that was never loaded"
    );
}

#[test]
fn save_writes_the_validated_settings() {
    let dir = profile();
    let store = SettingsStore::new(dir.path());
    let mut settings = Settings::default();
    settings.appearance.default_zoom_percent = 9000;

    store.save(&settings).expect("save");

    let Loaded::Ok { value, repairs } = store.load().expect("load") else {
        panic!("expected Ok");
    };
    assert_eq!(value.appearance.default_zoom_percent, 100);
    assert!(repairs.is_empty(), "the file on disk is already repaired");
}

#[test]
fn the_largest_valid_settings_fit_under_the_file_limit() {
    // Every list at its limit and every string at its longest, with characters JSON must
    // escape where they are allowed, so a saved file can always be loaded again.
    let quotes = |n: usize| "\"".repeat(n);
    let mut settings = Settings {
        homepage: format!("about:{}", quotes(MAX_URL_BYTES - "about:".len())),
        ..Settings::default()
    };
    settings.search.custom_engines = (0..MAX_CUSTOM_ENGINES)
        .map(|n| {
            let id = format!("e{n}");
            let prefix = format!("https://{id}.example/?q={{searchTerms}}&x=");
            let template = format!("{prefix}{}", quotes(MAX_TEMPLATE_BYTES - prefix.len()));
            SearchEngine {
                id: id.clone(),
                name: format!("{id}{}", quotes(MAX_ENGINE_NAME_BYTES - id.len())),
                keyword: format!("{id}{}", quotes(MAX_KEYWORD_BYTES - id.len())),
                url_template: template.clone(),
                suggest_template: Some(template),
            }
        })
        .collect();
    settings.passwords.never_save_origins = (0..MAX_NEVER_SAVE_ORIGINS)
        .map(|n| {
            format!(
                "https://{}{n}.example",
                format!("{}.", "a".repeat(63)).repeat(4)
            )
        })
        .collect();
    let (settings, repairs) = settings.validated();
    assert!(repairs.is_empty(), "{repairs:?}");

    let dir = profile();
    let store = SettingsStore::new(dir.path());
    store.save(&settings).expect("save");

    let size = fs::metadata(settings_path(dir.path()))
        .expect("the file")
        .len();
    assert!(size <= MAX_SETTINGS_FILE_BYTES, "{size} bytes");
    assert_eq!(
        store.load().expect("load"),
        Loaded::Ok {
            value: settings,
            repairs: Vec::new()
        }
    );
}

#[test]
fn a_file_starting_with_a_byte_order_mark_is_damaged() {
    assert_damaged(b"\xEF\xBB\xBF{\"format_version\": 1}");
}

#[test]
fn a_newer_file_over_the_limit_whose_version_is_not_first_is_damaged() {
    // The documented edge: finding a late version needs a scan, and the scan honours the limit.
    let limit = usize::try_from(MAX_SETTINGS_FILE_BYTES).expect("fits");
    let mut text = b"{\"homepage\": \"".to_vec();
    text.resize(limit + 10, b'p');
    text.extend_from_slice(
        format!("\", \"format_version\": {}}}", SETTINGS_FORMAT_VERSION + 1).as_bytes(),
    );
    assert_damaged(&text);
}
