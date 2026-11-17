//! Interrupting a download (the browser quitting) keeps what can be resumed, unlike cancel.

mod support;

use std::time::{Duration, Instant};

use mercury_downloads::{DownloadEvent, ResumeFrom, remove_partial_file};
use support::*;

/// Generous for a loaded machine, far shorter than the 83-minute stream that would otherwise
/// run on.
const PROMPTLY: Duration = Duration::from_secs(10);

fn partial_of(started: &DownloadEvent) -> (std::path::PathBuf, std::path::PathBuf) {
    match started {
        DownloadEvent::Started {
            partial_path,
            target_path,
            ..
        } => (partial_path.clone(), target_path.clone()),
        other => panic!("expected Started, got {other:?}"),
    }
}

fn wait_for_progress(events: &std::sync::mpsc::Receiver<DownloadEvent>) {
    loop {
        let event = events.recv_timeout(TEST_TIMEOUT).expect("progress");
        assert!(!is_terminal(&event), "{event:?}");
        if matches!(event, DownloadEvent::Progress { .. }) {
            return;
        }
    }
}

#[test]
fn interrupting_a_resumable_download_keeps_its_partial_file() {
    let server = TestServer::start();
    let dir = TempDir::new("interrupt-resumable");
    let (manager, events) = manager();
    let id = manager.start(request(server.url("/slow-long"), &dir.path));
    let (partial, target) = partial_of(&wait_started(&events, id));
    wait_for_progress(&events);

    let asked = Instant::now();
    manager.interrupt(id);
    let rest = events_until_done(&events, id);
    assert!(asked.elapsed() < PROMPTLY, "{:?}", asked.elapsed());
    match terminal(&rest) {
        DownloadEvent::Failed {
            error, resumable, ..
        } => {
            assert!(*resumable, "{error}");
            assert!(error.contains("interrupted"), "{error}");
        }
        other => panic!("expected Failed, got {other:?}"),
    }
    assert!(std::fs::metadata(&partial).unwrap().len() > 0);
    assert!(!target.exists());
    assert_eq!(manager.active(), 0);

    // Idempotent, and nothing follows the terminal event.
    manager.interrupt(id);
    assert!(events.recv_timeout(Duration::from_millis(300)).is_err());
}

#[test]
fn interrupting_a_download_that_cannot_resume_removes_its_partial_file() {
    let server = TestServer::start();
    let dir = TempDir::new("interrupt-plain");
    let (manager, events) = manager();
    let id = manager.start(request(server.url("/slow-plain"), &dir.path));
    wait_started(&events, id);
    wait_for_progress(&events);

    manager.interrupt(id);
    let rest = events_until_done(&events, id);
    match terminal(&rest) {
        DownloadEvent::Failed { resumable, .. } => assert!(!resumable),
        other => panic!("expected Failed, got {other:?}"),
    }
    assert!(TempDir::list(&dir.path).is_empty());
}

#[test]
fn interrupting_before_the_answer_leaves_nothing() {
    let server = TestServer::start();
    let parent = TempDir::new("interrupt-early");
    let dir = parent.path.join("downloads");
    let (manager, events) = manager();
    let id = manager.start(request(server.url("/slow-headers"), &dir));
    std::thread::sleep(Duration::from_millis(200));
    manager.interrupt(id);
    let done = events_until_done(&events, id);
    assert_eq!(done.len(), 1, "{done:#?}");
    assert!(matches!(
        done[0],
        DownloadEvent::Failed {
            resumable: false,
            ..
        }
    ));
    assert!(TempDir::list(&dir).is_empty());
}

