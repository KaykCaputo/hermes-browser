use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use mercury_store::{
    BookmarkImport, BookmarkKind, BookmarkRoot, DownloadState, NewDownload, Store, StoreError,
    VisitTransition,
};
use talaria_core::{NavigationOrigin, VisitKey, VisitUpdate};

use super::bookmarks::new_guid;
use super::downloads::DownloadStart;
use super::import::ImportVisit;
use super::*;

const DAY_MS: i64 = 86_400_000;
const NOW: i64 = 1_800_000_000_000;

/// A service on a database file in a fresh directory, with a controllable clock. The test
/// thread plays the UI thread: `settle` waits for wake-ups and delivers replies.
struct Harness {
    data: ProfileData,
    wakes: Receiver<()>,
    clock: Arc<AtomicI64>,
    directory: tempfile::TempDir,
}

impl Harness {
    fn new() -> Self {
        Self::with(|_| {}, Timing::default())
    }

    /// `prepare` runs on the database file before the service opens it.
    fn with(prepare: impl FnOnce(&Path), timing: Timing) -> Self {
        let directory = tempfile::tempdir().unwrap();
        prepare(&directory.path().join("places.sqlite"));
        Self::spawn_in(directory, timing)
    }

    fn spawn_in(directory: tempfile::TempDir, timing: Timing) -> Self {
        let path = directory.path().join("places.sqlite");
        let clock = Arc::new(AtomicI64::new(NOW));
        let (wake, wakes) = mpsc::channel();
        let read_clock = Arc::clone(&clock);
        let data = ProfileData::spawn(
            move || open_database(Some(path)),
            Arc::new(move || read_clock.load(Ordering::SeqCst)),
            Arc::new(move || {
                let _sent = wake.send(());
            }),
            timing,
        );
        Self {
            data,
            wakes,
            clock,
            directory,
        }
    }

    fn path(&self) -> PathBuf {
        self.directory.path().join("places.sqlite")
    }

    /// Delivers replies until `done` holds.
    fn settle(&self, done: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done() {
            let left = deadline.saturating_duration_since(Instant::now());
            self.wakes
                .recv_timeout(left)
                .expect("the service should answer");
            self.data.deliver_replies();
        }
    }

    /// Asks for `request`'s reply and waits for it.
    fn ask<T: 'static>(
        &self,
        request: impl FnOnce(&ProfileData, Box<dyn FnOnce(Result<T, ProfileDataError>)>),
    ) -> Result<T, ProfileDataError> {
        let slot = Rc::new(RefCell::new(None));
        let fill = Rc::clone(&slot);
        request(
            &self.data,
            Box::new(move |result| *fill.borrow_mut() = Some(result)),
        );
        self.settle(|| slot.borrow().is_some());
        slot.borrow_mut().take().unwrap()
    }

    /// Shuts the service down and opens the file directly.
    fn reopen(self) -> (Store, tempfile::TempDir) {
        self.data.shut_down(SHUTDOWN_TIMEOUT);
        let store = Store::open(&self.path()).unwrap();
        (store, self.directory)
    }
}

fn visit(key: u64, url: &str, origin: NavigationOrigin) -> VisitUpdate {
    VisitUpdate::Visit {
        key: VisitKey::new(key),
        url: url.to_owned(),
        origin,
    }
}

fn stale_scores(path: &Path, now: i64) -> i64 {
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row(
            "SELECT count(*) FROM places WHERE frecency_at <= ?1",
            [now - DAY_MS],
            |row| row.get(0),
        )
        .unwrap()
}

#[test]
fn replies_arrive_in_request_order_on_the_calling_thread() {
    let harness = Harness::new();
    let log = Rc::new(RefCell::new(Vec::new()));
    let data = &harness.data;

    let first = Rc::clone(&log);
    data.bookmark_root(BookmarkRoot::Toolbar, move |root| {
        first.borrow_mut().push(format!("root {:?}", root.is_ok()));
    });
    let second = Rc::clone(&log);
    data.downloads(10, move |records| {
        second
            .borrow_mut()
            .push(format!("downloads {}", records.unwrap().len()));
    });
    let third = Rc::clone(&log);
    data.bookmark_root(BookmarkRoot::Other, move |root| {
        third.borrow_mut().push(format!("other {:?}", root.is_ok()));
    });
    assert!(
        log.borrow().is_empty(),
        "replies only arrive when delivered"
    );

    harness.settle(|| log.borrow().len() == 3);
    assert_eq!(
        *log.borrow(),
        vec!["root true", "downloads 0", "other true"]
    );
}

