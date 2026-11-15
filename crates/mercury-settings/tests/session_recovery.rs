//! A crashed session stays recoverable until the application says the user dealt with it.
//! Each test walks a sequence of runs and checks what `begin_run` returns and what is on disk
//! after every step.

use std::fs;
use std::path::Path;

use mercury_settings::{
    PreviousSession, SESSION_FORMAT_VERSION, SessionSnapshot, SessionStore, SettingsError,
    StartupSessions, TabSnapshot, WindowSnapshot,
};

fn session_of(name: &str, tabs: usize) -> SessionSnapshot {
    SessionSnapshot {
        windows: vec![WindowSnapshot {
            tabs: (0..tabs)
                .map(|n| TabSnapshot {
                    url: format!("https://{name}.example/{n}"),
                    ..TabSnapshot::default()
                })
                .collect(),
            ..WindowSnapshot::default()
        }],
        ..SessionSnapshot::default()
    }
}

fn forty() -> SessionSnapshot {
    session_of("forty", 40)
}

fn one() -> SessionSnapshot {
    session_of("one", 1)
}

/// What a session file on disk holds: the first tab's host and the tab count, and the
/// clean-exit mark.
#[derive(Debug, PartialEq, Eq)]
struct OnDisk {
    first_url: String,
    tabs: usize,
    exited_cleanly: bool,
}

fn on_disk(path: &Path) -> Option<OnDisk> {
    let text = fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
    let tabs = value["windows"][0]["tabs"].as_array().expect("tabs");
    Some(OnDisk {
        first_url: tabs[0]["url"].as_str().expect("a URL").to_owned(),
        tabs: tabs.len(),
        exited_cleanly: value["exited_cleanly"].as_bool().expect("the mark"),
    })
}

fn expect(snapshot: &SessionSnapshot, exited_cleanly: bool) -> Option<OnDisk> {
    Some(OnDisk {
        first_url: snapshot.windows[0].tabs[0].url.clone(),
        tabs: snapshot.windows[0].tabs.len(),
        exited_cleanly,
    })
}

struct Profile {
    dir: tempfile::TempDir,
    store: SessionStore,
}

impl Profile {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("a profile");
        let store = SessionStore::new(dir.path());
        Self { dir, store }
    }

    fn start(&self) -> StartupSessions {
        self.store.begin_run().expect("begin_run")
    }

    fn save(&self, snapshot: &SessionSnapshot, exited_cleanly: bool) {
        self.store
            .save(snapshot, exited_cleanly)
            .expect("save the running session");
    }

    fn current(&self) -> Option<OnDisk> {
        on_disk(&self.dir.path().join("session.json"))
    }

    fn previous(&self) -> Option<OnDisk> {
        on_disk(&self.dir.path().join("session.previous.json"))
    }

    fn recovery(&self) -> Option<OnDisk> {
        on_disk(&self.dir.path().join("session.recovery.json"))
    }
}

fn crashed(snapshot: SessionSnapshot) -> Option<PreviousSession> {
    Some(PreviousSession {
        snapshot,
        exited_cleanly: false,
    })
}

fn clean(snapshot: SessionSnapshot) -> Option<PreviousSession> {
    Some(PreviousSession {
        snapshot,
        exited_cleanly: true,
    })
}

#[test]
fn a_crash_the_user_never_answered_survives_a_second_crash() {
    let profile = Profile::new();

    // Run 1: forty tabs, then a crash.
    profile.save(&forty(), false);

    // Run 2 starts on a new tab and shows the restore offer.
    let run_2 = profile.start();
    assert_eq!(run_2.last, crashed(forty()));
    assert_eq!(run_2.recovery, Some(forty()));
    assert_eq!(profile.recovery(), expect(&forty(), false));
    assert_eq!(profile.previous(), expect(&forty(), false));

    // Its one-tab session is saved a few seconds later; then it crashes.
    profile.save(&one(), false);
    assert_eq!(profile.current(), expect(&one(), false));
    assert_eq!(profile.recovery(), expect(&forty(), false));

    // Run 3: the forty tabs are still offered.
    let run_3 = profile.start();
    assert_eq!(run_3.last, crashed(one()));
    assert_eq!(run_3.recovery, Some(forty()));
    assert_eq!(profile.recovery(), expect(&forty(), false));
    assert_eq!(profile.previous(), expect(&one(), false));
}

#[test]
fn a_crash_the_user_never_answered_survives_a_clean_quit() {
    let profile = Profile::new();
    profile.save(&forty(), false);

    let run_2 = profile.start();
    assert_eq!(run_2.recovery, Some(forty()));
    profile.save(&one(), false);
    // Run 2 quits in order before the user answers.
    profile.save(&one(), true);
    assert_eq!(profile.current(), expect(&one(), true));

    let run_3 = profile.start();
    assert_eq!(run_3.last, clean(one()), "continue where you left off");
    assert_eq!(run_3.recovery, Some(forty()), "restore after the crash");
    assert_eq!(profile.recovery(), expect(&forty(), false));
    assert_eq!(profile.previous(), expect(&one(), true));
    assert_eq!(
        profile.current(),
        expect(&one(), false),
        "marked as running again"
    );
}

