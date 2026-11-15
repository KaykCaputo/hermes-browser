use std::fs;

use mercury_settings::sweep_stale_temporaries;

#[test]
fn only_stale_temporaries_of_this_crates_files_are_removed() {
    let dir = tempfile::tempdir().expect("a profile");
    let own = std::process::id();
    let other = own.wrapping_add(1);
    let stale = [
        format!(".settings.json.{other}-0.tmp"),
        format!(".session.json.{other}-17.tmp"),
        format!(".session.previous.json.{other}-3.tmp"),
        format!(".session.recovery.json.{other}-4.tmp"),
    ];
    let kept = [
        format!(".settings.json.{own}-0.tmp"), // this process's own write in progress
        format!(".bookmarks.json.{other}-0.tmp"), // not one of this crate's files
        format!(".settings.json.{other}-0.tmp.bak"),
        format!("settings.json.{other}-0.tmp"),
        format!(".settings.json.{other}x-0.tmp"),
        ".settings.json.tmp".to_owned(),
        "settings.json".to_owned(),
        "settings.json.damaged-1".to_owned(),
    ];
    for name in stale.iter().chain(&kept) {
        fs::write(dir.path().join(name), b"x").expect("seed");
    }
    let directory = format!(".session.json.{other}-9.tmp");
    fs::create_dir(dir.path().join(&directory)).expect("a directory with the name");

    let removed = sweep_stale_temporaries(dir.path()).expect("sweep");

    assert_eq!(removed, 4);
    for name in &stale {
        assert!(!dir.path().join(name).exists(), "{name} should be gone");
    }
    for name in kept.iter().chain([&directory]) {
        assert!(dir.path().join(name).exists(), "{name} should be kept");
    }
}

#[cfg(unix)]
#[test]
fn a_symlink_with_a_temporary_name_is_left_alone() {
    let dir = tempfile::tempdir().expect("a profile");
    let target = dir.path().join("precious");
    fs::write(&target, b"keep").expect("seed");
    let link = dir.path().join(format!(
        ".settings.json.{}-0.tmp",
        std::process::id().wrapping_add(1)
    ));
    std::os::unix::fs::symlink(&target, &link).expect("symlink");

    assert_eq!(sweep_stale_temporaries(dir.path()).expect("sweep"), 0);
    assert!(fs::symlink_metadata(&link).is_ok());
    assert_eq!(fs::read(&target).expect("target"), b"keep");
}

#[test]
fn a_missing_profile_is_an_error() {
    let dir = tempfile::tempdir().expect("a profile");
    assert!(sweep_stale_temporaries(&dir.path().join("missing")).is_err());
}