#[test]
fn a_reply_may_send_another_request() {
    let harness = Harness::new();
    let children = Rc::new(RefCell::new(None));
    let data = harness.data.clone();
    let slot = Rc::clone(&children);

    harness
        .data
        .bookmark_root(BookmarkRoot::Toolbar, move |root| {
            data.bookmark_children(root.unwrap(), move |nodes| {
                *slot.borrow_mut() = Some(nodes.unwrap().len());
            });
        });

    harness.settle(|| children.borrow().is_some());
    assert_eq!(*children.borrow(), Some(0));
}

#[test]
fn recorded_visits_titles_and_transitions_reach_the_database() {
    let harness = Harness::new();
    harness.data.record_updates(
        vec![
            visit(0, "https://a.test/", NavigationOrigin::Typed),
            VisitUpdate::Title {
                url: "https://a.test/".to_owned(),
                title: "Page A".to_owned(),
            },
            visit(1, "https://b.test/", NavigationOrigin::Link),
            visit(2, "https://b.test/", NavigationOrigin::Reload),
            visit(3, "https://c.test/", NavigationOrigin::History),
            visit(4, "https://d.test/", NavigationOrigin::Bookmark),
        ],
        NOW,
    );

    let (store, _directory) = harness.reopen();
    let entries = store.history(None, None, 10).unwrap();
    let mut seen = entries
        .iter()
        .map(|e| (e.url.as_str(), e.title.as_deref(), e.transition))
        .collect::<Vec<_>>();
    seen.reverse();
    assert_eq!(
        seen,
        vec![
            ("https://a.test/", Some("Page A"), VisitTransition::Typed),
            ("https://b.test/", None, VisitTransition::Link),
            ("https://b.test/", None, VisitTransition::Reload),
            ("https://c.test/", None, VisitTransition::Link),
            ("https://d.test/", None, VisitTransition::Bookmark),
        ]
    );
    assert!(entries.iter().all(|e| e.visited_at_ms == NOW));
}

#[test]
fn a_forgotten_visit_is_removed_with_its_page() {
    let harness = Harness::new();
    harness.data.record_updates(
        vec![
            visit(7, "https://a.test/", NavigationOrigin::Link),
            visit(8, "https://down.test/", NavigationOrigin::Typed),
            VisitUpdate::Forget {
                key: VisitKey::new(8),
            },
            VisitUpdate::Forget {
                key: VisitKey::new(99),
            },
        ],
        NOW,
    );

    let (store, _directory) = harness.reopen();
    let urls = store
        .history(None, None, 10)
        .unwrap()
        .into_iter()
        .map(|e| e.url)
        .collect::<Vec<_>>();
    assert_eq!(urls, vec!["https://a.test/"]);
    assert!(store.suggest("down", 5, NOW).unwrap().is_empty());
}

#[test]
fn only_recent_visits_can_be_forgotten() {
    let harness = Harness::new();
    let updates = (0..=RECENT_VISITS as u64)
        .map(|key| visit(key, &format!("https://{key}.test/"), NavigationOrigin::Link))
        .chain([VisitUpdate::Forget {
            key: VisitKey::new(0),
        }])
        .collect();
    harness.data.record_updates(updates, NOW);

    let (store, _directory) = harness.reopen();
    assert_eq!(
        store.history(None, None, 100).unwrap().len(),
        RECENT_VISITS + 1
    );
}

#[test]
fn a_refused_write_is_logged_and_later_requests_still_run() {
    let harness = Harness::new();
    let too_long = format!(
        "https://a.test/{}",
        "x".repeat(mercury_store::MAX_URL_BYTES)
    );
    harness
        .data
        .record_updates(vec![visit(0, &too_long, NavigationOrigin::Link)], NOW);
    harness.data.record_updates(
        vec![visit(1, "https://ok.test/", NavigationOrigin::Link)],
        NOW,
    );

    let found = harness
        .ask(|data, reply| data.bookmark_for_url(too_long.clone(), reply))
        .unwrap();
    assert_eq!(found, None);
    let (store, _directory) = harness.reopen();
    assert_eq!(store.history(None, None, 10).unwrap().len(), 1);
}

