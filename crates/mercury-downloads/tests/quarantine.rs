#![cfg(target_os = "macos")]

mod support;

use mercury_downloads::DownloadEvent;
use support::*;
use url::Url;

fn quarantine_attribute(path: &std::path::Path) -> Option<String> {
    let output = std::process::Command::new("xattr")
        .args(["-p", "com.apple.quarantine"])
        .arg(path)
        .output()
        .expect("run xattr");
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[test]
fn a_finished_download_is_marked_as_quarantined() {
    let server = TestServer::start();
    let dir = TempDir::new("quarantine");
    let (manager, events) = manager();
    let mut request = request(server.url("/file.bin"), &dir.path);
    request.referrer = Some(Url::parse("http://127.0.0.1/page").unwrap());
    let id = manager.start(request);
    let events = events_until_done(&events, id);
    let DownloadEvent::Finished { target_path, .. } = terminal(&events) else {
        panic!("{events:#?}")
    };

    let attribute = quarantine_attribute(target_path)
        .unwrap_or_else(|| panic!("{} has no quarantine attribute", target_path.display()));
    assert!(!attribute.is_empty());
    // `flags;hex time;agent;UUID`. Bit 0x0001 is the one that makes Gatekeeper check the file
    // on first open. (The agent field is filled by macOS from the calling app's bundle, so it is
    // empty for this unbundled test binary.)
    let flags = attribute.split(';').next().unwrap_or("");
    let flags = u32::from_str_radix(flags, 16)
        .unwrap_or_else(|_| panic!("quarantine attribute {attribute:?}"));
    assert_ne!(flags & 0x0001, 0, "quarantine attribute {attribute:?}");
}

#[test]
fn a_cut_partial_file_is_not_marked() {
    let server = TestServer::start();
    let dir = TempDir::new("quarantine-partial");
    let (manager, events) = manager();
    let id = manager.start(request(server.url("/cut"), &dir.path));
    let events = events_until_done(&events, id);
    let DownloadEvent::Started { partial_path, .. } = &events[0] else {
        panic!("{events:#?}")
    };
    assert!(matches!(terminal(&events), DownloadEvent::Failed { .. }));
    assert_eq!(quarantine_attribute(partial_path), None);
}
