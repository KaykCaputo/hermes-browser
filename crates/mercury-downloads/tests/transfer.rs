mod support;

#[cfg(unix)]
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mercury_downloads::{DownloadEvent, DownloadId, Timeouts};
use support::*;
use url::Url;

fn started_fields(event: &DownloadEvent) -> (String, PathBuf, PathBuf, Option<u64>) {
    match event {
        DownloadEvent::Started {
            file_name,
            target_path,
            partial_path,
            total_bytes,
            ..
        } => (
            file_name.clone(),
            target_path.clone(),
            partial_path.clone(),
            *total_bytes,
        ),
        other => panic!("expected Started, got {other:?}"),
    }
}

fn finished_path(event: &DownloadEvent) -> PathBuf {
    match event {
        DownloadEvent::Finished { target_path, .. } => target_path.clone(),
        other => panic!("expected Finished, got {other:?}"),
    }
}

fn failed(event: &DownloadEvent) -> (String, bool) {
    match event {
        DownloadEvent::Failed {
            error, resumable, ..
        } => (error.clone(), *resumable),
        other => panic!("expected Failed, got {other:?}"),
    }
}

/// Exactly one `Started` first (when there is one), only `Progress` in between, and exactly one
/// terminal event last.
fn assert_well_formed(events: &[DownloadEvent], expect_started: bool) {
    assert!(is_terminal(terminal(events)), "{events:#?}");
    let starts = events
        .iter()
        .filter(|event| matches!(event, DownloadEvent::Started { .. }))
        .count();
    assert_eq!(starts, usize::from(expect_started), "{events:#?}");
    if expect_started {
        assert!(
            matches!(events[0], DownloadEvent::Started { .. }),
            "{events:#?}"
        );
    }
    let skip = usize::from(expect_started);
    for event in &events[skip..events.len() - 1] {
        assert!(
            matches!(event, DownloadEvent::Progress { .. }),
            "unexpected {event:?} in {events:#?}"
        );
    }
}

fn download(url: Url, dir: &Path) -> Vec<DownloadEvent> {
    let (manager, events) = manager();
    let id = manager.start(request(url, dir));
    events_until_done(&events, id)
}

// --- 4. Transfer ---------------------------------------------------------------------------

#[test]
fn a_file_is_streamed_to_a_partial_file_and_renamed_when_complete() {
    let server = TestServer::start();
    let dir = TempDir::new("plain");
    let events = download(server.url("/file.bin"), &dir.path);

    assert_well_formed(&events, true);
    let (file_name, target_path, partial_path, total) = started_fields(&events[0]);
    assert_eq!(file_name, "file.bin");
    assert_eq!(target_path, dir.path.join("file.bin"));
    assert_eq!(partial_path, dir.path.join("file.bin.part"));
    assert_eq!(total, Some(FILE_LEN as u64));
    match &events[0] {
        DownloadEvent::Started {
            mime_type,
            final_url,
            etag,
            ..
        } => {
            assert_eq!(mime_type.as_deref(), Some("application/octet-stream"));
            assert_eq!(final_url, &server.url("/file.bin"));
            assert_eq!(etag.as_deref(), Some(ETAG));
        }
        _ => unreachable!(),
    }

    let finished = finished_path(terminal(&events));
    assert_eq!(finished, target_path);
    match terminal(&events) {
        DownloadEvent::Finished { received_bytes, .. } => {
            assert_eq!(*received_bytes, FILE_LEN as u64)
        }
        _ => unreachable!(),
    }
    assert_eq!(std::fs::read(&finished).unwrap(), pattern(FILE_LEN));
    assert_eq!(TempDir::list(&dir.path), ["file.bin"]);
    // Nothing was written anywhere but the chosen folder.
    assert_eq!(
        finished.canonicalize().unwrap().parent().unwrap(),
        dir.path.canonicalize().unwrap()
    );
}

#[test]
fn a_missing_destination_folder_is_created() {
    let server = TestServer::start();
    let dir = TempDir::new("create");
    let nested = dir.path.join("a").join("b");
    let events = download(server.url("/file.bin"), &nested);
    assert_eq!(finished_path(terminal(&events)), nested.join("file.bin"));
}

#[test]
fn the_content_disposition_name_is_used() {
    let server = TestServer::start();
    let dir = TempDir::new("attachment");
    let events = download(server.url("/attachment"), &dir.path);
    assert_eq!(
        finished_path(terminal(&events)),
        dir.path.join("report.pdf")
    );
    assert_eq!(TempDir::list(&dir.path), ["report.pdf"]);
}

#[test]
fn a_name_without_an_extension_takes_one_from_the_content_type() {
    let server = TestServer::start();
    let dir = TempDir::new("typed");
    let events = download(server.url("/typed"), &dir.path);
    assert_eq!(finished_path(terminal(&events)), dir.path.join("typed.txt"));
}