#[test]
fn shutting_down_finishes_every_queued_request_and_closes_the_database() {
    let harness = Harness::new();
    for key in 0..500 {
        harness.data.record_updates(
            vec![visit(
                key,
                &format!("https://{key}.test/"),
                NavigationOrigin::Link,
            )],
            NOW,
        );
    }

    let path = harness.path();
    harness.data.shut_down(SHUTDOWN_TIMEOUT);

    // Closed: the last connection checkpointed and removed the write-ahead log.
    assert!(!path.with_extension("sqlite-wal").exists());
    let store = Store::open(&path).unwrap();
    assert_eq!(store.history(None, None, 1000).unwrap().len(), 500);
}

#[test]
fn requests_after_shutdown_are_dropped() {
    let harness = Harness::new();
    harness.data.shut_down(SHUTDOWN_TIMEOUT);

    let answered = Rc::new(RefCell::new(false));
    let flag = Rc::clone(&answered);
    harness
        .data
        .downloads(1, move |_| *flag.borrow_mut() = true);
    harness
        .data
        .set_page_title("https://a.test/".into(), "A".into());
    harness.data.deliver_replies();

    assert!(!*answered.borrow());
    assert!(harness.data.shared.pending.borrow().is_empty());
}

fn assert_unavailable(harness: &Harness, reason_part: &str) {
    let error = harness
        .ask(|data, reply| data.bookmark_root(BookmarkRoot::Toolbar, reply))
        .unwrap_err();
    match &error {
        ProfileDataError::Unavailable(reason) => {
            assert!(reason.contains(reason_part), "{reason}");
        }
        other => panic!("expected the database to be unavailable, got {other:?}"),
    }
    // Writes without a reply are dropped, and the service keeps answering.
    harness.data.record_updates(
        vec![visit(0, "https://a.test/", NavigationOrigin::Link)],
        NOW,
    );
    assert!(matches!(
        harness.ask(|data, reply| data.downloads(5, reply)),
        Err(ProfileDataError::Unavailable(_))
    ));
}

#[test]
fn a_damaged_database_leaves_the_service_unavailable_and_the_file_untouched() {
    let garbage = b"this is not an SQLite database, but it is long enough to look like one....";
    let harness = Harness::with(
        |path| std::fs::write(path, garbage.repeat(100)).unwrap(),
        Timing::default(),
    );

    assert_unavailable(&harness, "places.sqlite could not be opened");
    let path = harness.path();
    harness.data.shut_down(SHUTDOWN_TIMEOUT);
    assert_eq!(std::fs::read(path).unwrap(), garbage.repeat(100));
}

#[test]
fn a_database_from_a_newer_mercury_leaves_the_service_unavailable() {
    let harness = Harness::with(
        |path| {
            let conn = rusqlite::Connection::open(path).unwrap();
            conn.pragma_update(None, "user_version", 999).unwrap();
        },
        Timing::default(),
    );

    assert_unavailable(&harness, "schema version 999");
}

#[test]
fn a_database_locked_elsewhere_leaves_the_service_unavailable() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("places.sqlite");
    let holder = rusqlite::Connection::open(&path).unwrap();
    holder
        .execute_batch("PRAGMA locking_mode = EXCLUSIVE; BEGIN EXCLUSIVE; CREATE TABLE t(x);")
        .unwrap();

    let harness = Harness::spawn_in(directory, Timing::default());

    assert_unavailable(&harness, "could not be opened");
    drop(holder);
}

#[test]
fn without_a_profile_the_service_is_unavailable() {
    let (wake, wakes) = mpsc::channel();
    let data = ProfileData::spawn(
        || open_database(None),
        Arc::new(|| NOW),
        Arc::new(move || {
            let _sent = wake.send(());
        }),
        Timing::default(),
    );
    let result = Rc::new(RefCell::new(None));
    let slot = Rc::clone(&result);
    data.downloads(1, move |r| *slot.borrow_mut() = Some(r.is_err()));
    wakes.recv_timeout(Duration::from_secs(10)).unwrap();
    data.deliver_replies();
    assert_eq!(*result.borrow(), Some(true));
}

#[test]
fn start_up_marks_unfinished_downloads_interrupted_before_any_request() {
    let harness = Harness::with(
        |path| {
            let mut store = Store::open(path).unwrap();
            store
                .insert_download(&NewDownload {
                    guid: "left-over",
                    url: "https://a.test/file.zip",
                    referrer: None,
                    target_path: "/downloads/file.zip",
                    temp_path: None,
                    mime_type: None,
                    total_bytes: None,
                    started_at_ms: NOW - DAY_MS,
                })
                .unwrap();
        },
        Timing::default(),
    );

    let records = harness.ask(|data, reply| data.downloads(5, reply)).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].state, DownloadState::Interrupted);
    assert_eq!(records[0].ended_at_ms, Some(NOW));
}

