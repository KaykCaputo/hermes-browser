// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy
// of the MPL was not distributed with this file, You can obtain one at
// https://mozilla.org/MPL/2.0/.
//
// Adapted from Goddv/Zervo (https://github.com/Goddv/Zervo) at commit `86cb7126`,
// `zervo-core/src/store.rs` (`write_private`, lines 80-123), MPL-2.0. Changes: the directory is
// no longer created; the temporary name carries a per-process counter as well as the process id
// and is created exclusively (`O_CREAT | O_EXCL`), retrying on a collision, so two writers never
// share one and a file or symlink planted at that name is never followed; the file is closed
// before the rename; and the directory is synced after the rename.

//! Crash-safe replacement of small private files.

use std::ffi::{OsStr, OsString};
use std::fs::{File, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// How many temporary names are tried before giving up. A collision needs a stale file left by
/// an earlier process with the same id and counter, so more than one retry is already unusual.
const MAX_NAME_ATTEMPTS: usize = 64;

static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);

/// Replaces `path` with `bytes` so a reader sees the old or the new content, never a mixture,
/// even across a crash or power loss: a uniquely named temporary file in the same directory,
/// created private and exclusively; written; synced; renamed over `path`; directory synced.
///
/// The directory must exist; it is not created. On Unix the file has mode 0600 from the moment
/// it exists. A symlink at `path` is replaced by the new file, not written through. If any step
/// before the rename fails, the temporary file is removed and `path` is untouched. An error from
/// the final directory sync means the new content is in place but may not yet be durable.
pub fn write_private_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let (_, name) = split(path)?;
    let names = std::iter::repeat_with(|| temporary_name(name));
    write_staged(path, bytes, names, Stop::Never).map(|_| ())
}

/// Where a write stops early. Only tests stop early, to stand in for a crash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stop {
    Never,
    #[cfg(test)]
    AfterCreated,
    #[cfg(test)]
    AfterWritten,
}

/// The write itself, with the temporary names supplied and an optional early stop. Returns the
/// temporary file's path when it stopped early (the "crash" leaves it behind).
fn write_staged(
    path: &Path,
    bytes: &[u8],
    names: impl Iterator<Item = OsString>,
    stop: Stop,
) -> io::Result<Option<PathBuf>> {
    let (dir, _) = split(path)?;
    let (temporary, mut file) = create_temporary(dir, names)?;
    #[cfg(not(test))]
    let Stop::Never = stop;

    let written = (|| -> io::Result<bool> {
        #[cfg(test)]
        if stop == Stop::AfterCreated {
            return Ok(false);
        }
        file.write_all(bytes)?;
        // Flushed before the rename: otherwise the rename can reach the disk before the
        // contents do, and a crash in between leaves an empty file where the old one was.
        file.sync_all()?;
        #[cfg(test)]
        if stop == Stop::AfterWritten {
            return Ok(false);
        }
        drop(file);
        std::fs::rename(&temporary, path)?;
        Ok(true)
    })();

    match written {
        Ok(true) => {
            sync_directory(dir)?;
            Ok(None)
        }
        Ok(false) => Ok(Some(temporary)),
        Err(error) => {
            let _ = std::fs::remove_file(&temporary);
            Err(error)
        }
    }
}

fn split(path: &Path) -> io::Result<(&Path, &OsStr)> {
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} does not name a file", path.display()),
        )
    })?;
    let dir = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    Ok((dir, name))
}

/// `.<name>.<pid>-<n>.tmp`: hidden, beside the target so the rename never crosses a filesystem,
/// and distinct per process (the id) and per write within a process (the counter).
fn temporary_name(name: &OsStr) -> OsString {
    let n = NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed);
    let mut temporary = OsString::from(".");
    temporary.push(name);
    temporary.push(format!(".{}-{n}.tmp", std::process::id()));
    temporary
}

/// The target file name and process id in a name made by [`temporary_name`], or `None` if
/// `name` is not exactly of that form.
pub(crate) fn parse_temporary_name(name: &str) -> Option<(&str, u32)> {
    let digits = |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    let inner = name.strip_prefix('.')?.strip_suffix(".tmp")?;
    let (rest, counter) = inner.rsplit_once('-')?;
    let (target, pid) = rest.rsplit_once('.')?;
    if target.is_empty() || !digits(counter) || !digits(pid) {
        return None;
    }
    counter.parse::<u64>().ok()?;
    Some((target, pid.parse().ok()?))
}