#[test]
fn the_suggested_name_is_sanitised_and_used() {
    let server = TestServer::start();
    let dir = TempDir::new("suggested");
    let (manager, events) = manager();
    let mut request = request(server.url("/plain.bin"), &dir.path);
    request.suggested_name = Some("../../chosen.bin".to_owned());
    let id = manager.start(request);
    let events = events_until_done(&events, id);
    assert_eq!(
        finished_path(terminal(&events)),
        dir.path.join("_.._chosen.bin")
    );
}

#[test]
fn a_hostile_server_name_cannot_leave_the_folder() {
    let server = TestServer::start();
    let parent = TempDir::new("escape-parent");
    let dir = parent.path.join("inner").join("downloads");
    let events = download(server.url("/escape"), &dir);
    let finished = finished_path(terminal(&events));
    assert_eq!(finished, dir.join("_.._escape.txt"));
    assert_eq!(
        finished.canonicalize().unwrap().parent().unwrap(),
        dir.canonicalize().unwrap()
    );
    assert_eq!(TempDir::list(&parent.path), ["inner"]);
    assert_eq!(TempDir::list(&parent.path.join("inner")), ["downloads"]);
}

#[test]
fn an_existing_file_is_never_overwritten() {
    let server = TestServer::start();
    let dir = TempDir::new("existing");
    std::fs::write(dir.path.join("file.bin"), b"keep me").unwrap();
    let events = download(server.url("/file.bin"), &dir.path);
    assert_eq!(
        finished_path(terminal(&events)),
        dir.path.join("file (1).bin")
    );
    assert_eq!(
        std::fs::read(dir.path.join("file.bin")).unwrap(),
        b"keep me"
    );
    assert_eq!(
        std::fs::read(dir.path.join("file (1).bin")).unwrap(),
        pattern(FILE_LEN)
    );
}

#[cfg(unix)]
#[test]
fn a_symlink_at_the_target_or_partial_path_is_not_followed() {
    let server = TestServer::start();
    let outside = TempDir::new("victim");
    let victim = outside.path.join("victim.txt");
    std::fs::write(&victim, b"victim").unwrap();
    let dir = TempDir::new("symlinks");
    std::os::unix::fs::symlink(&victim, dir.path.join("file.bin")).unwrap();
    std::os::unix::fs::symlink(&victim, dir.path.join("file (1).bin.part")).unwrap();
    // A dangling link must also count as taken.
    std::os::unix::fs::symlink(outside.path.join("nothing"), dir.path.join("file (2).bin"))
        .unwrap();

    let events = download(server.url("/file.bin"), &dir.path);
    assert_eq!(
        finished_path(terminal(&events)),
        dir.path.join("file (3).bin")
    );
    assert_eq!(std::fs::read(&victim).unwrap(), b"victim");
    assert_eq!(TempDir::list(&outside.path), ["victim.txt"]);
}