/// A database with `pages` pages whose scores were computed three days before `NOW`.
fn stale_database(path: &Path, pages: usize) {
    let mut store = Store::open(path).unwrap();
    for page in 0..pages {
        store
            .record_visit(
                &format!("https://{page}.test/"),
                None,
                VisitTransition::Link,
                NOW - 3 * DAY_MS,
            )
            .unwrap();
    }
}

#[test]
fn start_up_refreshes_every_stale_score() {
    let pages = REFRESH_BATCH as usize * 2 + 10;
    let harness = Harness::with(|path| stale_database(path, pages), Timing::default());

    let deadline = Instant::now() + Duration::from_secs(10);
    while stale_scores(&harness.path(), NOW) > 0 {
        assert!(Instant::now() < deadline, "the scores were not refreshed");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn requests_are_answered_while_scores_are_refreshed() {
    let harness = Harness::with(|path| stale_database(path, 3000), Timing::default());

    let root = harness.ask(|data, reply| data.bookmark_root(BookmarkRoot::Toolbar, reply));

    assert!(root.is_ok());
}

#[test]
fn an_idle_service_refreshes_scores_that_went_stale_again() {
    let timing = Timing {
        idle_before_upkeep: Duration::from_millis(50),
        upkeep_interval: Duration::from_secs(3600),
    };
    let harness = Harness::with(|path| stale_database(path, 10), timing);
    let deadline = Instant::now() + Duration::from_secs(10);
    while stale_scores(&harness.path(), NOW) > 0 {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }

    // Two days later every score is stale again; an hour has passed since the last pass.
    let later = NOW + 2 * DAY_MS;
    assert_eq!(stale_scores(&harness.path(), later), 10);
    harness.clock.store(later, Ordering::SeqCst);

    while stale_scores(&harness.path(), later) > 0 {
        assert!(Instant::now() < deadline, "the idle pass did not run");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn an_idle_service_waits_for_the_upkeep_interval() {
    let timing = Timing {
        idle_before_upkeep: Duration::from_millis(20),
        upkeep_interval: Duration::from_secs(10 * 86_400),
    };
    let harness = Harness::with(|path| stale_database(path, 10), timing);
    let deadline = Instant::now() + Duration::from_secs(10);
    while stale_scores(&harness.path(), NOW) > 0 {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }

    // Two days later every score is stale, but the last pass started less than the interval
    // ago, so idle periods do not start another.
    let later = NOW + 2 * DAY_MS;
    harness.clock.store(later, Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(300));

    assert_eq!(stale_scores(&harness.path(), later), 10);
}

#[test]
fn bookmark_requests_edit_the_tree() {
    let harness = Harness::new();
    let toolbar = harness
        .ask(|data, reply| data.bookmark_root(BookmarkRoot::Toolbar, reply))
        .unwrap();

    let folder = harness
        .ask(|data, reply| data.add_bookmark_folder(toolbar, None, "Work".into(), reply))
        .unwrap();
    let mark = harness
        .ask(|data, reply| {
            data.add_bookmark(
                folder,
                None,
                "Example".into(),
                "https://example.com/".into(),
                reply,
            )
        })
        .unwrap();
    harness
        .ask(|data, reply| data.add_bookmark_separator(toolbar, Some(0), reply))
        .unwrap();
    harness
        .ask(|data, reply| data.rename_bookmark(mark, "Renamed".into(), reply))
        .unwrap();
    harness
        .ask(|data, reply| data.set_bookmark_url(mark, "https://example.org/".into(), reply))
        .unwrap();
    harness
        .ask(|data, reply| data.move_bookmark(mark, toolbar, None, reply))
        .unwrap();

    let node = harness
        .ask(|data, reply| data.bookmark(mark, reply))
        .unwrap();
    assert_eq!(node.title, "Renamed");
    assert_eq!(
        node.kind,
        BookmarkKind::Bookmark {
            url: "https://example.org/".into()
        }
    );
    assert_eq!(node.guid.len(), 12);
    let kinds = harness
        .ask(|data, reply| data.bookmark_children(toolbar, reply))
        .unwrap()
        .into_iter()
        .map(|n| n.kind)
        .collect::<Vec<_>>();
    assert_eq!(kinds.len(), 3);
    assert_eq!(
        harness
            .ask(|data, reply| data.bookmark_for_url("https://example.org/".into(), reply))
            .unwrap(),
        Some(mark)
    );

    harness
        .ask(|data, reply| data.remove_bookmark(folder, reply))
        .unwrap();
    let error = harness
        .ask(|data, reply| data.remove_bookmark(toolbar, reply))
        .unwrap_err();
    assert!(matches!(
        error,
        ProfileDataError::Store(StoreError::CannotModifyRoot)
    ));
}

#[test]
fn download_requests_follow_a_download() {
    let harness = Harness::new();
    let id = harness
        .ask(|data, reply| {
            data.insert_download(
                DownloadStart {
                    url: "https://a.test/f.zip".into(),
                    referrer: Some("https://a.test/".into()),
                    target_path: "/downloads/f.zip".into(),
                    temp_path: Some("/downloads/f.zip.part".into()),
                    mime_type: Some("application/zip".into()),
                    total_bytes: Some(100),
                    started_at_ms: NOW,
                },
                reply,
            )
        })
        .unwrap();
    harness.data.update_download_progress(id, 40, None);
    harness
        .ask(|data, reply| {
            data.set_download_validators(id, None, Some("\"etag\"".into()), None, reply)
        })
        .unwrap();
    harness
        .ask(|data, reply| {
            data.finish_download(
                id,
                DownloadState::Interrupted,
                Some("offline".into()),
                reply,
            )
        })
        .unwrap();
    harness
        .ask(|data, reply| data.resume_download(id, reply))
        .unwrap();
    harness.data.update_download_progress(id, 100, None);
    harness
        .ask(|data, reply| data.finish_download(id, DownloadState::Complete, None, reply))
        .unwrap();
    // A late progress report is refused and logged, not answered.
    harness.data.update_download_progress(id, 1, None);

    let records = harness.ask(|data, reply| data.downloads(5, reply)).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(
        (records[0].received_bytes, records[0].state),
        (100, DownloadState::Complete)
    );
    assert_eq!(records[0].etag.as_deref(), Some("\"etag\""));
    assert_eq!(
        harness
            .ask(|data, reply| data.clear_finished_downloads(reply))
            .unwrap(),
        1
    );
    assert!(
        !harness
            .ask(|data, reply| data.remove_download(id, reply))
            .unwrap()
    );
}

#[test]
fn imports_are_all_or_nothing() {
    let harness = Harness::new();
    let other = harness
        .ask(|data, reply| data.bookmark_root(BookmarkRoot::Other, reply))
        .unwrap();
    let visits = vec![
        ImportVisit {
            url: "https://a.test/".into(),
            title: Some("A".into()),
            visited_at_ms: NOW - DAY_MS,
            transition: VisitTransition::Link,
        },
        ImportVisit {
            url: "https://b.test/".into(),
            title: None,
            visited_at_ms: NOW,
            transition: VisitTransition::Typed,
        },
    ];
    assert_eq!(
        harness
            .ask(|data, reply| data.import_visits(visits, reply))
            .unwrap(),
        2
    );
    let tree = vec![BookmarkImport::Folder {
        title: "Imported".into(),
        guid: new_guid().unwrap(),
        added_at_ms: None,
        children: vec![BookmarkImport::Bookmark {
            title: "A".into(),
            url: "https://a.test/".into(),
            guid: new_guid().unwrap(),
            added_at_ms: Some(NOW - DAY_MS),
        }],
    }];
    assert_eq!(
        harness
            .ask(|data, reply| data.import_bookmarks(vec![(other, tree.clone())], reply))
            .unwrap(),
        2
    );
    // The same GUIDs again: refused, and nothing is added.
    assert!(matches!(
        harness.ask(|data, reply| data.import_bookmarks(vec![(other, tree)], reply)),
        Err(ProfileDataError::Store(StoreError::ImportEntryRefused {
            index: 0,
            ..
        }))
    ));
    assert_eq!(
        harness
            .ask(|data, reply| data.bookmark_children(other, reply))
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn guids_are_twelve_url_safe_characters_and_differ() {
    let guids = (0..100).map(|_| new_guid().unwrap()).collect::<Vec<_>>();
    for guid in &guids {
        assert_eq!(guid.len(), 12);
        assert!(
            guid.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        );
    }
    let mut unique = guids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), guids.len());
}

#[test]
fn back_and_forward_are_stored_as_links() {
    assert_eq!(
        history::transition(NavigationOrigin::History),
        VisitTransition::Link
    );
    assert_eq!(
        history::transition(NavigationOrigin::Typed),
        VisitTransition::Typed
    );
}
