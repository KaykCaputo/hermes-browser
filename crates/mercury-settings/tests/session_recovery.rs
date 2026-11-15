//! Every crashed session stays recoverable until the application says the user dealt with it.
//! Each test walks a sequence of runs and checks what `begin_run` returns and what is on disk
//! after every step.

use std::fs;
use std::path::Path;

use mercury_settings::{
    MAX_RECOVERY_SESSIONS, PreviousSession, SESSION_FORMAT_VERSION, SessionSnapshot, SessionStore,
    SettingsError, StartupSessions, TabSnapshot, WindowSnapshot,
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

fn ten() -> SessionSnapshot {
    session_of("ten", 10)
}

/// A session file on disk, summarised: the first tab's URL, the tab count and the mark.
#[derive(Debug, PartialEq, Eq)]
struct OnDisk {
    first_url: String,
    tabs: usize,
    exited_cleanly: bool,
}

fn summary(value: &serde_json::Value) -> (String, usize) {
    let tabs = value["windows"][0]["tabs"].as_array().expect("tabs");
    (
        tabs[0]["url"].as_str().expect("a URL").to_owned(),
        tabs.len(),
    )
}

fn on_disk(path: &Path) -> Option<OnDisk> {
    let text = fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
    let (first_url, tabs) = summary(&value);
    Some(OnDisk {
        first_url,
        tabs,
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

/// The first URL and tab count of each session in the recovery file, in file order.
fn listed(snapshots: &[SessionSnapshot]) -> Vec<(String, usize)> {
    snapshots
        .iter()
        .map(|snapshot| {
            (
                snapshot.windows[0].tabs[0].url.clone(),
                snapshot.windows[0].tabs.len(),
            )
        })
        .collect()
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

    fn path(&self, name: &str) -> std::path::PathBuf {
        self.dir.path().join(name)
    }

    fn current(&self) -> Option<OnDisk> {
        on_disk(&self.path("session.json"))
    }

    fn previous(&self) -> Option<OnDisk> {
        on_disk(&self.path("session.previous.json"))
    }

    /// The recovery file's sessions, summarised; `None` when there is no file.
    fn recovery(&self) -> Option<Vec<(String, usize)>> {
        let text = fs::read_to_string(self.path("session.recovery.json")).ok()?;
        let value: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
        assert_eq!(value["format_version"], 1);
        Some(
            value["sessions"]
                .as_array()
                .expect("a list of sessions")
                .iter()
                .map(summary)
                .collect(),
        )
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
fn an_ignored_offer_and_a_second_crash_keep_both_sessions() {
    let profile = Profile::new();

    // Run 1: forty tabs, then a crash.
    profile.save(&forty(), false);

    // Run 2 offers the forty; the user ignores it and opens ten new tabs; then a crash.
    let run_2 = profile.start();
    assert_eq!(run_2.last, crashed(forty()));
    assert_eq!(run_2.recovery, vec![forty()]);
    assert_eq!(profile.recovery(), Some(listed(&[forty()])));
    assert_eq!(profile.previous(), expect(&forty(), false));
    profile.save(&ten(), false);
    assert_eq!(profile.current(), expect(&ten(), false));
    assert_eq!(profile.recovery(), Some(listed(&[forty()])));

    // Run 3 offers both, newest first.
    let run_3 = profile.start();
    assert_eq!(run_3.last, crashed(ten()));
    assert_eq!(run_3.recovery, vec![ten(), forty()]);
    assert_eq!(profile.recovery(), Some(listed(&[ten(), forty()])));
    assert_eq!(profile.previous(), expect(&ten(), false));

    // Run 3 crashes before saving; run 4 still offers both, once each.
    let run_4 = profile.start();
    assert_eq!(run_4.recovery, vec![ten(), forty()]);
    assert_eq!(profile.recovery(), Some(listed(&[ten(), forty()])));
}

#[test]
fn four_crashes_keep_the_first_and_the_two_newest() {
    let profile = Profile::new();
    let sessions: Vec<SessionSnapshot> = ["a", "b", "c", "d"]
        .iter()
        .enumerate()
        .map(|(n, name)| session_of(name, n + 1))
        .collect();
    let [a, b, c, d] = [0, 1, 2, 3].map(|n| sessions[n].clone());
    assert_eq!(MAX_RECOVERY_SESSIONS, 3);

    profile.save(&a, false);
    assert_eq!(profile.start().recovery, vec![a.clone()]);
    assert_eq!(profile.recovery(), Some(listed(std::slice::from_ref(&a))));

    profile.save(&b, false);
    assert_eq!(profile.start().recovery, vec![b.clone(), a.clone()]);
    assert_eq!(profile.recovery(), Some(listed(&[b.clone(), a.clone()])));

    profile.save(&c, false);
    assert_eq!(
        profile.start().recovery,
        vec![c.clone(), b.clone(), a.clone()]
    );
    assert_eq!(
        profile.recovery(),
        Some(listed(&[c.clone(), b.clone(), a.clone()]))
    );

    // Full: the oldest (a) stays, b — the oldest of the others — gives way to d.
    profile.save(&d, false);
    let run_5 = profile.start();
    assert_eq!(run_5.recovery, vec![d.clone(), c.clone(), a.clone()]);
    assert_eq!(profile.recovery(), Some(listed(&[d, c, a])));
}

#[test]
fn a_crash_loop_of_empty_sessions_leaves_a_real_one_alone() {
    let profile = Profile::new();
    profile.save(&forty(), false);
    let _ = profile.start();
    let slot = profile.path("session.recovery.json");
    let before = fs::read(&slot).expect("the recovery file");

    let only_new_tabs = SessionSnapshot {
        windows: vec![WindowSnapshot {
            tabs: ["about:newtab", "about:blank", "about:home"]
                .map(|url| TabSnapshot {
                    url: url.to_owned(),
                    ..TabSnapshot::default()
                })
                .to_vec(),
            ..WindowSnapshot::default()
        }],
        ..SessionSnapshot::default()
    };
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
    for empty in [
        &only_new_tabs,
        &nothing,
        &SessionSnapshot::default(),
        &only_new_tabs,
    ] {
        profile.save(empty, false);
        let run = profile.start();
        assert_eq!(run.recovery, vec![forty()]);
        assert_eq!(fs::read(&slot).expect("the recovery file"), before);
    }
}

#[test]
fn identical_consecutive_crashes_are_kept_once() {
    let profile = Profile::new();
    profile.save(&forty(), false);
    let _ = profile.start();

    // The next run shows exactly the same tabs, saved at a later time, and crashes too.
    let again = SessionSnapshot {
        saved_at_ms: 99,
        ..forty()
    };
    profile.save(&again, false);
    let run_3 = profile.start();

    assert_eq!(run_3.recovery, vec![forty()]);
    assert_eq!(profile.recovery(), Some(listed(&[forty()])));
}

#[test]
fn resolving_clears_every_entry() {
    let profile = Profile::new();
    profile.save(&forty(), false);
    let _ = profile.start();
    profile.save(&ten(), false);
    assert_eq!(profile.start().recovery.len(), 2);

    profile.store.resolve_recovery().expect("resolve");

    assert_eq!(profile.recovery(), None);
    profile.save(&ten(), true);
    let run = profile.start();
    assert_eq!(run.recovery, Vec::<SessionSnapshot>::new());
    assert_eq!(run.last, clean(ten()));
}

#[test]
fn a_clean_quit_between_crashes_leaves_the_list_alone() {
    let profile = Profile::new();
    profile.save(&forty(), false);
    let _ = profile.start();

    // Run 2 quits in order without the user answering.
    profile.save(&ten(), true);
    let run_3 = profile.start();
    assert_eq!(run_3.last, clean(ten()), "continue where you left off");
    assert_eq!(run_3.recovery, vec![forty()], "restore after the crash");
    assert_eq!(profile.recovery(), Some(listed(&[forty()])));
    assert_eq!(
        profile.current(),
        expect(&ten(), false),
        "marked as running"
    );

    // Run 3 crashes with something new.
    let other = session_of("other", 2);
    profile.save(&other, false);
    let run_4 = profile.start();
    assert_eq!(run_4.recovery, vec![other.clone(), forty()]);
    assert_eq!(profile.recovery(), Some(listed(&[other, forty()])));
}

#[test]
fn a_restored_and_resolved_crash_is_offered_no_more() {
    let profile = Profile::new();
    profile.save(&forty(), false);

    let run_2 = profile.start();
    assert_eq!(run_2.recovery, vec![forty()]);
    profile.store.resolve_recovery().expect("resolve");
    assert_eq!(profile.recovery(), None);
    profile.save(&forty(), false);
    profile.save(&forty(), true);

    let run_3 = profile.start();
    assert_eq!(run_3.last, clean(forty()));
    assert!(run_3.recovery.is_empty());
    assert_eq!(profile.recovery(), None);
}

#[test]
fn a_damaged_current_file_with_pending_recoveries_keeps_them() {
    let profile = Profile::new();
    profile.save(&forty(), false);
    let _ = profile.start();
    profile.save(&ten(), false);
    fs::write(profile.path("session.json"), b"{\"windows\": [{\"ta").expect("damage");

    let run_3 = profile.start();

    // The previous copy (forty) is offered as crashed; it is already in the list.
    assert_eq!(run_3.last, crashed(forty()));
    assert_eq!(run_3.recovery, vec![forty()]);
    assert_eq!(profile.recovery(), Some(listed(&[forty()])));
    assert_eq!(profile.current(), None);
    assert_eq!(
        fs::read(profile.path("session.json.damaged-1")).expect("kept aside"),
        b"{\"windows\": [{\"ta"
    );
}

#[test]
fn a_damaged_current_file_offers_the_previous_copy_and_keeps_it() {
    let profile = Profile::new();
    profile.save(&forty(), true);
    let run_2 = profile.start();
    assert_eq!(run_2.last, clean(forty()));
    assert!(run_2.recovery.is_empty());
    profile.save(&ten(), false);
    fs::write(profile.path("session.json"), b"\0\0\0\0").expect("damage");

    let run_3 = profile.start();

    assert_eq!(run_3.last, crashed(forty()));
    assert_eq!(run_3.recovery, vec![forty()]);
    assert_eq!(profile.recovery(), Some(listed(&[forty()])));
    assert!(profile.path("session.json.damaged-1").exists());

    profile.save(&ten(), false);
    let run_4 = profile.start();
    assert_eq!(run_4.recovery, vec![ten(), forty()]);
}

#[test]
fn a_session_only_in_the_previous_file_is_found() {
    let profile = Profile::new();
    profile.save(&forty(), true);
    let _ = profile.start();
    assert_eq!(profile.previous(), expect(&forty(), true));
    // A crash after session.json was moved aside and before anything replaced it.
    fs::remove_file(profile.path("session.json")).expect("remove");

    let run_3 = profile.start();

    assert_eq!(run_3.last, crashed(forty()));
    assert_eq!(run_3.recovery, vec![forty()]);
    assert_eq!(profile.recovery(), Some(listed(&[forty()])));
    assert_eq!(profile.previous(), expect(&forty(), true), "left as it was");
    assert_eq!(profile.current(), None);
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
fn a_newer_recovery_file_is_neither_replaced_nor_removed() {
    let profile = Profile::new();
    let slot = profile.path("session.recovery.json");
    let newer = format!(r#"{{"format_version": {}}}"#, SESSION_FORMAT_VERSION + 1);
    fs::write(&slot, &newer).expect("seed");
    profile.save(&forty(), false);

    let run_2 = profile.start();

    // The crash is still offered for this run, but it cannot be kept on disk.
    assert_eq!(run_2.last, crashed(forty()));
    assert_eq!(run_2.recovery, vec![forty()]);
    assert!(matches!(
        profile.store.resolve_recovery(),
        Err(SettingsError::NewerVersion { .. })
    ));
    assert_eq!(fs::read_to_string(&slot).expect("untouched"), newer);
}

#[test]
fn a_damaged_recovery_file_is_kept_aside_and_refilled() {
    let profile = Profile::new();
    let slot = profile.path("session.recovery.json");
    fs::write(&slot, b"garbage").expect("seed");
    profile.save(&forty(), false);

    let run_2 = profile.start();

    assert_eq!(run_2.recovery, vec![forty()]);
    assert_eq!(profile.recovery(), Some(listed(&[forty()])));
    assert_eq!(
        fs::read(profile.path("session.recovery.json.damaged-1")).expect("kept"),
        b"garbage"
    );
}

#[test]
fn the_recovery_file_bounds_each_session_on_its_own() {
    let profile = Profile::new();
    let big = |name: &str| session_of(name, 500);
    for name in ["one", "two", "three"] {
        profile.save(&big(name), false);
        let _ = profile.start();
    }

    let run = profile.start();

    assert_eq!(
        run.recovery
            .iter()
            .map(|s| s.windows[0].tabs.len())
            .collect::<Vec<_>>(),
        vec![500, 500, 500],
        "a shared tab budget would have starved the older entries"
    );
}
