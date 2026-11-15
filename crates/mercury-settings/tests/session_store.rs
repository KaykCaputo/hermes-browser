use std::fs;
use std::path::Path;

use mercury_settings::{
    ClosedTab, MAX_CLOSED_TABS, MAX_RESTORED_TABS, MAX_SESSION_FILE_BYTES, MAX_TITLE_BYTES,
    MAX_URL_BYTES, PreviousSession, SESSION_FORMAT_VERSION, SessionSnapshot, SessionStore,
    SettingsError, StartupSessions, TabSnapshot, WindowBounds, WindowSnapshot,
};

const EXAMPLE_FILE: &str = r#"{
  "format_version": 1,
  "exited_cleanly": false,
  "windows": [
    {
      "tabs": [
        {
          "url": "https://example.org/",
          "title": "Example Domain",
          "zoom_percent": 100,
          "pinned": true
        },
        {
          "url": "about:blank",
          "title": "",
          "zoom_percent": 125,
          "pinned": false
        }
      ],
      "active_tab": 1,
      "bounds": {
        "x": 80,
        "y": 60,
        "width": 1280,
        "height": 800
      },
      "maximized": false,
      "fullscreen": false
    }
  ],
  "closed_tabs": [
    {
      "tab": {
        "url": "https://www.rust-lang.org/",
        "title": "Rust",
        "zoom_percent": 100,
        "pinned": false
      },
      "window": 0,
      "index": 2,
      "closed_at_ms": 1763200000000
    }
  ],
  "saved_at_ms": 1763200005000
}
"#;

fn example() -> SessionSnapshot {
    SessionSnapshot {
        windows: vec![WindowSnapshot {
            tabs: vec![
                TabSnapshot {
                    url: "https://example.org/".to_owned(),
                    title: "Example Domain".to_owned(),
                    zoom_percent: 100,
                    pinned: true,
                },
                TabSnapshot {
                    url: "about:blank".to_owned(),
                    title: String::new(),
                    zoom_percent: 125,
                    pinned: false,
                },
            ],
            active_tab: 1,
            bounds: Some(WindowBounds {
                x: 80,
                y: 60,
                width: 1280,
                height: 800,
            }),
            maximized: false,
            fullscreen: false,
        }],
        closed_tabs: vec![ClosedTab {
            tab: TabSnapshot {
                url: "https://www.rust-lang.org/".to_owned(),
                title: "Rust".to_owned(),
                zoom_percent: 100,
                pinned: false,
            },
            window: 0,
            index: 2,
            closed_at_ms: 1_763_200_000_000,
        }],
        saved_at_ms: 1_763_200_005_000,
    }
}

fn other() -> SessionSnapshot {
    let mut other = example();
    other.windows[0].tabs.truncate(1);
    other.windows[0].active_tab = 0;
    other.saved_at_ms += 60_000;
    other
}

fn session_path(dir: &Path) -> std::path::PathBuf {
    dir.join("session.json")
}

fn previous_path(dir: &Path) -> std::path::PathBuf {
    dir.join("session.previous.json")
}

fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .expect("the profile")
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

/// The last run's session, as `begin_run` reports it.
fn begin(dir: &Path) -> Option<PreviousSession> {
    SessionStore::new(dir).begin_run().expect("begin_run").last
}

#[test]
fn the_first_run_has_no_previous_session() {
    let dir = tempfile::tempdir().expect("a profile");

    assert_eq!(begin(dir.path()), None);
    assert!(names_in(dir.path()).is_empty());
}

#[test]
fn the_file_has_the_documented_shape() {
    let dir = tempfile::tempdir().expect("a profile");

    SessionStore::new(dir.path())
        .save(&example(), false)
        .expect("save");

    assert_eq!(
        fs::read_to_string(session_path(dir.path())).expect("read"),
        EXAMPLE_FILE
    );
}

#[cfg(unix)]
#[test]
fn the_files_are_private() {
    use std::os::unix::fs::PermissionsExt as _;

    let dir = tempfile::tempdir().expect("a profile");
    let store = SessionStore::new(dir.path());
    store.save(&example(), true).expect("save");
    let _ = store.begin_run().expect("begin_run");

    for path in [session_path(dir.path()), previous_path(dir.path())] {
        let mode = fs::metadata(&path).expect("the file").permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "{}", path.display());
    }
}

