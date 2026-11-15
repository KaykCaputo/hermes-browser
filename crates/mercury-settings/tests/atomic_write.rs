use std::fs;
use std::path::Path;

use mercury_settings::write_private_atomic;

fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .expect("the test directory should be readable")
        .map(|entry| {
            entry
                .expect("a directory entry should be readable")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

#[test]
fn a_new_file_is_created_with_the_bytes() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let path = dir.path().join("settings.json");

    write_private_atomic(&path, b"hello").expect("the write should succeed");

    assert_eq!(fs::read(&path).expect("the file should exist"), b"hello");
    assert_eq!(names_in(dir.path()), vec!["settings.json"]);
}

#[test]
fn the_content_is_replaced_whole() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let path = dir.path().join("settings.json");
    fs::write(&path, vec![b'x'; 10_000]).expect("seed the old file");

    write_private_atomic(&path, b"short").expect("the write should succeed");

    assert_eq!(fs::read(&path).expect("the file should exist"), b"short");
    assert_eq!(names_in(dir.path()), vec!["settings.json"]);
}

#[test]
fn an_empty_write_leaves_an_empty_file() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let path = dir.path().join("empty.json");
    fs::write(&path, b"old").expect("seed the old file");

    write_private_atomic(&path, b"").expect("the write should succeed");

    assert_eq!(fs::read(&path).expect("the file should exist"), b"");
}

#[cfg(unix)]
#[test]
fn the_file_is_private_even_when_it_replaces_a_public_one() {
    use std::os::unix::fs::PermissionsExt as _;

    let dir = tempfile::tempdir().expect("a temporary directory");
    let fresh = dir.path().join("fresh.json");
    let replaced = dir.path().join("replaced.json");
    fs::write(&replaced, b"old").expect("seed the old file");
    fs::set_permissions(&replaced, fs::Permissions::from_mode(0o644)).expect("make it public");

    write_private_atomic(&fresh, b"new").expect("the write should succeed");
    write_private_atomic(&replaced, b"new").expect("the write should succeed");

    for path in [&fresh, &replaced] {
        let mode = fs::metadata(path)
            .expect("the file should exist")
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "{} has mode {:o}",
            path.display(),
            mode
        );
    }
}

#[test]
fn a_directory_at_the_target_is_an_error_and_nothing_is_left_behind() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let path = dir.path().join("settings.json");
    fs::create_dir(&path).expect("put a directory in the way");
    fs::write(path.join("inside"), b"keep").expect("put a file inside it");

    let result = write_private_atomic(&path, b"new");

    assert!(result.is_err(), "writing over a directory should fail");
    assert!(path.is_dir(), "the directory should still be there");
    assert_eq!(fs::read(path.join("inside")).expect("still there"), b"keep");
    assert_eq!(names_in(dir.path()), vec!["settings.json"]);
}

#[cfg(unix)]
#[test]
fn an_unwritable_directory_is_an_error_and_the_old_file_survives() {
    use std::os::unix::fs::PermissionsExt as _;

    let dir = tempfile::tempdir().expect("a temporary directory");
    let path = dir.path().join("settings.json");
    fs::write(&path, b"old").expect("seed the old file");
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o555)).expect("lock the dir");
    if fs::write(dir.path().join("probe"), b"").is_ok() {
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).expect("unlock");
        eprintln!("skipped: permissions are not enforced here (running as root?)");
        return;
    }

    let result = write_private_atomic(&path, b"new");
    let names = names_in(dir.path());
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).expect("unlock the dir");

    assert!(
        result.is_err(),
        "writing into a read-only directory should fail"
    );
    assert_eq!(fs::read(&path).expect("the old file"), b"old");
    assert_eq!(names, vec!["settings.json"]);
}

#[test]
fn a_missing_directory_is_an_error_and_is_not_created() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let missing = dir.path().join("missing");

    let result = write_private_atomic(&missing.join("settings.json"), b"new");

    assert!(result.is_err());
    assert!(!missing.exists(), "the directory is the caller's to create");
}

#[test]
fn a_path_without_a_file_name_is_an_error() {
    assert!(write_private_atomic(Path::new("/"), b"new").is_err());
    assert!(write_private_atomic(Path::new(""), b"new").is_err());
}

#[cfg(unix)]
#[test]
fn a_symlink_at_the_target_is_replaced_not_written_through() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let victim = dir.path().join("victim.txt");
    let path = dir.path().join("settings.json");
    fs::write(&victim, b"precious").expect("seed the victim");
    std::os::unix::fs::symlink(&victim, &path).expect("make the symlink");

    write_private_atomic(&path, b"new").expect("the write should succeed");

    assert_eq!(fs::read(&victim).expect("the victim"), b"precious");
    let metadata = fs::symlink_metadata(&path).expect("the target");
    assert!(metadata.file_type().is_file(), "the symlink should be gone");
    assert_eq!(fs::read(&path).expect("the target"), b"new");
}

#[test]
fn concurrent_writers_never_mix_and_leave_nothing_behind() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let path = dir.path().join("session.json");
    let payloads: Vec<Vec<u8>> = (0..8u8)
        .map(|writer| vec![b'a' + writer; 64 * 1024])
        .collect();

    std::thread::scope(|scope| {
        for payload in &payloads {
            let path = &path;
            scope.spawn(move || {
                for _ in 0..20 {
                    write_private_atomic(path, payload).expect("every write should succeed");
                }
            });
        }
    });

    let content = fs::read(&path).expect("the file should exist");
    assert!(
        payloads.contains(&content),
        "the file should hold exactly one writer's bytes"
    );
    assert_eq!(names_in(dir.path()), vec!["session.json"]);
}