#[cfg(unix)]
#[test]
fn a_name_taken_during_the_download_is_not_replaced_when_publishing() {
    let server = TestServer::start();
    let outside = TempDir::new("late-victim");
    let victim = outside.path.join("victim.txt");
    std::fs::write(&victim, b"victim").unwrap();
    let dir = TempDir::new("late");
    let (manager, events) = manager();
    let id = manager.start(request(server.url("/slow"), &dir.path));
    let started = wait_started(&events, id);
    let (_, target_path, _, _) = started_fields(&started);
    // Someone puts a link at the final name while the body is still arriving.
    std::os::unix::fs::symlink(&victim, &target_path).unwrap();

    let rest = events_until_done(&events, id);
    let finished = finished_path(terminal(&rest));
    assert_eq!(finished, dir.path.join("slow (1)"));
    assert!(
        target_path
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(std::fs::read(&victim).unwrap(), b"victim");
    assert_eq!(
        std::fs::read(&finished).unwrap().len(),
        SLOW_CHUNKS * SLOW_CHUNK
    );
    assert_eq!(TempDir::list(&dir.path), ["slow", "slow (1)"]);
}

#[test]
fn a_server_name_with_a_huge_extension_can_be_downloaded_twice() {
    let server = TestServer::start();
    let dir = TempDir::new("long-extension");
    let (manager, events) = manager();
    let mut paths = Vec::new();
    for _ in 0..2 {
        let id = manager.start(request(server.url("/long-extension"), &dir.path));
        let done = events_until_done(&events, id);
        paths.push(finished_path(terminal(&done)));
    }
    assert_ne!(paths[0], paths[1]);
    for path in &paths {
        assert_eq!(std::fs::read(path).unwrap(), b"long");
        let name = path.file_name().unwrap().to_str().unwrap();
        assert!(name.len() + ".part".len() <= 255, "{} bytes", name.len());
    }
    assert_eq!(TempDir::list(&dir.path).len(), 2);
}

#[test]
fn concurrent_downloads_of_one_name_get_different_files() {
    let server = TestServer::start();
    let dir = TempDir::new("concurrent");
    let (manager, events) = manager();
    let ids: Vec<DownloadId> = (0..4)
        .map(|_| manager.start(request(server.url("/file.bin"), &dir.path)))
        .collect();
    let mut paths: Vec<PathBuf> = Vec::new();
    let deadline = Instant::now() + TEST_TIMEOUT;
    while paths.len() < ids.len() {
        let left = deadline.saturating_duration_since(Instant::now());
        let event = events.recv_timeout(left).expect("all four should finish");
        if let DownloadEvent::Finished { target_path, .. } = event {
            paths.push(target_path);
        } else {
            assert!(!is_terminal(&event), "{event:?}");
        }
    }
    paths.sort();
    paths.dedup();
    assert_eq!(paths.len(), 4, "{paths:?}");
    for path in &paths {
        assert_eq!(std::fs::read(path).unwrap(), pattern(FILE_LEN));
    }
}

// --- 5. Headers ----------------------------------------------------------------------------

#[test]
fn the_user_agent_referrer_and_cookie_are_sent() {
    let server = TestServer::start();
    let dir = TempDir::new("headers");
    let (manager, events) = manager();
    let mut request = request(server.url("/file.bin"), &dir.path);
    request.referrer = Some(server.url("/page?q=1#section"));
    request.cookie_header = Some("session=abc; theme=dark".to_owned());
    let id = manager.start(request);
    events_until_done(&events, id);

    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    let first = &requests[0];
    assert_eq!(first.header("user-agent"), Some("MercuryTest/1.0"));
    // Same origin: the full URL, without the fragment.
    let page = format!("http://127.0.0.1:{}/page?q=1", server.addr.port());
    assert_eq!(first.header("referer"), Some(page.as_str()));
    assert_eq!(first.header("cookie"), Some("session=abc; theme=dark"));
}

#[test]
fn the_cookie_is_kept_on_a_same_host_redirect() {
    let server = TestServer::start();
    let dir = TempDir::new("same-host");
    let (manager, events) = manager();
    let mut request = request(server.url("/redirect-same-host"), &dir.path);
    request.cookie_header = Some("session=abc".to_owned());
    let id = manager.start(request);
    let events = events_until_done(&events, id);
    finished_path(terminal(&events));

    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].path, "/file.bin");
    assert_eq!(requests[1].header("cookie"), Some("session=abc"));
}

#[test]
fn the_cookie_is_not_forwarded_when_a_redirect_changes_host() {
    let server = TestServer::start();
    let dir = TempDir::new("cross-host");
    let (manager, events) = manager();
    let mut request = request(server.url("/redirect-other-host"), &dir.path);
    request.cookie_header = Some("session=abc".to_owned());
    let id = manager.start(request);
    let events = events_until_done(&events, id);
    let finished = finished_path(terminal(&events));
    assert_eq!(finished, dir.path.join("file.bin"));
    match &events[0] {
        DownloadEvent::Started { final_url, .. } => {
            assert_eq!(final_url.host_str(), Some("localhost"))
        }
        other => panic!("{other:?}"),
    }

    let requests = server.requests();
    assert_eq!(requests.len(), 2, "{requests:#?}");
    assert_eq!(requests[0].header("cookie"), Some("session=abc"));
    assert!(
        requests[1]
            .header("host")
            .unwrap()
            .starts_with("localhost:")
    );
    assert_eq!(requests[1].header("cookie"), None, "{:#?}", requests[1]);
    assert_eq!(requests[1].header("user-agent"), Some("MercuryTest/1.0"));
}

#[test]
fn a_cross_origin_request_gets_only_the_referrer_origin() {
    let server = TestServer::start();
    let dir = TempDir::new("referrer-cross");
    let (manager, events) = manager();
    let mut request = request(server.url("/file.bin"), &dir.path);
    request.referrer = Some(Url::parse("http://user:pw@example.test:8080/page?q=1#x").unwrap());
    let id = manager.start(request);
    events_until_done(&events, id);
    assert_eq!(
        server.requests()[0].header("referer"),
        Some("http://example.test:8080/")
    );
}

#[test]
fn the_referrer_is_trimmed_to_its_origin_when_a_redirect_changes_origin() {
    let server = TestServer::start();
    let dir = TempDir::new("referrer-redirect");
    let (manager, events) = manager();
    let mut request = request(server.url("/redirect-other-host"), &dir.path);
    request.referrer = Some(server.url("/page?q=1"));
    let id = manager.start(request);
    let done = events_until_done(&events, id);
    finished_path(terminal(&done));

    let port = server.addr.port();
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    let full = format!("http://127.0.0.1:{port}/page?q=1");
    let origin = format!("http://127.0.0.1:{port}/");
    assert_eq!(requests[0].header("referer"), Some(full.as_str()));
    assert_eq!(requests[1].header("referer"), Some(origin.as_str()));
}