#[test]
fn an_orderly_quit_is_reported_as_clean() {
    let dir = tempfile::tempdir().expect("a profile");
    let store = SessionStore::new(dir.path());
    store.save(&other(), false).expect("a running save");
    store.save(&example(), true).expect("the final save");

    assert_eq!(
        begin(dir.path()),
        Some(PreviousSession {
            snapshot: example(),
            exited_cleanly: true
        })
    );
}

#[test]
fn a_run_whose_last_save_was_not_final_crashed() {
    let dir = tempfile::tempdir().expect("a profile");
    let store = SessionStore::new(dir.path());
    store.save(&example(), false).expect("a running save");

    assert_eq!(
        begin(dir.path()),
        Some(PreviousSession {
            snapshot: example(),
            exited_cleanly: false
        })
    );
}

#[test]
fn a_crash_right_after_start_up_is_not_mistaken_for_a_clean_exit() {
    let dir = tempfile::tempdir().expect("a profile");
    SessionStore::new(dir.path())
        .save(&example(), true)
        .expect("the final save of run 1");

    let run_2 = begin(dir.path()).expect("run 1's session");
    assert!(run_2.exited_cleanly);
    // Run 2 crashes before its first save.

    let run_3 = begin(dir.path()).expect("still run 1's session");
    assert_eq!(run_3.snapshot, example());
    assert!(!run_3.exited_cleanly, "run 2 did not quit in order");
}

#[test]
fn the_previous_session_stays_on_disk_after_new_saves() {
    let dir = tempfile::tempdir().expect("a profile");
    let store = SessionStore::new(dir.path());
    store.save(&example(), true).expect("run 1");

    let run_2 = store
        .begin_run()
        .expect("begin_run")
        .last
        .expect("run 1's session");
    store.save(&other(), false).expect("run 2 saves");
    store.save(&other(), false).expect("run 2 saves again");

    assert_eq!(run_2.snapshot, example());
    let previous: serde_json::Value =
        serde_json::from_slice(&fs::read(previous_path(dir.path())).expect("previous"))
            .expect("json");
    let expected: serde_json::Value = serde_json::from_str(EXAMPLE_FILE).expect("json");
    assert_eq!(previous["windows"], expected["windows"]);
    assert_eq!(previous["closed_tabs"], expected["closed_tabs"]);
    assert_eq!(previous["format_version"], SESSION_FORMAT_VERSION);
}

#[test]
fn a_second_crash_before_any_save_does_not_lose_the_session() {
    let dir = tempfile::tempdir().expect("a profile");
    SessionStore::new(dir.path())
        .save(&example(), false)
        .expect("run 1 crashes after this save");

    let run_2 = begin(dir.path()).expect("run 1's session");
    // Run 2 crashes before saving anything.
    let run_3 = begin(dir.path()).expect("run 1's session again");

    assert_eq!(run_2.snapshot, example());
    assert_eq!(run_3.snapshot, example());
    assert!(!run_3.exited_cleanly);
}

#[test]
fn the_session_returned_is_sanitized() {
    let dir = tempfile::tempdir().expect("a profile");
    let raw = r#"{"format_version": 1, "exited_cleanly": false, "windows": [
        {"tabs": [{"url": "javascript:alert(1)"}, {"url": "https://ok.example/", "zoom_percent": 9000}],
         "active_tab": 7}]}"#;
    fs::write(session_path(dir.path()), raw).expect("seed");

    let previous = begin(dir.path()).expect("a session");

    let window = &previous.snapshot.windows[0];
    assert_eq!(window.tabs.len(), 1);
    assert_eq!(window.tabs[0].url, "https://ok.example/");
    assert_eq!(window.tabs[0].zoom_percent, 500);
    assert_eq!(window.active_tab, 0);
}

#[test]
fn save_writes_the_sanitized_session() {
    let dir = tempfile::tempdir().expect("a profile");
    let mut session = example();
    session.windows[0].tabs.push(TabSnapshot {
        url: "data:text/html,secret".to_owned(),
        ..TabSnapshot::default()
    });

    SessionStore::new(dir.path())
        .save(&session, false)
        .expect("save");

    let text = fs::read_to_string(session_path(dir.path())).expect("read");
    assert!(!text.contains("data:"), "{text}");
}

