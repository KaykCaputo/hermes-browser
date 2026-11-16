use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use mercury_store::{
    BookmarkImport, BookmarkKind, BookmarkRoot, DownloadState, NewDownload, Store, StoreError,
    VisitImport, VisitTransition,
};
use talaria_core::{NavigationOrigin, VisitKey, VisitUpdate};

use super::downloads::DownloadStart;
use super::import::{IMPORT_CHUNK, ImportOutcome, ImportProgress, ImportSummary, ImportVisit};
use super::*;

const DAY_MS: i64 = 86_400_000;
const NOW: i64 = 1_800_000_000_000;
/// Generous: CI machines are slow, and a locked database takes the store's 5 s busy timeout.
const PATIENCE: Duration = Duration::from_secs(30);

fn deadline() -> Instant {
    Instant::now() + PATIENCE
}

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
        let deadline = deadline();
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

    /// A request that only waits for the requests before it.
    fn sync(&self) {
        self.ask(|data, reply| data.downloads(1, reply)).unwrap();
    }

    fn stop(&self) {
        self.data.shut_down(deadline());
    }

    /// Shuts the service down and opens the file directly.
    fn reopen(self) -> (Store, tempfile::TempDir) {
        self.stop();
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

fn title(url: &str, title: &str) -> VisitUpdate {
    VisitUpdate::Title {
        url: url.to_owned(),
        title: title.to_owned(),
    }
}

/// The stored title of `url`, read on a connection of the test's own; `None` also before the
/// page (or the schema) exists.
fn stored_title(path: &Path, url: &str) -> Option<String> {
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row("SELECT title FROM places WHERE url = ?1", [url], |row| {
            row.get(0)
        })
        .ok()
        .flatten()
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

fn wait_until(what: &str, done: impl Fn() -> bool) {
    let deadline = deadline();
    while !done() {
        assert!(Instant::now() < deadline, "{what} did not happen");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A recorder of what a test's callbacks saw.
fn log() -> Rc<RefCell<Vec<String>>> {
    Rc::new(RefCell::new(Vec::new()))
}

// Ordering, threading, notifications.

#[test]
fn fifty_requests_and_writes_are_answered_in_order() {
    let harness = Harness::new();
    let toolbar = harness
        .ask(|data, reply| data.bookmark_root(BookmarkRoot::Toolbar, reply))
        .unwrap();
    let seen = log();
    for index in 0..25 {
        let url = format!("https://{index}.test/");
        harness
            .data
            .record_updates(vec![visit(index, &url, NavigationOrigin::Link)], NOW);
        let added = Rc::clone(&seen);
        harness
            .data
            .add_bookmark(toolbar, None, format!("{index}"), url, move |result| {
                added.borrow_mut().push(format!("add {}", result.is_ok()));
            });
        let read = Rc::clone(&seen);
        harness.data.bookmark_children(toolbar, move |children| {
            read.borrow_mut()
                .push(format!("children {}", children.unwrap().len()));
        });
    }
    assert!(
        seen.borrow().is_empty(),
        "replies only arrive when delivered"
    );

    harness.settle(|| seen.borrow().len() == 50);
    let expected = (1..=25)
        .flat_map(|count| ["add true".to_owned(), format!("children {count}")])
        .collect::<Vec<_>>();
    assert_eq!(*seen.borrow(), expected);
    let (store, _directory) = harness.reopen();
    assert_eq!(store.history(None, None, 100).unwrap().len(), 25);
}

#[test]
fn wake_ups_are_coalesced_until_the_replies_are_delivered() {
    let harness = Harness::new();
    let answered = Rc::new(RefCell::new(0));
    for _ in 0..50 {
        let count = Rc::clone(&answered);
        harness.data.downloads(1, move |_| *count.borrow_mut() += 1);
    }
    std::thread::sleep(Duration::from_millis(300));

    assert_eq!(harness.wakes.try_iter().count(), 1);
    harness.data.deliver_replies();
    assert_eq!(*answered.borrow(), 50);
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
fn writes_notify_their_area_once_per_delivery_after_the_replies() {
    let harness = Harness::new();
    let seen = log();
    for (area, name) in [
        (Area::Bookmarks, "bookmarks changed"),
        (Area::History, "history changed"),
        (Area::Downloads, "downloads changed"),
    ] {
        let seen = Rc::clone(&seen);
        harness
            .data
            .on_change(area, move |_| seen.borrow_mut().push(name.to_owned()));
    }
    let toolbar = harness
        .ask(|data, reply| data.bookmark_root(BookmarkRoot::Toolbar, reply))
        .unwrap();
    assert!(seen.borrow().is_empty(), "a read changes nothing");

    for index in 0..3 {
        let replies = Rc::clone(&seen);
        harness.data.add_bookmark(
            toolbar,
            None,
            "A".into(),
            format!("https://{index}.test/"),
            move |_| replies.borrow_mut().push("added".to_owned()),
        );
    }
    std::thread::sleep(Duration::from_millis(200));
    harness.settle(|| seen.borrow().len() == 4);
    assert_eq!(
        *seen.borrow(),
        vec!["added", "added", "added", "bookmarks changed"]
    );

    seen.borrow_mut().clear();
    harness.data.record_updates(
        vec![visit(0, "https://a.test/", NavigationOrigin::Link)],
        NOW,
    );
    harness.settle(|| !seen.borrow().is_empty());
    assert_eq!(*seen.borrow(), vec!["history changed"]);

    seen.borrow_mut().clear();
    let guid = new_guid().unwrap();
    harness
        .ask(|data, reply| data.insert_download(download(&guid), reply))
        .unwrap();
    harness.settle(|| !seen.borrow().is_empty());
    assert_eq!(*seen.borrow(), vec!["downloads changed"]);
}

#[test]
fn a_failed_write_notifies_nothing_and_a_removed_listener_is_not_called() {
    let harness = Harness::new();
    let calls = Rc::new(RefCell::new(0));
    let count = Rc::clone(&calls);
    let listener = harness
        .data
        .on_change(Area::Bookmarks, move |_| *count.borrow_mut() += 1);
    let toolbar = harness
        .ask(|data, reply| data.bookmark_root(BookmarkRoot::Toolbar, reply))
        .unwrap();

    assert!(
        harness
            .ask(|data, reply| data.remove_bookmark(toolbar, reply))
            .is_err()
    );
    assert_eq!(*calls.borrow(), 0);

    harness.data.remove_change_listener(listener);
    harness
        .ask(|data, reply| {
            data.add_bookmark_folder(toolbar, None, "F".into(), reply);
        })
        .unwrap();
    harness.sync();
    assert_eq!(*calls.borrow(), 0);
}

#[test]
fn a_panicking_request_is_answered_unavailable_and_the_service_goes_on() {
    let harness = Harness::new();

    let result = harness.ask(|data, reply| {
        data.request::<()>(None, |_, _| panic!("deliberate panic in a request"), reply);
    });

    assert!(
        matches!(&result, Err(ProfileDataError::Unavailable(reason)) if reason.contains("unexpectedly")),
        "{result:?}"
    );
    harness.sync();
}

#[test]
fn requests_after_shutdown_are_answered_unavailable() {
    let harness = Harness::new();
    harness.stop();

    let result = harness.ask(|data, reply| data.downloads(1, reply));
    harness
        .data
        .set_page_title("https://a.test/".into(), "A".into());

    assert!(matches!(result, Err(ProfileDataError::Unavailable(_))));
    assert!(harness.data.shared.pending.borrow().is_empty());
}

// History.

#[test]
fn recorded_visits_titles_and_transitions_reach_the_database() {
    let harness = Harness::new();
    harness.data.record_updates(
        vec![
            visit(0, "https://a.test/", NavigationOrigin::Typed),
            title("https://a.test/", "Page A"),
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

    harness.sync();
    let (store, _directory) = harness.reopen();
    assert_eq!(store.history(None, None, 10).unwrap().len(), 1);
}

fn short_debounce() -> Timing {
    Timing {
        title_debounce: Duration::from_millis(400),
        ..Timing::default()
    }
}

#[test]
fn a_pages_first_title_is_written_at_once_and_later_ones_are_held() {
    let harness = Harness::with(|_| {}, short_debounce());
    let url = "https://a.test/";
    harness.data.record_updates(
        vec![
            visit(0, url, NavigationOrigin::Link),
            title(url, "First"),
            title(url, "Second"),
            title(url, "Third"),
        ],
        NOW,
    );
    wait_until("the first title", || {
        stored_title(&harness.path(), url).is_some()
    });
    assert_eq!(stored_title(&harness.path(), url).as_deref(), Some("First"));

    // The last held title is written when the window has passed, without any request.
    wait_until("the held title", || {
        stored_title(&harness.path(), url).as_deref() == Some("Third")
    });
}

#[test]
fn held_titles_are_written_before_a_request_with_a_reply() {
    let harness = Harness::new();
    let url = "https://a.test/";
    harness.data.record_updates(
        vec![
            visit(0, url, NavigationOrigin::Link),
            title(url, "First"),
            title(url, "Latest"),
        ],
        NOW,
    );

    harness.sync();

    assert_eq!(
        stored_title(&harness.path(), url).as_deref(),
        Some("Latest")
    );
}

#[test]
fn held_titles_are_written_at_shutdown() {
    let harness = Harness::new();
    let url = "https://a.test/";
    harness.data.record_updates(
        vec![
            visit(0, url, NavigationOrigin::Link),
            title(url, "First"),
            title(url, "Latest"),
        ],
        NOW,
    );

    let (store, _directory) = harness.reopen();
    assert_eq!(
        store.history(None, None, 1).unwrap()[0].title.as_deref(),
        Some("Latest")
    );
}

// Opening, unavailability, shutdown.

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
    harness.stop();

    // Closed: the last connection checkpointed and removed the write-ahead log.
    assert!(!path.with_extension("sqlite-wal").exists());
    let store = Store::open(&path).unwrap();
    assert_eq!(store.history(None, None, 1000).unwrap().len(), 500);
}

#[test]
fn shutdown_returns_by_its_deadline() {
    let harness = Harness::new();
    harness.data.request::<()>(
        None,
        |_, _| {
            std::thread::sleep(Duration::from_millis(1500));
            Ok(())
        },
        |_| {},
    );

    let started = Instant::now();
    harness
        .data
        .shut_down(Instant::now() + Duration::from_millis(200));

    assert!(started.elapsed() < Duration::from_millis(1000));
}

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[cfg(unix)]
#[test]
fn the_database_and_its_journal_files_are_private() {
    let harness = Harness::new();
    harness.data.record_updates(
        vec![visit(0, "https://a.test/", NavigationOrigin::Link)],
        NOW,
    );
    harness.sync();

    assert_eq!(mode(&harness.path()), 0o600);
    assert_eq!(mode(&harness.path().with_extension("sqlite-wal")), 0o600);
    assert_eq!(mode(&harness.path().with_extension("sqlite-shm")), 0o600);
}

#[cfg(unix)]
#[test]
fn an_existing_database_open_to_others_is_made_private() {
    use std::os::unix::fs::PermissionsExt;
    let harness = Harness::with(
        |path| {
            drop(Store::open(path).unwrap());
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644)).unwrap();
        },
        Timing::default(),
    );
    harness.sync();

    assert_eq!(mode(&harness.path()), 0o600);
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
    let reason = harness.data.unavailable_reason().expect("a reason");
    assert!(reason.contains(reason_part), "{reason}");
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
fn an_open_database_has_no_unavailable_reason() {
    let harness = Harness::new();
    harness.sync();

    assert_eq!(harness.data.unavailable_reason(), None);
}

#[test]
fn a_file_that_is_not_a_database_leaves_the_service_unavailable_and_the_file_untouched() {
    let garbage = b"this is not an SQLite database, but it is long enough to look like one....";
    let harness = Harness::with(
        |path| std::fs::write(path, garbage.repeat(100)).unwrap(),
        Timing::default(),
    );

    assert_unavailable(&harness, "places.sqlite could not be opened");
    let path = harness.path();
    harness.stop();
    assert_eq!(std::fs::read(path).unwrap(), garbage.repeat(100));
}

#[test]
fn a_database_with_damaged_pages_fails_the_check_and_is_left_untouched() {
    let harness = Harness::with(
        |path| {
            let mut store = Store::open(path).unwrap();
            let visits = (0..2000)
                .map(|n| format!("https://{n}.test/some/longer/path"))
                .collect::<Vec<_>>();
            let imports = visits
                .iter()
                .map(|url| VisitImport {
                    url,
                    title: Some("A title"),
                    visited_at_ms: NOW,
                    transition: VisitTransition::Link,
                })
                .collect::<Vec<_>>();
            store.import_visits(&imports, NOW).unwrap();
            drop(store);
            let mut bytes = std::fs::read(path).unwrap();
            assert!(bytes.len() > 40_000);
            // Page 5 onwards: the header and schema stay readable, the tables do not.
            bytes[4 * 4096..9 * 4096].fill(0xA5);
            std::fs::write(path, bytes).unwrap();
        },
        Timing::default(),
    );
    let before = std::fs::read(harness.path()).unwrap();

    assert_unavailable(&harness, "could not be opened");
    let path = harness.path();
    harness.stop();
    assert_eq!(std::fs::read(path).unwrap(), before);
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
    // Takes the 5 s busy timeout: neither the store nor the check lets a caller shorten it.
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
    wakes.recv_timeout(PATIENCE).unwrap();
    data.deliver_replies();
    assert_eq!(*result.borrow(), Some(true));
    assert!(
        data.unavailable_reason()
            .is_some_and(|reason| reason.contains("no usable profile"))
    );
}

// Start-up and upkeep.

fn download(guid: &str) -> DownloadStart {
    DownloadStart {
        guid: guid.to_owned(),
        url: "https://a.test/f.zip".into(),
        referrer: Some("https://a.test/".into()),
        target_path: "/downloads/f.zip".into(),
        temp_path: Some("/downloads/f.zip.part".into()),
        mime_type: Some("application/zip".into()),
        total_bytes: Some(100),
        started_at_ms: NOW,
    }
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
    let urls = (0..pages)
        .map(|page| format!("https://{page}.test/"))
        .collect::<Vec<_>>();
    let visits = urls
        .iter()
        .map(|url| VisitImport {
            url,
            title: None,
            visited_at_ms: NOW - 3 * DAY_MS,
            transition: VisitTransition::Link,
        })
        .collect::<Vec<_>>();
    store.import_visits(&visits, NOW - 3 * DAY_MS).unwrap();
}

#[test]
fn start_up_refreshes_every_stale_score() {
    let pages = REFRESH_BATCH as usize * 2 + 10;
    let harness = Harness::with(|path| stale_database(path, pages), Timing::default());

    wait_until("the refresh", || stale_scores(&harness.path(), NOW) == 0);
}

#[test]
fn requests_are_answered_between_upkeep_chunks() {
    let harness = Harness::with(|path| stale_database(path, 30_000), Timing::default());
    let path = harness.path();

    let stale_when_answered = harness.ask(|data, reply| {
        data.bookmark_root(BookmarkRoot::Toolbar, move |root| {
            reply(root.map(|_| stale_scores(&path, NOW)))
        });
    });

    let stale = stale_when_answered.unwrap();
    assert!(stale > 0, "the request waited for the whole pass");
    wait_until("the refresh", || stale_scores(&harness.path(), NOW) == 0);
}

#[test]
fn an_idle_service_refreshes_scores_that_went_stale_again() {
    let timing = Timing {
        idle_before_upkeep: Duration::from_millis(50),
        upkeep_interval: Duration::from_millis(100),
        ..Timing::default()
    };
    let harness = Harness::with(|path| stale_database(path, 10), timing);
    wait_until("the first pass", || stale_scores(&harness.path(), NOW) == 0);

    // Two days later every score is stale again.
    let later = NOW + 2 * DAY_MS;
    assert_eq!(stale_scores(&harness.path(), later), 10);
    harness.clock.store(later, Ordering::SeqCst);

    wait_until("the idle pass", || {
        stale_scores(&harness.path(), later) == 0
    });
}

#[test]
fn an_idle_service_waits_for_the_upkeep_interval() {
    let timing = Timing {
        idle_before_upkeep: Duration::from_millis(20),
        upkeep_interval: Duration::from_secs(10 * 86_400),
        ..Timing::default()
    };
    let harness = Harness::with(|path| stale_database(path, 10), timing);
    wait_until("the first pass", || stale_scores(&harness.path(), NOW) == 0);

    // Two days later by the clock every score is stale, but the interval is measured on the
    // monotonic clock and has not passed.
    let later = NOW + 2 * DAY_MS;
    harness.clock.store(later, Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(300));

    assert_eq!(stale_scores(&harness.path(), later), 10);
}

// Bookmarks and downloads.

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
    assert_eq!(
        harness
            .ask(|data, reply| data.bookmark_children(toolbar, reply))
            .unwrap()
            .len(),
        3
    );
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
fn a_download_is_addressed_by_its_guid_before_the_insert_is_answered() {
    let harness = Harness::new();
    let guid = new_guid().unwrap();
    let data = &harness.data;

    // All sent at once, without waiting for any reply.
    data.insert_download(download(&guid), |result| result.unwrap());
    data.update_download_progress(guid.clone(), 40, None);
    data.set_download_validators(guid.clone(), None, Some("\"etag\"".into()), None, |r| {
        r.unwrap()
    });
    data.finish_download(
        guid.clone(),
        DownloadState::Interrupted,
        Some("offline".into()),
        |r| r.unwrap(),
    );
    data.resume_download(guid.clone(), |r| r.unwrap());
    data.update_download_progress(guid.clone(), 100, None);
    data.finish_download(guid.clone(), DownloadState::Complete, None, |r| r.unwrap());
    // A late progress report is refused and logged, not answered.
    data.update_download_progress(guid.clone(), 1, None);

    let records = harness.ask(|data, reply| data.downloads(5, reply)).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].guid, guid);
    assert_eq!(
        (records[0].received_bytes, records[0].state),
        (100, DownloadState::Complete)
    );
    assert_eq!(records[0].etag.as_deref(), Some("\"etag\""));
    assert!(matches!(
        harness.ask(|data, reply| data.resume_download("unknown".into(), reply)),
        Err(ProfileDataError::Store(StoreError::NotFound))
    ));
    assert!(
        harness
            .ask(|data, reply| data.remove_download(guid.clone(), reply))
            .unwrap()
    );
    assert_eq!(
        harness
            .ask(|data, reply| data.clear_finished_downloads(reply))
            .unwrap(),
        0
    );
}

#[test]
fn a_download_of_an_earlier_run_is_addressed_by_its_guid() {
    let harness = Harness::with(
        |path| {
            let mut store = Store::open(path).unwrap();
            store
                .insert_download(&NewDownload {
                    guid: "earlier-run",
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

    harness
        .ask(|data, reply| data.resume_download("earlier-run".into(), reply))
        .unwrap();
    let records = harness.ask(|data, reply| data.downloads(5, reply)).unwrap();
    assert_eq!(records[0].state, DownloadState::InProgress);
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

// Stepwise import.

fn visits_to_import(count: usize) -> Vec<ImportVisit> {
    (0..count)
        .map(|n| ImportVisit {
            url: format!("https://site-{}.test/page/{n}", n % 5000),
            title: Some(format!("Page {n}")),
            visited_at_ms: NOW - (n as i64) * 60_000,
            transition: VisitTransition::Link,
        })
        .collect()
}

/// Starts an import and gathers its progress and summary.
struct ImportRun {
    progress: Rc<RefCell<Vec<ImportProgress>>>,
    summary: Rc<RefCell<Option<ImportSummary>>>,
    handle: super::jobs::JobHandle,
}

impl ImportRun {
    fn visits(harness: &Harness, visits: Vec<ImportVisit>) -> Self {
        let progress = Rc::new(RefCell::new(Vec::new()));
        let summary = Rc::new(RefCell::new(None));
        let (seen, done) = (Rc::clone(&progress), Rc::clone(&summary));
        let handle = harness.data.import_visits(
            visits,
            move |p| seen.borrow_mut().push(p),
            move |s| *done.borrow_mut() = Some(s),
        );
        Self {
            progress,
            summary,
            handle,
        }
    }

    fn finish(&self, harness: &Harness) -> ImportSummary {
        harness.settle(|| self.summary.borrow().is_some());
        self.summary.borrow_mut().take().unwrap()
    }
}

#[test]
fn a_visit_import_runs_in_chunks_with_progress() {
    let harness = Harness::new();
    let total = IMPORT_CHUNK * 2 + 7;

    let run = ImportRun::visits(&harness, visits_to_import(total));
    let summary = run.finish(&harness);

    assert!(matches!(summary.outcome, ImportOutcome::Completed));
    assert_eq!(
        (summary.imported, summary.total),
        (total as u64, total as u64)
    );
    let done = run
        .progress
        .borrow()
        .iter()
        .map(|p| p.done)
        .collect::<Vec<_>>();
    assert_eq!(done, vec![IMPORT_CHUNK as u64, 2 * IMPORT_CHUNK as u64]);
    let (store, _directory) = harness.reopen();
    assert_eq!(
        store.history(None, None, total as u32 + 1).unwrap().len(),
        total
    );
}

#[test]
fn requests_are_answered_while_an_import_runs() {
    let harness = Harness::new();
    let run = ImportRun::visits(&harness, visits_to_import(IMPORT_CHUNK * 10));

    harness.sync();

    assert!(
        run.summary.borrow().is_none(),
        "the request waited for the whole import"
    );
    run.finish(&harness);
}

#[test]
fn a_cancelled_import_keeps_the_chunks_it_finished() {
    let harness = Harness::new();
    let run = ImportRun::visits(&harness, visits_to_import(IMPORT_CHUNK * 30));
    harness.settle(|| !run.progress.borrow().is_empty());

    run.handle.cancel();
    let summary = run.finish(&harness);

    assert!(matches!(summary.outcome, ImportOutcome::Cancelled));
    assert!(summary.imported < summary.total);
    assert_eq!(summary.imported % IMPORT_CHUNK as u64, 0);
    let imported = summary.imported;
    let (store, _directory) = harness.reopen();
    assert_eq!(
        store.history(None, None, u32::MAX).unwrap().len() as u64,
        imported
    );
}

#[test]
fn shutdown_cancels_an_import_between_chunks_and_reports_it() {
    let harness = Harness::new();
    let run = ImportRun::visits(&harness, visits_to_import(IMPORT_CHUNK * 50));
    harness.settle(|| !run.progress.borrow().is_empty());

    harness.stop();
    harness.data.deliver_replies();

    let summary = run.summary.borrow_mut().take().expect("a summary");
    assert!(matches!(summary.outcome, ImportOutcome::Cancelled));
    assert!(summary.imported < summary.total);
    let store = Store::open(&harness.path()).unwrap();
    assert_eq!(
        store.history(None, None, u32::MAX).unwrap().len() as u64,
        summary.imported
    );
}

#[test]
fn a_refused_visit_is_skipped_and_the_import_goes_on() {
    let harness = Harness::new();
    let total = IMPORT_CHUNK * 2 + 10;
    let mut visits = visits_to_import(total);
    let too_long = format!(
        "https://a.test/{}",
        "x".repeat(mercury_store::MAX_URL_BYTES)
    );
    visits[3].url = too_long.clone();
    visits[IMPORT_CHUNK * 2 + 3].url = too_long;

    let run = ImportRun::visits(&harness, visits);
    let summary = run.finish(&harness);

    assert!(matches!(summary.outcome, ImportOutcome::Completed));
    assert_eq!(
        (
            summary.imported,
            summary.skipped,
            summary.total,
            summary.resume_from
        ),
        (total as u64 - 2, 2, total as u64, total as u64)
    );
    assert_eq!(run.progress.borrow()[0].skipped, 1);
    assert_eq!(run.progress.borrow()[0].done, IMPORT_CHUNK as u64);
    let (store, _directory) = harness.reopen();
    assert_eq!(
        store.history(None, None, u32::MAX).unwrap().len(),
        total - 2
    );
}

#[test]
fn a_cancelled_import_says_where_to_resume() {
    let harness = Harness::new();
    let visits = visits_to_import(IMPORT_CHUNK * 30);
    let run = ImportRun::visits(&harness, visits.clone());
    harness.settle(|| !run.progress.borrow().is_empty());
    run.handle.cancel();
    let first = run.finish(&harness);
    assert!(matches!(first.outcome, ImportOutcome::Cancelled));

    let rest = visits[first.resume_from as usize..].to_vec();
    let second = ImportRun::visits(&harness, rest).finish(&harness);

    assert!(matches!(second.outcome, ImportOutcome::Completed));
    assert_eq!(first.imported + second.imported, visits.len() as u64);
    let (store, _directory) = harness.reopen();
    assert_eq!(
        store.history(None, None, u32::MAX).unwrap().len(),
        visits.len()
    );
}

#[test]
fn an_import_into_an_unavailable_database_fails_at_once() {
    let harness = Harness::with(
        |path| std::fs::write(path, b"not a database".repeat(100)).unwrap(),
        Timing::default(),
    );

    let summary = ImportRun::visits(&harness, visits_to_import(10)).finish(&harness);

    assert!(matches!(
        summary.outcome,
        ImportOutcome::Failed(ProfileDataError::Unavailable(_))
    ));
}

fn bookmark_tree(count: usize) -> Vec<BookmarkImport> {
    (0..count)
        .map(|n| BookmarkImport::Folder {
            title: format!("Folder {n}"),
            guid: new_guid().unwrap(),
            added_at_ms: None,
            children: (0..3)
                .map(|m| BookmarkImport::Bookmark {
                    title: format!("Mark {n}.{m}"),
                    url: format!("https://{n}.test/{m}"),
                    guid: new_guid().unwrap(),
                    added_at_ms: Some(NOW),
                })
                .collect(),
        })
        .collect()
}

#[test]
fn a_bookmark_import_runs_in_chunks_of_whole_trees_and_notifies_once() {
    let harness = Harness::new();
    let changes = Rc::new(RefCell::new(0));
    let count = Rc::clone(&changes);
    harness
        .data
        .on_change(Area::Bookmarks, move |_| *count.borrow_mut() += 1);
    let other = harness
        .ask(|data, reply| data.bookmark_root(BookmarkRoot::Other, reply))
        .unwrap();
    let toolbar = harness
        .ask(|data, reply| data.bookmark_root(BookmarkRoot::Toolbar, reply))
        .unwrap();
    // 1,000 folders of 4 nodes after one on the toolbar: chunks of 125 trees, 500 nodes.
    let trees = bookmark_tree(1000);
    let progress = Rc::new(RefCell::new(Vec::new()));
    let summary = Rc::new(RefCell::new(None));
    let (seen, done) = (Rc::clone(&progress), Rc::clone(&summary));

    harness.data.import_bookmarks(
        vec![(toolbar, bookmark_tree(1)), (other, trees)],
        move |p| seen.borrow_mut().push(p.done),
        move |s| *done.borrow_mut() = Some(s),
    );
    harness.settle(|| summary.borrow().is_some());
    // The notification follows the summary, possibly in a later delivery.
    harness.settle(|| *changes.borrow() > 0);

    let summary = summary.borrow_mut().take().unwrap();
    assert!(matches!(summary.outcome, ImportOutcome::Completed));
    assert_eq!((summary.imported, summary.total), (4004, 4004));
    assert_eq!(
        *progress.borrow(),
        (1..=8).map(|n| n * IMPORT_CHUNK as u64).collect::<Vec<_>>()
    );
    assert_eq!(*changes.borrow(), 1);
    let positions = harness
        .ask(|data, reply| data.bookmark_children(other, reply))
        .unwrap()
        .into_iter()
        .map(|node| node.position)
        .collect::<Vec<_>>();
    assert_eq!(positions, (0..1000).collect::<Vec<_>>());
}

#[test]
fn a_refused_bookmark_folder_is_skipped_with_its_contents() {
    let harness = Harness::new();
    let other = harness
        .ask(|data, reply| data.bookmark_root(BookmarkRoot::Other, reply))
        .unwrap();
    let mut trees = bookmark_tree(600);
    let BookmarkImport::Folder { guid, .. } = &trees[0] else {
        unreachable!()
    };
    let reused = guid.clone();
    let BookmarkImport::Folder { guid, .. } = &mut trees[550] else {
        unreachable!()
    };
    *guid = reused;

    let summary = Rc::new(RefCell::new(None));
    let done = Rc::clone(&summary);
    harness.data.import_bookmarks(
        vec![(other, trees)],
        |_| {},
        move |s| *done.borrow_mut() = Some(s),
    );
    harness.settle(|| summary.borrow().is_some());

    let summary = summary.borrow_mut().take().unwrap();
    assert!(
        matches!(summary.outcome, ImportOutcome::Completed),
        "{:?}",
        summary.outcome
    );
    // Folder 550 (a GUID already used by folder 0) and its three bookmarks are left out.
    assert_eq!(
        (summary.imported, summary.skipped, summary.resume_from),
        (2396, 4, 600)
    );
    assert_eq!(
        harness
            .ask(|data, reply| data.bookmark_children(other, reply))
            .unwrap()
            .len(),
        599
    );
}

#[test]
fn a_bookmark_import_notifies_the_folders_it_filled() {
    let harness = Harness::new();
    let changes = log();
    let seen = Rc::clone(&changes);
    harness.data.on_change(Area::Bookmarks, move |change| {
        seen.borrow_mut().push(format!("{:?}", change.folders))
    });
    let other = harness
        .ask(|data, reply| data.bookmark_root(BookmarkRoot::Other, reply))
        .unwrap();
    let summary = Rc::new(RefCell::new(None));
    let done = Rc::clone(&summary);

    harness.data.import_bookmarks(
        vec![(other, bookmark_tree(300))],
        |_| {},
        move |s| *done.borrow_mut() = Some(s),
    );
    harness.settle(|| summary.borrow().is_some() && !changes.borrow().is_empty());

    assert_eq!(*changes.borrow(), vec![format!("{:?}", Some(vec![other]))]);
}

/// The benchmark the review asked for: 100,000 visits through the chunked import into a
/// temporary database, then the open-time integrity check on the result. Two shapes: every visit
/// a new page (the worst case per chunk), and 100,000 visits over 20,000 pages. Run with
/// `cargo test --release -p mercury-desktop import_benchmark -- --ignored --nocapture`.
#[test]
#[ignore = "a benchmark; run it on its own"]
fn import_benchmark_100k_visits() {
    for (shape, pages) in [("one page per visit", 100_000), ("20,000 pages", 20_000)] {
        let harness = Harness::new();
        let visits = (0..100_000)
            .map(|n| ImportVisit {
                url: format!("https://site-{}.test/page/{}", n % 5000, n % pages),
                title: Some(format!("Page {}", n % pages)),
                visited_at_ms: NOW - (n as i64) * 60_000,
                transition: VisitTransition::Link,
            })
            .collect::<Vec<_>>();
        let started = Instant::now();

        let run = ImportRun::visits(&harness, visits);
        let summary = run.finish(&harness);

        let elapsed = started.elapsed();
        assert!(matches!(summary.outcome, ImportOutcome::Completed));
        let path = harness.path();
        harness.stop();
        let size = std::fs::metadata(&path).unwrap().len();
        let check_started = Instant::now();
        quick_check(&path).unwrap();
        println!(
            "{shape}: imported {} visits in {:.2} s, chunks of {IMPORT_CHUNK}; longest chunk \
             {:.1} ms; file {:.1} MB; quick_check {:.1} ms",
            summary.imported,
            elapsed.as_secs_f64(),
            summary.longest_chunk.as_secs_f64() * 1000.0,
            size as f64 / 1e6,
            check_started.elapsed().as_secs_f64() * 1000.0,
        );
        // One upkeep chunk three days later, when every score is stale.
        let mut store = Store::open(&path).unwrap();
        let mut longest = Duration::ZERO;
        for _ in 0..5 {
            let started = Instant::now();
            let refreshed = store
                .refresh_frecencies(NOW + 3 * DAY_MS, REFRESH_BATCH)
                .unwrap();
            assert_eq!(refreshed, u64::from(REFRESH_BATCH));
            longest = longest.max(started.elapsed());
        }
        println!(
            "{shape}: longest of 5 upkeep chunks of {REFRESH_BATCH} pages {:.1} ms",
            longest.as_secs_f64() * 1000.0
        );
    }
}

// Round 1c: change hints, quiet progress, coalesced history, remembered misses, check policy.

/// Records each change notification of `area`.
fn listen(harness: &Harness, area: Area) -> Rc<RefCell<Vec<Change>>> {
    let changes = Rc::new(RefCell::new(Vec::new()));
    let seen = Rc::clone(&changes);
    harness
        .data
        .on_change(area, move |change| seen.borrow_mut().push(change.clone()));
    changes
}

#[test]
fn bookmark_changes_name_the_folders_whose_listing_changed() {
    let harness = Harness::new();
    let changes = listen(&harness, Area::Bookmarks);
    let toolbar = harness
        .ask(|data, reply| data.bookmark_root(BookmarkRoot::Toolbar, reply))
        .unwrap();
    let other = harness
        .ask(|data, reply| data.bookmark_root(BookmarkRoot::Other, reply))
        .unwrap();
    let take = |harness: &Harness| {
        harness.settle(|| !changes.borrow().is_empty());
        let change = changes.borrow_mut().remove(0);
        change.folders.unwrap()
    };

    let mark = harness
        .ask(|data, reply| {
            data.add_bookmark(toolbar, None, "A".into(), "https://a.test/".into(), reply)
        })
        .unwrap();
    assert_eq!(take(&harness), vec![toolbar]);
    harness
        .ask(|data, reply| data.rename_bookmark(mark, "B".into(), reply))
        .unwrap();
    assert_eq!(take(&harness), vec![toolbar]);
    harness
        .ask(|data, reply| data.move_bookmark(mark, other, None, reply))
        .unwrap();
    assert_eq!(take(&harness), vec![toolbar, other]);
    harness
        .ask(|data, reply| data.remove_bookmark(mark, reply))
        .unwrap();
    assert_eq!(take(&harness), vec![other]);
}

#[test]
fn download_changes_name_the_record_and_progress_notifies_nothing() {
    let harness = Harness::new();
    let changes = listen(&harness, Area::Downloads);
    let guid = new_guid().unwrap();
    harness
        .ask(|data, reply| data.insert_download(download(&guid), reply))
        .unwrap();
    harness.settle(|| !changes.borrow().is_empty());
    assert_eq!(
        changes.borrow_mut().remove(0).downloads,
        Some(vec![guid.clone()])
    );

    for received in [10, 20, 30] {
        harness
            .data
            .update_download_progress(guid.clone(), received, None);
    }
    harness
        .ask(|data, reply| {
            data.set_download_validators(guid.clone(), None, Some("e".into()), None, reply)
        })
        .unwrap();
    harness.sync();
    assert!(changes.borrow().is_empty(), "{:?}", changes.borrow());

    harness
        .ask(|data, reply| data.finish_download(guid.clone(), DownloadState::Complete, None, reply))
        .unwrap();
    harness.settle(|| !changes.borrow().is_empty());
    assert_eq!(changes.borrow()[0].downloads, Some(vec![guid]));
}

#[test]
fn history_notifications_are_coalesced() {
    let timing = Timing {
        history_notify_interval: Duration::from_millis(400),
        ..Timing::default()
    };
    let harness = Harness::with(|_| {}, timing);
    let changes = listen(&harness, Area::History);

    harness.data.record_updates(
        vec![visit(0, "https://a.test/", NavigationOrigin::Link)],
        NOW,
    );
    harness.settle(|| changes.borrow().len() == 1);
    for key in 1..10 {
        harness.data.record_updates(
            vec![
                visit(key, &format!("https://{key}.test/"), NavigationOrigin::Link),
                title(&format!("https://{key}.test/"), "T"),
            ],
            NOW,
        );
    }
    harness.sync();
    assert_eq!(changes.borrow().len(), 1, "held back within the interval");

    harness.settle(|| changes.borrow().len() == 2);
    std::thread::sleep(Duration::from_millis(600));
    harness.data.deliver_replies();
    assert_eq!(changes.borrow().len(), 2);
}

#[test]
fn an_unknown_download_guid_is_looked_up_only_once() {
    let harness = Harness::new();
    let guid = new_guid().unwrap();

    harness.data.update_download_progress(guid.clone(), 1, None);
    harness.data.update_download_progress(guid.clone(), 2, None);
    harness.data.update_download_progress(guid.clone(), 3, None);
    let looked_up = harness.ask(|data, reply| {
        let guid = guid.clone();
        data.request(
            None,
            move |db, _| Ok((db.download_misses.contains(&guid), db.download_scans)),
            reply,
        );
    });
    assert_eq!(looked_up.unwrap(), (true, 1));

    harness
        .ask(|data, reply| data.insert_download(download(&guid), reply))
        .unwrap();
    harness
        .data
        .update_download_progress(guid.clone(), 50, None);
    let records = harness.ask(|data, reply| data.downloads(1, reply)).unwrap();
    assert_eq!(records[0].received_bytes, 50);
    let still_missed = harness.ask(|data, reply| {
        let guid = guid.clone();
        data.request(
            None,
            move |db, _| Ok(db.download_misses.contains(&guid)),
            reply,
        );
    });
    assert!(!still_missed.unwrap());

    harness
        .ask(|data, reply| data.remove_download(guid.clone(), reply))
        .unwrap();
    assert!(matches!(
        harness.ask(|data, reply| data.resume_download(guid.clone(), reply)),
        Err(ProfileDataError::Store(StoreError::NotFound))
    ));
}

/// A database whose table pages are damaged but whose header and schema are readable: the
/// store opens it; only `quick_check` notices.
fn damaged_database(path: &Path) {
    let mut store = Store::open(path).unwrap();
    let visits = (0..2000)
        .map(|n| format!("https://{n}.test/some/longer/path"))
        .collect::<Vec<_>>();
    let imports = visits
        .iter()
        .map(|url| VisitImport {
            url,
            title: Some("A title"),
            visited_at_ms: NOW,
            transition: VisitTransition::Link,
        })
        .collect::<Vec<_>>();
    store.import_visits(&imports, NOW).unwrap();
    drop(store);
    let mut bytes = std::fs::read(path).unwrap();
    bytes[4 * 4096..9 * 4096].fill(0xA5);
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn the_check_is_skipped_within_the_week_after_a_clean_exit() {
    let harness = Harness::with(
        |path| {
            damaged_database(path);
            std::fs::write(check_marker(path), now_ms().to_string()).unwrap();
        },
        Timing::default(),
    );

    // Not checked, so the damage goes unnoticed at open (the store reads only the schema).
    assert!(
        harness
            .ask(|data, reply| data.bookmark_root(BookmarkRoot::Toolbar, reply))
            .is_ok()
    );
}

#[test]
fn the_check_runs_after_an_unclean_exit_even_within_the_week() {
    let harness = Harness::with(
        |path| {
            damaged_database(path);
            std::fs::write(check_marker(path), now_ms().to_string()).unwrap();
            // What a run that did not close the database leaves behind.
            std::fs::write(path.with_extension("sqlite-wal"), b"").unwrap();
        },
        Timing::default(),
    );

    assert_unavailable(&harness, "could not be opened");
}

#[test]
fn a_check_is_due_when_none_is_recorded_or_the_last_is_a_week_old() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("places.sqlite");
    let marker = check_marker(&database);
    let now = SystemTime::now();

    assert!(check_is_due(&database, &marker, now));
    std::fs::write(&marker, "garbage").unwrap();
    assert!(check_is_due(&database, &marker, now));
    std::fs::write(&marker, now_ms().to_string()).unwrap();
    let now = SystemTime::now();
    assert!(!check_is_due(&database, &marker, now));
    assert!(check_is_due(&database, &marker, now + CHECK_INTERVAL));
    // A check recorded in the future (the clock went back) is not trusted.
    assert!(check_is_due(
        &database,
        &marker,
        now - Duration::from_secs(3600)
    ));
}

#[test]
fn a_successful_check_is_recorded() {
    let harness = Harness::new();
    harness.sync();

    let recorded = std::fs::read_to_string(check_marker(&harness.path())).unwrap();
    assert!(recorded.trim().parse::<i64>().unwrap() > 0);
    #[cfg(unix)]
    assert_eq!(mode(&check_marker(&harness.path())), 0o600);
}
