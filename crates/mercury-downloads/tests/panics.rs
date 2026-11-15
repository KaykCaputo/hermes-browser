//! A panic inside a transfer — injected here through the public `on_event` callback, which the
//! transfer calls from inside its task — must still end the download properly.

mod support;

use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use mercury_downloads::{DownloadEvent, DownloadId, DownloadManager};
use support::*;

/// A manager whose callback forwards every event, then panics when `panics_on` says so.
fn panicking_manager(
    panics_on: impl Fn(&DownloadEvent) -> bool + Send + Sync + 'static,
) -> (DownloadManager, Receiver<DownloadEvent>) {
    let (sender, receiver) = mpsc::channel();
    let manager = DownloadManager::new(move |event| {
        let panic_now = panics_on(&event);
        let _ = sender.send(event);
        if panic_now {
            panic!("injected panic in on_event");
        }
    })
    .expect("start the download manager");
    (manager, receiver)
}

fn assert_internal_failure(events: &[DownloadEvent]) {
    match terminal(events) {
        DownloadEvent::Failed {
            error, resumable, ..
        } => {
            assert!(error.contains("internally"), "{error}");
            assert!(!resumable);
        }
        other => panic!("expected an internal failure, got {other:?}"),
    }
    let terminals = events.iter().filter(|event| is_terminal(event)).count();
    assert_eq!(terminals, 1, "{events:#?}");
}

#[test]
fn a_callback_panicking_on_progress_fails_the_download_and_cleans_up() {
    let server = TestServer::start();
    let dir = TempDir::new("panic-progress");
    let (manager, events) = panicking_manager(
        |event| matches!(event, DownloadEvent::Progress { id, .. } if *id == DownloadId(1)),
    );
    let first = manager.start(request(server.url("/slow"), &dir.path));
    let done = events_until_done(&events, first);
    assert_internal_failure(&done);
    assert_eq!(manager.active(), 0);
    assert!(TempDir::list(&dir.path).is_empty());

    // The manager keeps working.
    let second = manager.start(request(server.url("/file.bin"), &dir.path));
    let done = events_until_done(&events, second);
    assert!(
        matches!(terminal(&done), DownloadEvent::Finished { .. }),
        "{done:#?}"
    );
}

#[test]
fn a_callback_panicking_on_started_removes_the_partial_file() {
    let server = TestServer::start();
    let dir = TempDir::new("panic-started");
    let (manager, events) =
        panicking_manager(|event| matches!(event, DownloadEvent::Started { .. }));
    let id = manager.start(request(server.url("/file.bin"), &dir.path));
    let done = events_until_done(&events, id);
    assert!(matches!(done[0], DownloadEvent::Started { .. }));
    assert_internal_failure(&done);
    assert_eq!(manager.active(), 0);
    assert!(TempDir::list(&dir.path).is_empty());
}

#[test]
fn a_callback_panicking_on_the_terminal_event_leaves_consistent_state() {
    let server = TestServer::start();
    let dir = TempDir::new("panic-terminal");
    let (manager, events) = panicking_manager(
        |event| matches!(event, DownloadEvent::Finished { id, .. } if *id == DownloadId(1)),
    );
    let first = manager.start(request(server.url("/file.bin"), &dir.path));
    let done = events_until_done(&events, first);
    let DownloadEvent::Finished { target_path, .. } = terminal(&done) else {
        panic!("{done:#?}")
    };
    // The file is complete; the panic did not undo it or produce a second terminal event.
    assert_eq!(std::fs::read(target_path).unwrap(), pattern(FILE_LEN));
    let second = manager.start(request(server.url("/file.bin"), &dir.path));
    let later = events_until_done(&events, second);
    assert!(matches!(terminal(&later), DownloadEvent::Finished { .. }));
    assert_eq!(manager.active(), 0);

    // No task is counted as still alive, so dropping does not wait for the grace period.
    let asked = Instant::now();
    drop(manager);
    assert!(
        asked.elapsed() < Duration::from_millis(500),
        "{:?}",
        asked.elapsed()
    );
    assert!(
        events
            .try_iter()
            .all(|event| !(is_terminal(&event) && event_id(&event) == first))
    );
}