#[test]
fn a_damaged_session_is_moved_aside_and_start_up_goes_on() {
    for damage in [
        b"{\"windows\": [".to_vec(),
        b"[]".to_vec(),
        b"not json".to_vec(),
        br#"{"windows": [{"tabs": "many"}]}"#.to_vec(),
        br#"{"exited_cleanly": "yes"}"#.to_vec(),
    ] {
        let dir = tempfile::tempdir().expect("a profile");
        fs::write(session_path(dir.path()), &damage).expect("seed");

        assert_eq!(begin(dir.path()), None);

        assert_eq!(
            names_in(dir.path()),
            vec!["session.json.damaged-1"],
            "{}",
            String::from_utf8_lossy(&damage)
        );
        assert_eq!(
            fs::read(dir.path().join("session.json.damaged-1")).expect("the backup"),
            damage
        );
    }
}

#[test]
fn a_session_over_the_size_limit_is_damaged() {
    let dir = tempfile::tempdir().expect("a profile");
    let limit = usize::try_from(MAX_SESSION_FILE_BYTES).expect("fits");
    let mut big = b"{\"windows\": []".to_vec();
    big.resize(limit, b' ');
    big.push(b'}');
    fs::write(session_path(dir.path()), &big).expect("seed");

    assert_eq!(begin(dir.path()), None);
    assert_eq!(names_in(dir.path()), vec!["session.json.damaged-1"]);
}

#[test]
fn a_newer_session_is_left_alone_and_never_overwritten() {
    let dir = tempfile::tempdir().expect("a profile");
    let newer = format!(
        r#"{{"format_version": {}, "windows": [], "tab_groups": []}}"#,
        SESSION_FORMAT_VERSION + 1
    );
    fs::write(session_path(dir.path()), &newer).expect("seed");
    let store = SessionStore::new(dir.path());

    assert_eq!(
        store.begin_run().expect("begin_run"),
        StartupSessions::default()
    );
    let saved = store.save(&example(), false);

    assert!(
        matches!(saved, Err(SettingsError::NewerVersion { found, .. }) if found == SESSION_FORMAT_VERSION + 1),
        "{saved:?}"
    );
    assert_eq!(
        fs::read_to_string(session_path(dir.path())).expect("read"),
        newer
    );
    assert_eq!(names_in(dir.path()), vec!["session.json"]);
}