#[test]
fn an_interrupted_download_resumes_to_the_whole_file() {
    let server = TestServer::start();
    let dir = TempDir::new("interrupt-resume");
    let (manager, events) = manager();
    let id = manager.start(request(server.url("/slow-file.bin"), &dir.path));
    let started = wait_started(&events, id);
    wait_for_progress(&events);
    manager.interrupt(id);
    let rest = events_until_done(&events, id);
    match terminal(&rest) {
        DownloadEvent::Failed { resumable, .. } => assert!(*resumable, "{rest:#?}"),
        other => panic!("expected Failed, got {other:?}"),
    }
    let (partial, target) = partial_of(&started);
    let kept = std::fs::metadata(&partial).unwrap().len();
    assert!(kept > 0 && kept < FILE_LEN as u64, "{kept} bytes kept");

    let mut request = request(server.url("/slow-file.bin"), &dir.path);
    request.resume = Some(resume_from(partial, target.clone(), Some(ETAG), None));
    let second = manager.start(request);
    let rest = events_until_done(&events, second);
    assert!(
        matches!(terminal(&rest), DownloadEvent::Finished { .. }),
        "{rest:#?}"
    );
    assert_eq!(std::fs::read(&target).unwrap(), pattern(FILE_LEN));
    // The second request asked only for the rest.
    let ranges: Vec<Option<String>> = server
        .requests()
        .iter()
        .map(|request| request.header("range").map(str::to_owned))
        .collect();
    assert_eq!(ranges, [None, Some(format!("bytes={kept}-"))]);
}

#[test]
fn removing_a_partial_file_removes_only_a_regular_file_named_for_its_target() {
    let dir = TempDir::new("remove-partial");
    let target = dir.path.join("file.bin");
    let partial = dir.path.join("file.bin.part");
    std::fs::write(&partial, b"part").unwrap();
    let resume = ResumeFrom {
        partial_path: partial.clone(),
        target_path: target.clone(),
        etag: None,
        last_modified: None,
    };
    remove_partial_file(&resume).unwrap();
    assert!(!partial.exists());
    // Already gone: nothing to do.
    remove_partial_file(&resume).unwrap();

    // A path that is not `<target>.part` is refused and left alone.
    let other = dir.path.join("notes.txt");
    std::fs::write(&other, b"keep").unwrap();
    let wrong = ResumeFrom {
        partial_path: other.clone(),
        ..resume.clone()
    };
    assert!(remove_partial_file(&wrong).is_err());
    assert_eq!(std::fs::read(&other).unwrap(), b"keep");

    // A partial file in another folder than its target is refused too.
    let elsewhere = TempDir::new("remove-partial-elsewhere");
    let stray = elsewhere.path.join("file.bin.part");
    std::fs::write(&stray, b"keep").unwrap();
    let split = ResumeFrom {
        partial_path: stray.clone(),
        ..resume.clone()
    };
    assert!(remove_partial_file(&split).is_err());
    assert!(stray.exists());
}

#[cfg(unix)]
#[test]
fn removing_a_partial_file_never_follows_a_link() {
    let dir = TempDir::new("remove-partial-link");
    let victim = dir.path.join("victim.txt");
    std::fs::write(&victim, b"victim").unwrap();
    let partial = dir.path.join("file.bin.part");
    std::os::unix::fs::symlink(&victim, &partial).unwrap();
    let resume = ResumeFrom {
        partial_path: partial.clone(),
        target_path: dir.path.join("file.bin"),
        etag: None,
        last_modified: None,
    };
    assert!(remove_partial_file(&resume).is_err());
    assert!(partial.symlink_metadata().unwrap().file_type().is_symlink());
    assert_eq!(std::fs::read(&victim).unwrap(), b"victim");
}

#[test]
fn shutting_down_returns_by_its_deadline_even_if_a_transfer_is_slow_to_end() {
    use std::sync::{Arc, Condvar, Mutex};

    let server = TestServer::start();
    let dir = TempDir::new("shutdown-deadline");
    // Delivering the terminal event blocks until the test lets it go, after `shut_down` has
    // returned: the transfer cannot end before then.
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    let held = Arc::clone(&release);
    let (sender, events) = std::sync::mpsc::channel();
    let manager = mercury_downloads::DownloadManager::new(move |event| {
        let terminal = is_terminal(&event);
        let _ = sender.send(event);
        if terminal {
            let (released, wake) = &*held;
            let mut released = released.lock().unwrap();
            while !*released {
                released = wake.wait(released).unwrap();
            }
        }
    })
    .unwrap();
    let id = manager.start(request(server.url("/slow-long"), &dir.path));
    wait_started(&events, id);

    let asked = Instant::now();
    manager.shut_down(asked + Duration::from_millis(50));
    let took = asked.elapsed();
    let (released, wake) = &*release;
    *released.lock().unwrap() = true;
    wake.notify_all();
    // Without the deadline it would wait its three-second grace.
    assert!(took < Duration::from_secs(2), "{took:?}");
}