fn create_temporary(
    dir: &Path,
    names: impl Iterator<Item = OsString>,
) -> io::Result<(PathBuf, File)> {
    let mut options = OpenOptions::new();
    // `create_new` is `O_CREAT | O_EXCL`: it fails if anything, a symlink included, already has
    // the name, so a planted file is never truncated and a planted symlink never followed.
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        // Created private rather than created and then tightened: there is no moment in
        // between for anything else to open it.
        options.mode(0o600);
    }

    let mut last_collision = None;
    for name in names.take(MAX_NAME_ATTEMPTS) {
        let candidate = dir.join(name);
        match options.open(&candidate) {
            Ok(file) => return Ok((candidate, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                last_collision = Some(error);
            }
            Err(error) => return Err(error),
        }
    }
    Err(last_collision.unwrap_or_else(|| {
        io::Error::new(io::ErrorKind::AlreadyExists, "no free temporary file name")
    }))
}

/// Makes the rename itself durable: until the directory is synced, a power loss can bring back
/// the old directory entry.
#[cfg(unix)]
fn sync_directory(dir: &Path) -> io::Result<()> {
    match File::open(dir).and_then(|dir| dir.sync_all()) {
        Ok(()) => Ok(()),
        // Some filesystems cannot sync a directory; the rename is then as durable as it gets.
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::InvalidInput | io::ErrorKind::Unsupported
            ) =>
        {
            Ok(())
        }
        Err(error) => Err(error),
    }
}

#[cfg(not(unix))]
fn sync_directory(_dir: &Path) -> io::Result<()> {
    Ok(())
}

/// No test here can tell whether the file and the directory were synced: a sync only matters
/// across a power loss, which a test cannot cause. The syncs are checked by reading the code.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temporary_names_parse_back_and_nothing_else_does() {
        let name = temporary_name(OsStr::new("session.json"));
        let name = name.to_str().unwrap();
        assert_eq!(
            parse_temporary_name(name),
            Some(("session.json", std::process::id()))
        );
        for other in [
            "session.json",
            ".session.json.tmp",
            ".session.json.12-3.tmp.bak",
            "session.json.12-3.tmp",
            ".session.json.12a-3.tmp",
            ".session.json.12-.tmp",
            ".session.json.-3.tmp",
            "..12-3.tmp",
            ".session.json.99999999999-3.tmp",
        ] {
            assert_eq!(parse_temporary_name(other), None, "{other}");
        }
    }

    fn listing(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn temporary_names_are_unique_hidden_and_beside_the_target() {
        let first = temporary_name(OsStr::new("settings.json"));
        let second = temporary_name(OsStr::new("settings.json"));
        assert_ne!(first, second);
        for name in [&first, &second] {
            let name = name.to_string_lossy();
            assert!(name.starts_with(".settings.json."), "{name}");
            assert!(name.ends_with(".tmp"), "{name}");
            assert!(!name.contains('/'), "{name}");
        }
    }

    #[test]
    fn a_crash_between_write_and_rename_leaves_the_original_intact() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, b"original").unwrap();

        let names = std::iter::once(OsString::from(".settings.json.crash.tmp"));
        let left = write_staged(&path, b"replacement", names, Stop::AfterWritten)
            .unwrap()
            .unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        assert_eq!(std::fs::read(&left).unwrap(), b"replacement");
    }

    #[cfg(unix)]
    #[test]
    fn the_temporary_file_is_private_from_the_moment_it_exists() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");

        let names = std::iter::once(OsString::from(".settings.json.new.tmp"));
        let created = write_staged(&path, b"secret", names, Stop::AfterCreated)
            .unwrap()
            .unwrap();

        let metadata = std::fs::metadata(&created).unwrap();
        assert_eq!(metadata.len(), 0, "nothing has been written yet");
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn a_file_already_at_the_temporary_name_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let squatter = dir.path().join(".taken.tmp");
        std::fs::write(&squatter, b"someone else's").unwrap();

        let names = [".taken.tmp", ".free.tmp"].into_iter().map(OsString::from);
        write_staged(&path, b"new", names, Stop::Never).unwrap();

        assert_eq!(std::fs::read(&squatter).unwrap(), b"someone else's");
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert_eq!(listing(dir.path()), vec![".taken.tmp", "settings.json"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_at_the_temporary_name_is_never_followed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let victim = dir.path().join("victim");
        std::fs::write(&victim, b"precious").unwrap();
        std::os::unix::fs::symlink(&victim, dir.path().join(".trap.tmp")).unwrap();

        let names = [".trap.tmp", ".free.tmp"].into_iter().map(OsString::from);
        write_staged(&path, b"new", names, Stop::Never).unwrap();

        assert_eq!(std::fs::read(&victim).unwrap(), b"precious");
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
    }

    #[test]
    fn when_every_temporary_name_is_taken_nothing_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, b"original").unwrap();
        std::fs::write(dir.path().join(".taken.tmp"), b"x").unwrap();

        let names = std::iter::repeat(OsString::from(".taken.tmp"));
        let error = write_staged(&path, b"new", names, Stop::Never).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        assert_eq!(listing(dir.path()), vec![".taken.tmp", "settings.json"]);
    }
}