#[test]
fn a_newer_previous_file_is_not_overwritten_either() {
    let dir = tempfile::tempdir().expect("a profile");
    let newer = format!(r#"{{"format_version": {}}}"#, SESSION_FORMAT_VERSION + 1);
    fs::write(previous_path(dir.path()), &newer).expect("seed");
    let store = SessionStore::new(dir.path());
    store.save(&example(), false).expect("save");

    let previous = store.begin_run().expect("begin_run").last;

    assert_eq!(previous.map(|previous| previous.snapshot), Some(example()));
    assert_eq!(
        fs::read_to_string(previous_path(dir.path())).expect("read"),
        newer
    );
}

#[test]
fn an_unversioned_session_is_read_as_version_0() {
    let dir = tempfile::tempdir().expect("a profile");
    fs::write(
        session_path(dir.path()),
        r#"{"windows": [{"tabs": [{"url": "https://example.org/"}]}]}"#,
    )
    .expect("seed");

    let previous = begin(dir.path()).expect("a session");

    assert_eq!(
        previous.snapshot.windows[0].tabs[0].url,
        "https://example.org/"
    );
    assert!(
        !previous.exited_cleanly,
        "no marker means it did not quit in order"
    );
}

#[test]
fn a_directory_at_the_session_path_is_an_error() {
    let dir = tempfile::tempdir().expect("a profile");
    fs::create_dir(session_path(dir.path())).expect("put a directory in the way");
    let store = SessionStore::new(dir.path());

    assert!(matches!(store.begin_run(), Err(SettingsError::Io { .. })));
    assert!(matches!(
        store.save(&example(), false),
        Err(SettingsError::Io { .. })
    ));
}

#[test]
fn the_largest_session_fits_under_the_file_limit() {
    // Every tab at the longest URL and title, with characters JSON must escape.
    let worst_tab = |n: usize| TabSnapshot {
        url: format!(
            "about:{n}{}",
            "\"".repeat(MAX_URL_BYTES - 6 - n.to_string().len())
        ),
        title: "\u{1}".repeat(MAX_TITLE_BYTES),
        zoom_percent: 100,
        pinned: false,
    };
    let session = SessionSnapshot {
        windows: vec![WindowSnapshot {
            tabs: (0..MAX_RESTORED_TABS).map(worst_tab).collect(),
            active_tab: 0,
            bounds: None,
            maximized: false,
            fullscreen: false,
        }],
        closed_tabs: (0..MAX_CLOSED_TABS)
            .map(|n| ClosedTab {
                tab: worst_tab(n),
                window: usize::MAX,
                index: usize::MAX,
                closed_at_ms: i64::MIN,
            })
            .collect(),
        saved_at_ms: i64::MAX,
    };
    assert_eq!(session.clone().sanitized(), session);

    let dir = tempfile::tempdir().expect("a profile");
    SessionStore::new(dir.path())
        .save(&session, false)
        .expect("save");

    let size = fs::metadata(session_path(dir.path()))
        .expect("the file")
        .len();
    assert!(size <= MAX_SESSION_FILE_BYTES, "{size} bytes");
    assert_eq!(
        begin(dir.path()).map(|previous| previous.snapshot),
        Some(session)
    );
}

#[test]
fn save_keeps_a_copy_of_a_damaged_session_file_it_finds() {
    let dir = tempfile::tempdir().expect("a profile");
    fs::write(session_path(dir.path()), b"{\"windows\": [ truncated").expect("seed");

    SessionStore::new(dir.path())
        .save(&example(), false)
        .expect("save");

    assert_eq!(
        names_in(dir.path()),
        vec!["session.json", "session.json.damaged-1"]
    );
    assert_eq!(
        fs::read(dir.path().join("session.json.damaged-1")).expect("the copy"),
        b"{\"windows\": [ truncated"
    );
    assert_eq!(
        fs::read_to_string(session_path(dir.path())).expect("read"),
        EXAMPLE_FILE
    );
}

#[test]
fn a_newer_session_larger_than_the_limit_is_still_newer() {
    let dir = tempfile::tempdir().expect("a profile");
    let limit = usize::try_from(MAX_SESSION_FILE_BYTES).expect("fits");
    let mut newer = format!(
        r#"{{"format_version": {}, "windows": []"#,
        SESSION_FORMAT_VERSION + 1
    )
    .into_bytes();
    newer.resize(limit + 10, b' ');
    newer.push(b'}');
    fs::write(session_path(dir.path()), &newer).expect("seed");
    let store = SessionStore::new(dir.path());

    assert_eq!(
        store.begin_run().expect("begin_run"),
        StartupSessions::default()
    );
    assert!(matches!(
        store.save(&example(), false),
        Err(SettingsError::NewerVersion { .. })
    ));
    assert_eq!(names_in(dir.path()), vec!["session.json"]);
    assert_eq!(fs::read(session_path(dir.path())).expect("read"), newer);
}

#[test]
fn a_version_beyond_u32_is_newer_not_damaged() {
    let dir = tempfile::tempdir().expect("a profile");
    let newer = r#"{"format_version": 99999999999999999999999, "windows": []}"#;
    fs::write(session_path(dir.path()), newer).expect("seed");
    let store = SessionStore::new(dir.path());

    assert_eq!(
        store.begin_run().expect("begin_run"),
        StartupSessions::default()
    );
    let saved = store.save(&example(), false);

    assert!(
        matches!(
            saved,
            Err(SettingsError::NewerVersion {
                found: u32::MAX,
                ..
            })
        ),
        "{saved:?}"
    );
    assert_eq!(
        fs::read_to_string(session_path(dir.path())).expect("read"),
        newer
    );
}

#[cfg(unix)]
#[test]
fn the_recovery_file_is_private() {
    use std::os::unix::fs::PermissionsExt as _;

    let dir = tempfile::tempdir().expect("a profile");
    let store = SessionStore::new(dir.path());
    store.save(&example(), false).expect("a crashed run's save");
    let _ = store.begin_run().expect("begin_run");

    let path = dir.path().join("session.recovery.json");
    let mode = fs::metadata(&path).expect("the slot").permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn credentials_never_reach_the_session_file() {
    let dir = tempfile::tempdir().expect("a profile");
    let mut session = example();
    session.windows[0].tabs[0].url = "https://alice:hunter2@example.org/inbox".to_owned();
    session.closed_tabs[0].tab.url = "https://bob:swordfish@www.rust-lang.org/".to_owned();

    SessionStore::new(dir.path())
        .save(&session, false)
        .expect("save");

    let text = fs::read_to_string(session_path(dir.path())).expect("read");
    assert!(
        !text.contains("hunter2") && !text.contains("swordfish"),
        "{text}"
    );
    assert!(!text.contains("alice") && !text.contains("bob@"), "{text}");
    assert!(text.contains("https://example.org/inbox"), "{text}");
}