#[test]
fn the_final_url_names_the_file_after_a_redirect() {
    let server = TestServer::start();
    let dir = TempDir::new("redirect");
    let events = download(server.url("/redirect"), &dir.path);
    assert_eq!(finished_path(terminal(&events)), dir.path.join("file.bin"));
    match &events[0] {
        DownloadEvent::Started { final_url, .. } => {
            assert_eq!(final_url, &server.url("/file.bin"))
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn non_http_schemes_are_rejected_before_any_io() {
    let server = TestServer::start();
    let parent = TempDir::new("schemes");
    let dir = parent.path.join("never-created");
    let (manager, events) = manager();
    for url in [
        "file:///etc/passwd",
        "ftp://127.0.0.1/file.bin",
        "data:text/plain,hello",
        "javascript:alert(1)",
        "blob:http://127.0.0.1/uuid",
    ] {
        let id = manager.start(request(Url::parse(url).unwrap(), &dir));
        let events = events_until_done(&events, id);
        assert_well_formed(&events, false);
        let (error, resumable) = failed(terminal(&events));
        assert!(error.contains("http"), "{url}: {error}");
        assert!(!resumable);
    }
    assert!(!dir.exists());
    assert!(server.requests().is_empty());
}

#[test]
fn a_redirect_to_a_non_http_scheme_is_rejected() {
    let server = TestServer::start();
    let dir = TempDir::new("redirect-scheme");
    let events = download(server.url("/redirect-file-scheme"), &dir.path);
    assert_well_formed(&events, false);
    let (error, resumable) = failed(terminal(&events));
    assert!(error.contains("file"), "{error}");
    assert!(!resumable);
    assert!(TempDir::list(&dir.path).is_empty());
}

#[test]
fn a_redirect_loop_fails() {
    let server = TestServer::start();
    let dir = TempDir::new("loop");
    let events = download(server.url("/redirect-loop"), &dir.path);
    let (error, resumable) = failed(terminal(&events));
    assert!(error.contains("redirect"), "{error}");
    assert!(!resumable);
    assert!(TempDir::list(&dir.path).is_empty());
}

#[test]
fn header_values_that_could_inject_headers_are_refused() {
    let server = TestServer::start();
    let dir = TempDir::new("inject");
    let (manager, events) = manager();
    let mut request = request(server.url("/file.bin"), &dir.path);
    request.cookie_header = Some("a=b\r\nX-Injected: yes".to_owned());
    let id = manager.start(request);
    let events = events_until_done(&events, id);
    let (error, _) = failed(terminal(&events));
    assert!(error.contains("Cookie"), "{error}");
    assert!(server.requests().is_empty());
}

// --- 6. Events -----------------------------------------------------------------------------

#[test]
fn progress_is_throttled_and_ends_at_the_total() {
    let server = TestServer::start();
    let dir = TempDir::new("progress");
    let began = Instant::now();
    let events = download(server.url("/slow"), &dir.path);
    let elapsed = began.elapsed();
    assert_well_formed(&events, true);

    let total = (SLOW_CHUNKS * SLOW_CHUNK) as u64;
    let progress: Vec<(u64, Option<u64>, u64)> = events
        .iter()
        .filter_map(|event| match event {
            DownloadEvent::Progress {
                received_bytes,
                total_bytes,
                bytes_per_second,
                ..
            } => Some((*received_bytes, *total_bytes, *bytes_per_second)),
            _ => None,
        })
        .collect();
    // About four a second, plus the final one.
    let allowed = (elapsed.as_secs_f64() * 4.0).ceil() as usize + 2;
    assert!(
        progress.len() >= 2 && progress.len() <= allowed,
        "{} progress events in {elapsed:?}",
        progress.len()
    );
    assert_eq!(progress.last().unwrap().0, total);
    assert!(progress.iter().all(|(_, of, _)| *of == Some(total)));
    assert!(progress.windows(2).all(|pair| pair[0].0 <= pair[1].0));
    // The server sends about 20 KiB a second; the average should be in that region.
    let speed = progress[progress.len() / 2].2;
    assert!((5_000..80_000).contains(&speed), "speed {speed} B/s");
}

#[test]
fn the_total_is_unknown_without_a_content_length() {
    let server = TestServer::start();
    let dir = TempDir::new("no-length");
    let events = download(server.url("/no-length"), &dir.path);
    let (_, _, _, total) = started_fields(&events[0]);
    assert_eq!(total, None);
    let finished = finished_path(terminal(&events));
    assert_eq!(std::fs::read(finished).unwrap(), pattern(FILE_LEN));
}

// --- 7. Failure ----------------------------------------------------------------------------

#[test]
fn an_http_error_fails_with_the_status_and_leaves_nothing() {
    let server = TestServer::start();
    let dir = TempDir::new("404");
    let events = download(server.url("/missing"), &dir.path);
    assert_well_formed(&events, false);
    let (error, resumable) = failed(terminal(&events));
    assert!(error.contains("404"), "{error}");
    assert!(!resumable);
    assert!(TempDir::list(&dir.path).is_empty());

    let events = download(server.url("/broken"), &dir.path);
    let (error, _) = failed(terminal(&events));
    assert!(error.contains("500"), "{error}");
    assert!(TempDir::list(&dir.path).is_empty());
}

#[test]
fn a_body_cut_short_with_validators_is_resumable_and_keeps_the_partial_file() {
    let server = TestServer::start();
    let dir = TempDir::new("cut");
    let events = download(server.url("/cut"), &dir.path);
    assert_well_formed(&events, true);
    let (_, _, partial_path, _) = started_fields(&events[0]);
    let (error, resumable) = failed(terminal(&events));
    assert!(resumable, "{error}");
    assert_eq!(TempDir::list(&dir.path), ["cut.part"]);
    assert_eq!(std::fs::read(partial_path).unwrap(), pattern(CUT_SENT));
}

#[test]
fn a_body_cut_short_without_validators_is_not_resumable_and_leaves_nothing() {
    let server = TestServer::start();
    let dir = TempDir::new("cut-plain");
    let events = download(server.url("/cut-plain"), &dir.path);
    let (_, resumable) = failed(terminal(&events));
    assert!(!resumable);
    assert!(TempDir::list(&dir.path).is_empty());
}

// --- 8. Resume -----------------------------------------------------------------------------

fn write_partial(dir: &Path, name: &str, len: usize) -> (PathBuf, PathBuf) {
    let partial = dir.join(format!("{name}.part"));
    std::fs::write(&partial, &pattern(FILE_LEN)[..len]).unwrap();
    (partial, dir.join(name))
}

#[test]
fn a_resume_with_a_matching_validator_appends_the_rest() {
    let server = TestServer::start();
    let dir = TempDir::new("resume");
    let (partial, target) = write_partial(&dir.path, "file.bin", 10_000);
    let (manager, events) = manager();
    let mut request = request(server.url("/file.bin"), &dir.path);
    request.resume = Some(resume_from(
        partial.clone(),
        target.clone(),
        Some(ETAG),
        None,
    ));
    let id = manager.start(request);
    let events = events_until_done(&events, id);

    assert_well_formed(&events, true);
    let (_, started_target, started_partial, total) = started_fields(&events[0]);
    assert_eq!(started_target, target);
    assert_eq!(started_partial, partial);
    assert_eq!(total, Some(FILE_LEN as u64));
    assert_eq!(finished_path(terminal(&events)), target);
    assert_eq!(std::fs::read(&target).unwrap(), pattern(FILE_LEN));
    assert_eq!(TempDir::list(&dir.path), ["file.bin"]);

    let requests = server.requests();
    assert_eq!(requests[0].header("range"), Some("bytes=10000-"));
    assert_eq!(requests[0].header("if-range"), Some(ETAG));
}

#[test]
fn a_resume_can_use_last_modified_as_the_validator() {
    let server = TestServer::start();
    let dir = TempDir::new("resume-lm");
    let (partial, target) = write_partial(&dir.path, "lm.bin", 5_000);
    let (manager, events) = manager();
    let mut request = request(server.url("/lm.bin"), &dir.path);
    request.resume = Some(resume_from(
        partial,
        target.clone(),
        None,
        Some(LAST_MODIFIED),
    ));
    let id = manager.start(request);
    let events = events_until_done(&events, id);
    assert_eq!(finished_path(terminal(&events)), target);
    assert_eq!(std::fs::read(&target).unwrap(), pattern(FILE_LEN));
    assert_eq!(server.requests()[0].header("if-range"), Some(LAST_MODIFIED));
}

#[test]
fn a_resume_with_a_stale_validator_starts_again_from_zero() {
    let server = TestServer::start();
    let dir = TempDir::new("resume-stale");
    let partial = dir.path.join("file.bin.part");
    // Contents that do not match the server's: they must not survive.
    std::fs::write(&partial, vec![b'x'; 30_000]).unwrap();
    let target = dir.path.join("file.bin");
    let (manager, events) = manager();
    let mut request = request(server.url("/file.bin"), &dir.path);
    request.resume = Some(resume_from(partial, target.clone(), Some("\"old\""), None));
    let id = manager.start(request);
    let events = events_until_done(&events, id);
    assert_eq!(finished_path(terminal(&events)), target);
    assert_eq!(std::fs::read(&target).unwrap(), pattern(FILE_LEN));
    assert_eq!(server.requests()[0].header("if-range"), Some("\"old\""));
}

#[test]
fn a_cut_download_can_be_resumed_from_its_own_events() {
    let server = TestServer::start();
    let dir = TempDir::new("resume-own");
    // First attempt: cut short, keeps the partial file.
    let (manager, events) = manager_with(Timeouts {
        idle: Duration::from_millis(300),
        ..Timeouts::default()
    });
    let id = manager.start(request(server.url("/stall"), &dir.path));
    let first = events_until_done(&events, id);
    let (error, resumable) = failed(terminal(&first));
    assert!(resumable, "{error}");
    let DownloadEvent::Started {
        partial_path,
        target_path,
        etag,
        last_modified,
        ..
    } = first[0].clone()
    else {
        panic!("{first:#?}")
    };
    assert_eq!(std::fs::metadata(&partial_path).unwrap().len(), 1000);

    // Second attempt against the full file, with the validators from the first.
    let mut request = request(server.url("/file.bin"), &dir.path);
    request.resume = Some(mercury_downloads::ResumeFrom {
        partial_path,
        target_path: target_path.clone(),
        etag,
        last_modified,
    });
    let id = manager.start(request);
    let second = events_until_done(&events, id);
    assert_eq!(finished_path(terminal(&second)), target_path);
    assert_eq!(std::fs::read(&target_path).unwrap(), pattern(FILE_LEN));
    assert_eq!(
        server.requests().last().unwrap().header("range"),
        Some("bytes=1000-")
    );
}

fn resume_request(
    server: &TestServer,
    path: &str,
    dir: &Path,
    len: usize,
) -> mercury_downloads::DownloadRequest {
    let (partial, target) = write_partial(dir, "file.bin", len);
    let mut request = request(server.url(path), dir);
    request.resume = Some(resume_from(partial, target, Some(ETAG), None));
    request
}

#[test]
fn a_range_not_satisfiable_restarts_from_zero_once() {
    for partial_len in [FILE_LEN, FILE_LEN + 100] {
        let server = TestServer::start();
        let dir = TempDir::new("resume-416");
        let partial = dir.path.join("file.bin.part");
        std::fs::write(&partial, vec![b'x'; partial_len]).unwrap();
        let (manager, events) = manager();
        let mut request = request(server.url("/file.bin"), &dir.path);
        request.resume = Some(resume_from(
            partial,
            dir.path.join("file.bin"),
            Some(ETAG),
            None,
        ));
        let id = manager.start(request);
        let done = events_until_done(&events, id);
        assert_eq!(finished_path(terminal(&done)), dir.path.join("file.bin"));
        assert_eq!(
            std::fs::read(dir.path.join("file.bin")).unwrap(),
            pattern(FILE_LEN)
        );
        let requests = server.requests();
        assert_eq!(requests.len(), 2, "{requests:#?}");
        assert_eq!(
            requests[0].header("range"),
            Some(format!("bytes={partial_len}-").as_str())
        );
        assert_eq!(requests[1].header("range"), None);
        assert_eq!(requests[1].header("if-range"), None);
    }
}

#[test]
fn a_partial_response_that_does_not_continue_the_file_fails() {
    let server = TestServer::start();
    let dir = TempDir::new("resume-mismatch");
    let (manager, events) = manager();
    let id = manager.start(resume_request(&server, "/bad-range", &dir.path, 1000));
    let done = events_until_done(&events, id);
    let (error, resumable) = failed(terminal(&done));
    assert!(error.contains("does not continue"), "{error}");
    assert!(!resumable);
    assert!(TempDir::list(&dir.path).is_empty());
}

#[test]
fn an_unrequested_partial_response_fails_and_leaves_nothing() {
    let server = TestServer::start();
    let dir = TempDir::new("unexpected-206");
    let events = download(server.url("/always-206"), &dir.path);
    assert_well_formed(&events, false);
    let (error, resumable) = failed(terminal(&events));
    assert!(error.contains("without being asked"), "{error}");
    assert!(!resumable);
    assert!(TempDir::list(&dir.path).is_empty());
}

#[test]
fn a_resume_answered_with_not_found_removes_the_partial_file() {
    let server = TestServer::start();
    let dir = TempDir::new("resume-404");
    let (manager, events) = manager();
    let id = manager.start(resume_request(&server, "/missing", &dir.path, 1000));
    let (error, resumable) = failed(terminal(&events_until_done(&events, id)));
    assert!(error.contains("404"), "{error}");
    assert!(!resumable);
    assert!(TempDir::list(&dir.path).is_empty());
}

#[test]
fn a_resume_answered_with_service_unavailable_keeps_the_partial_file() {
    let server = TestServer::start();
    let dir = TempDir::new("resume-503");
    let (manager, events) = manager();
    let id = manager.start(resume_request(&server, "/unavailable", &dir.path, 1000));
    let (error, resumable) = failed(terminal(&events_until_done(&events, id)));
    assert!(error.contains("503"), "{error}");
    assert!(resumable);
    assert_eq!(TempDir::list(&dir.path), ["file.bin.part"]);
    assert_eq!(
        std::fs::read(dir.path.join("file.bin.part")).unwrap(),
        &pattern(FILE_LEN)[..1000]
    );
}

#[test]
fn a_partial_response_shorter_than_the_file_is_a_resumable_failure() {
    // The server honours the range with only 1000 more bytes: the body itself is complete, so
    // only the length check against `Content-Range` notices the file is not.
    let server = TestServer::start();
    let dir = TempDir::new("short-range");
    let (manager, events) = manager();
    let id = manager.start(resume_request(&server, "/short-range", &dir.path, 2000));
    let done = events_until_done(&events, id);
    let (error, resumable) = failed(terminal(&done));
    assert!(error.contains("3000 of 65536"), "{error}");
    assert!(resumable);
    assert_eq!(
        std::fs::read(dir.path.join("file.bin.part")).unwrap(),
        &pattern(FILE_LEN)[..3000]
    );
}

#[cfg(unix)]
fn make_fifo(path: &Path) {
    let status = std::process::Command::new("mkfifo")
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success());
}

#[cfg(unix)]
#[test]
fn a_fifo_at_the_partial_path_fails_fast() {
    let server = TestServer::start();
    let dir = TempDir::new("resume-fifo");
    let partial = dir.path.join("file.bin.part");
    make_fifo(&partial);
    let (manager, events) = manager();
    let mut request = request(server.url("/file.bin"), &dir.path);
    request.resume = Some(resume_from(
        partial.clone(),
        dir.path.join("file.bin"),
        Some(ETAG),
        None,
    ));
    let began = Instant::now();
    let id = manager.start(request);
    let done = events_until_done(&events, id);
    failed(terminal(&done));
    assert!(began.elapsed() < Duration::from_secs(2));
    assert!(partial.symlink_metadata().unwrap().file_type().is_fifo());
}

#[test]
fn a_resume_outside_the_destination_folder_is_refused() {
    let server = TestServer::start();
    let dir = TempDir::new("resume-outside");
    let elsewhere = TempDir::new("resume-elsewhere");
    let (partial, target) = write_partial(&elsewhere.path, "file.bin", 1000);
    let (manager, events) = manager();

    let mut outside = request(server.url("/file.bin"), &dir.path);
    outside.resume = Some(resume_from(partial.clone(), target, Some(ETAG), None));
    let id = manager.start(outside);
    let events_out = events_until_done(&events, id);
    let (_, resumable) = failed(terminal(&events_out));
    assert!(!resumable);

    // A partial name that does not belong to the target is refused as well.
    let mut mismatched = request(server.url("/file.bin"), &dir.path);
    mismatched.resume = Some(resume_from(
        dir.path.join("other.part"),
        dir.path.join("file.bin"),
        Some(ETAG),
        None,
    ));
    let id = manager.start(mismatched);
    failed(terminal(&events_until_done(&events, id)));

    // Neither touched anything or asked the server.
    assert_eq!(std::fs::read(&partial).unwrap(), &pattern(FILE_LEN)[..1000]);
    assert!(TempDir::list(&dir.path).is_empty());
    assert!(server.requests().is_empty());
}

#[cfg(unix)]
#[test]
fn a_resume_never_writes_through_a_symlinked_partial_file() {
    let server = TestServer::start();
    let outside = TempDir::new("resume-victim");
    let victim = outside.path.join("victim.txt");
    std::fs::write(&victim, b"victim").unwrap();
    let dir = TempDir::new("resume-symlink");
    let partial = dir.path.join("file.bin.part");
    std::os::unix::fs::symlink(&victim, &partial).unwrap();

    let (manager, events) = manager();
    let mut request = request(server.url("/file.bin"), &dir.path);
    request.resume = Some(resume_from(
        partial.clone(),
        dir.path.join("file.bin"),
        Some(ETAG),
        None,
    ));
    let id = manager.start(request);
    let events = events_until_done(&events, id);
    failed(terminal(&events));
    assert_eq!(std::fs::read(&victim).unwrap(), b"victim");
    assert!(partial.symlink_metadata().unwrap().file_type().is_symlink());
}

// --- 9. Cancel -----------------------------------------------------------------------------

#[test]
fn cancel_stops_promptly_removes_the_partial_file_and_never_finishes() {
    let server = TestServer::start();
    let dir = TempDir::new("cancel");
    let (manager, events) = manager();
    let id = manager.start(request(server.url("/slow-long"), &dir.path));
    let started = wait_started(&events, id);
    let (_, _, partial_path, _) = started_fields(&started);
    assert!(partial_path.exists());
    assert_eq!(manager.active(), 1);

    let asked = Instant::now();
    manager.cancel(id);
    let rest = events_until_done(&events, id);
    assert!(
        asked.elapsed() < Duration::from_secs(1),
        "{:?}",
        asked.elapsed()
    );
    assert!(
        matches!(terminal(&rest), DownloadEvent::Cancelled { .. }),
        "{rest:#?}"
    );
    assert!(TempDir::list(&dir.path).is_empty());
    assert_eq!(manager.active(), 0);

    // Idempotent, and nothing follows the terminal event.
    manager.cancel(id);
    manager.cancel(DownloadId(9_999));
    assert!(events.recv_timeout(Duration::from_millis(500)).is_err());
}

#[test]
fn cancel_before_started_emits_only_cancelled_and_leaves_nothing() {
    let server = TestServer::start();
    let parent = TempDir::new("cancel-early");
    let dir = parent.path.join("downloads");
    let (manager, events) = manager();
    let id = manager.start(request(server.url("/slow-headers"), &dir));
    std::thread::sleep(Duration::from_millis(200));
    let asked = Instant::now();
    manager.cancel(id);
    let done = events_until_done(&events, id);
    assert!(asked.elapsed() < Duration::from_secs(1));
    assert_eq!(done.len(), 1, "{done:#?}");
    assert!(matches!(done[0], DownloadEvent::Cancelled { .. }));
    assert!(TempDir::list(&dir).is_empty());
}

#[test]
fn cancel_racing_the_start_never_leaves_a_partial_file() {
    let server = TestServer::start();
    let dir = TempDir::new("cancel-race");
    let (manager, events) = manager();
    for round in 0..60_u64 {
        let id = manager.start(request(server.url("/file.bin"), &dir.path));
        // Land the cancel at different points of the request, the claim and the body.
        let spin = Instant::now();
        while spin.elapsed() < Duration::from_micros(round * 50) {
            std::hint::spin_loop();
        }
        manager.cancel(id);
        let done = events_until_done(&events, id);
        let terminals = done.iter().filter(|event| is_terminal(event)).count();
        assert_eq!(terminals, 1);
    }
    let leftovers: Vec<String> = TempDir::list(&dir.path)
        .into_iter()
        .filter(|name| name.ends_with(".part"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn cancelling_a_finished_download_does_nothing() {
    let server = TestServer::start();
    let dir = TempDir::new("cancel-finished");
    let (manager, events) = manager();
    let id = manager.start(request(server.url("/file.bin"), &dir.path));
    let done = events_until_done(&events, id);
    let finished = finished_path(terminal(&done));
    manager.cancel(id);
    assert!(events.recv_timeout(Duration::from_millis(300)).is_err());
    assert_eq!(std::fs::read(finished).unwrap(), pattern(FILE_LEN));
}

#[test]
fn dropping_the_manager_cancels_everything_and_returns() {
    let server = TestServer::start();
    let dir = TempDir::new("drop");
    let (manager, events) = manager();
    let first = manager.start(request(server.url("/slow-long"), &dir.path));
    let second = manager.start(request(server.url("/slow-long"), &dir.path));
    let mut started = Vec::new();
    while started.len() < 2 {
        let event = events
            .recv_timeout(TEST_TIMEOUT)
            .expect("both should start");
        assert!(!is_terminal(&event), "{event:?}");
        if let DownloadEvent::Started { id, .. } = event {
            started.push(id);
        }
    }
    assert_eq!(manager.active(), 2);

    let asked = Instant::now();
    drop(manager);
    assert!(
        asked.elapsed() < Duration::from_secs(3),
        "{:?}",
        asked.elapsed()
    );
    let mut cancelled = Vec::new();
    while let Ok(event) = events.recv_timeout(Duration::from_millis(500)) {
        if let DownloadEvent::Cancelled { id } = event {
            cancelled.push(id);
        }
    }
    cancelled.sort_by_key(|id| id.0);
    assert_eq!(cancelled, [first, second]);
    assert!(TempDir::list(&dir.path).is_empty());
}

// --- 10. Timeouts --------------------------------------------------------------------------

#[test]
fn a_stalled_body_fails_as_resumable_after_the_idle_timeout() {
    let server = TestServer::start();
    let dir = TempDir::new("stall");
    let (manager, events) = manager_with(Timeouts {
        idle: Duration::from_millis(300),
        ..Timeouts::default()
    });
    let began = Instant::now();
    let id = manager.start(request(server.url("/stall"), &dir.path));
    let events = events_until_done(&events, id);
    assert!(began.elapsed() < Duration::from_secs(5));
    let (error, resumable) = failed(terminal(&events));
    assert!(resumable, "{error}");
    assert!(error.contains("no data"), "{error}");
    assert_eq!(TempDir::list(&dir.path), ["stall.part"]);
}

#[test]
fn a_connection_refused_fails_without_leaving_files() {
    // Bind and drop a listener to find a port nothing listens on.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let dir = TempDir::new("refused");
    let url = Url::parse(&format!("http://127.0.0.1:{port}/file.bin")).unwrap();
    let events = download(url, &dir.path);
    let (_, resumable) = failed(terminal(&events));
    assert!(!resumable);
    assert!(TempDir::list(&dir.path).is_empty());
}

#[test]
fn the_default_timeouts_are_thirty_and_sixty_seconds() {
    let timeouts = Timeouts::default();
    assert_eq!(timeouts.connect, Duration::from_secs(30));
    assert_eq!(timeouts.idle, Duration::from_secs(60));
}