#[test]
fn three_crashes_in_a_row_keep_the_first() {
    let profile = Profile::new();
    let first = session_of("first", 40);
    let second = session_of("second", 2);
    let third = session_of("third", 3);

    profile.save(&first, false);

    let run_2 = profile.start();
    assert_eq!(run_2.last, crashed(first.clone()));
    assert_eq!(run_2.recovery, Some(first.clone()));
    assert_eq!(profile.recovery(), expect(&first, false));
    profile.save(&second, false);

    let run_3 = profile.start();
    assert_eq!(run_3.last, crashed(second.clone()));
    assert_eq!(run_3.recovery, Some(first.clone()));
    assert_eq!(profile.recovery(), expect(&first, false));
    profile.save(&third, false);

    let run_4 = profile.start();
    assert_eq!(run_4.last, crashed(third.clone()));
    assert_eq!(run_4.recovery, Some(first.clone()));
    assert_eq!(profile.recovery(), expect(&first, false));
    assert_eq!(profile.previous(), expect(&third, false));
}

#[test]
fn a_restored_and_resolved_crash_is_offered_no_more() {
    let profile = Profile::new();
    profile.save(&forty(), false);

    let run_2 = profile.start();
    assert_eq!(run_2.recovery, Some(forty()));
    // The user restores; the application says so.
    profile.store.resolve_recovery().expect("resolve");
    assert_eq!(profile.recovery(), None);
    profile.save(&forty(), false);
    profile.save(&forty(), true);

    let run_3 = profile.start();
    assert_eq!(run_3.last, clean(forty()));
    assert_eq!(run_3.recovery, None);
    assert_eq!(profile.recovery(), None);
}

#[test]
fn a_dismissed_crash_is_replaced_by_the_next_one() {
    let profile = Profile::new();
    profile.save(&forty(), false);
    profile.start();
    profile.store.resolve_recovery().expect("dismiss");
    profile.save(&one(), false);

    let run_3 = profile.start();

    assert_eq!(run_3.recovery, Some(one()));
    assert_eq!(profile.recovery(), expect(&one(), false));
}

#[test]
fn a_damaged_current_file_with_a_pending_recovery_keeps_the_recovery() {
    let profile = Profile::new();
    profile.save(&forty(), false);
    profile.start();
    profile.save(&one(), false);
    // A power cut leaves session.json damaged.
    fs::write(
        profile.dir.path().join("session.json"),
        b"{\"windows\": [{\"ta",
    )
    .expect("damage");

    let run_3 = profile.start();

    assert_eq!(run_3.recovery, Some(forty()));
    assert_eq!(
        run_3.last,
        crashed(forty()),
        "the previous copy, marked as not having quit in order"
    );
    assert_eq!(profile.recovery(), expect(&forty(), false));
    assert_eq!(profile.current(), None);
    assert_eq!(
        fs::read(profile.dir.path().join("session.json.damaged-1")).expect("kept aside"),
        b"{\"windows\": [{\"ta"
    );
}

#[test]
fn a_damaged_current_file_offers_the_previous_copy_and_keeps_it() {
    let profile = Profile::new();
    profile.save(&forty(), true);
    let run_2 = profile.start();
    assert_eq!(run_2.last, clean(forty()));
    assert_eq!(run_2.recovery, None);
    profile.save(&one(), false);
    fs::write(profile.dir.path().join("session.json"), b"\0\0\0\0").expect("damage");

    let run_3 = profile.start();

    assert_eq!(run_3.last, crashed(forty()));
    assert_eq!(run_3.recovery, Some(forty()));
    assert_eq!(profile.recovery(), expect(&forty(), false));
    assert!(profile.dir.path().join("session.json.damaged-1").exists());

    // Run 3 saves and crashes; the forty tabs are still there for run 4.
    profile.save(&one(), false);
    let run_4 = profile.start();
    assert_eq!(run_4.recovery, Some(forty()));
}

#[test]
fn a_crash_with_nothing_to_restore_is_not_offered() {
    let profile = Profile::new();
    let nothing = SessionSnapshot {
        windows: vec![WindowSnapshot {
            tabs: vec![TabSnapshot {
                url: "javascript:void(0)".to_owned(),
                ..TabSnapshot::default()
            }],
            ..WindowSnapshot::default()
        }],
        ..SessionSnapshot::default()
    };
    profile.save(&nothing, false);

    let run_2 = profile.start();

    assert_eq!(run_2.recovery, None);
    assert_eq!(profile.recovery(), None);
}

#[test]
fn resolving_with_nothing_pending_is_fine() {
    let profile = Profile::new();
    profile.store.resolve_recovery().expect("nothing to do");
    assert!(
        fs::read_dir(profile.dir.path())
            .expect("list")
            .next()
            .is_none()
    );
}

#[test]
fn a_newer_recovery_slot_is_neither_offered_nor_replaced_nor_removed() {
    let profile = Profile::new();
    let slot = profile.dir.path().join("session.recovery.json");
    let newer = format!(r#"{{"format_version": {}}}"#, SESSION_FORMAT_VERSION + 1);
    fs::write(&slot, &newer).expect("seed");
    profile.save(&forty(), false);

    let run_2 = profile.start();

    assert_eq!(run_2.recovery, None);
    assert_eq!(run_2.last, crashed(forty()));
    assert!(matches!(
        profile.store.resolve_recovery(),
        Err(SettingsError::NewerVersion { .. })
    ));
    assert_eq!(fs::read_to_string(&slot).expect("untouched"), newer);
}

#[test]
fn a_damaged_recovery_slot_is_kept_aside_and_refilled() {
    let profile = Profile::new();
    let slot = profile.dir.path().join("session.recovery.json");
    fs::write(&slot, b"garbage").expect("seed");
    profile.save(&forty(), false);

    let run_2 = profile.start();

    assert_eq!(run_2.recovery, Some(forty()));
    assert_eq!(profile.recovery(), expect(&forty(), false));
    assert_eq!(
        fs::read(profile.dir.path().join("session.recovery.json.damaged-1")).expect("kept"),
        b"garbage"
    );
}
