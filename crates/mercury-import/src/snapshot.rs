// Adapted from Zephium, https://github.com/zephium-browser/Zephium, commit
// 9026a1fffad74d6e201ef97fe8c02c1b01403cd2, crates/zephium-import/src/snapshot.rs (lines 1-45),
// licensed under MPL-2.0: copying the database with its `-wal` and `-journal` to a temporary
// directory and opening the copy with `query_only`. Ported to rusqlite 0.38 and Mercury's
// errors. New in Mercury: the private directory and file modes, the retry until a copy is taken
// while nothing changed (`stamps`, `wal_header`, `copy_settled`, `copy_once`), the
// `quick_check`, `require` and `rows`.

//! A running browser keeps its databases open, often under an exclusive lock, and may be
//! mid-write. Reading a private copy (with its write-ahead log or rollback journal) never
//! disturbs it: SQLite recovers the copy as it would after a crash.
//!
//! The files are copied one after the other, and a copy taken while the browser writes could
//! mix two states. Before and after copying, the size and modification time of every file and
//! the header of the write-ahead log are compared; a copy taken while any of them changed is
//! taken again, and a database that never holds still for long enough is reported as
//! [`ImportError::Locked`]. The copy must then pass SQLite's `quick_check`.
//!
//! **What this cannot see.** In write-ahead-log mode the log header catches the harmful case, a
//! reset of the log during the copy, whatever the file times. In rollback-journal mode (Chromium's
//! `History`) the modification time is the only signal. `quick_check` finds structural damage
//! but not a structurally valid mixture of two states, so where modification times are coarse a
//! database written during the copy can still be imported silently incomplete. That is the case
//! on FAT and exFAT (two-second times), HFS+ (one second), Linux kernels before 6.13 on ext4,
//! xfs and btrfs (times taken from a coarse clock, a few milliseconds), and network home
//! directories (NFS, SMB), whose times are whatever the server reports. APFS and current Linux
//! kernels keep fine-grained times, and on APFS each file is cloned atomically.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use rusqlite::{Connection, OpenFlags, Row};
use tempfile::TempDir;

use crate::model::ImportError;

pub(crate) struct Snapshot {
    conn: Connection,
    /// The original file, named in errors.
    path: PathBuf,
    // Dropped after the connection, removing the copy.
    _dir: TempDir,
}

/// The companion files SQLite keeps beside a database: the write-ahead log and the rollback
/// journal. The `-shm` index is not copied: it only caches where frames sit in the log, SQLite
/// rebuilds it from the copied log when the copy is opened, and a copied one could describe a
/// different log than the one copied (in exclusive locking mode there is none at all).
const COMPANIONS: [&str; 2] = ["-wal", "-journal"];
/// Copies taken before giving up on a database that keeps changing.
const ATTEMPTS: u32 = 5;
/// The wait between copies: long enough for a browser's burst of writes to end.
const PAUSE: Duration = Duration::from_millis(50);

/// Copies the database at `path` (and its `-wal` or `-journal`, if any) to a private temporary
/// directory and opens the copy for queries only. A copy taken while anything changed is taken
/// again, up to `ATTEMPTS` times, and then the import reports [`ImportError::Locked`]; a copy
/// that fails `quick_check` is reported as [`ImportError::Database`]. See the module
/// documentation for what this cannot detect.
pub(crate) fn open(path: &Path) -> Result<Snapshot, ImportError> {
    open_in(path, &std::env::temp_dir(), stamps)
}

/// What is known of the source files before and after a copy; any difference means the copy
/// may mix two states.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Stamps {
    /// Size and modification time of the database, its `-wal` and its `-journal`.
    files: Vec<Option<(u64, Option<SystemTime>)>>,
    /// The checkpoint sequence and salts of the `-wal` header.
    wal_header: Option<[u8; 12]>,
}

fn stamps(path: &Path) -> Stamps {
    Stamps {
        files: std::iter::once(path.to_owned())
            .chain(COMPANIONS.iter().map(|suffix| with_suffix(path, suffix)))
            .map(|file| {
                std::fs::metadata(file)
                    .ok()
                    .map(|metadata| (metadata.len(), metadata.modified().ok()))
            })
            .collect(),
        wal_header: wal_header(&with_suffix(path, "-wal")),
    }
}

/// Bytes 12-23 of a write-ahead log: the checkpoint sequence number, salt-1 and salt-2 of its
/// 32-byte header. SQLite rewrites them whenever it resets the log after a checkpoint; frames
/// written before the reset no longer count. `None` when there is no log or it is shorter than a
/// header (a log truncated to nothing).
///
/// Read from the source before the main file is copied and again after the log is copied, this
/// notices a reset in between even where file times are too coarse to show one. A reset
/// matters: the copied main file could predate the checkpoint while the copied log no longer
/// holds the frames it moved there. Checking the copied log alone is not enough, because a
/// chunked copy can take the old header with frames written after the reset, which SQLite then
/// ignores.
fn wal_header(wal: &Path) -> Option<[u8; 12]> {
    use std::io::Read;
    let mut header = [0; 32];
    std::fs::File::open(wal)
        .ok()?
        .read_exact(&mut header)
        .ok()?;
    header[12..24].try_into().ok()
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

fn open_in(
    path: &Path,
    temp_root: &Path,
    stamps: impl Fn(&Path) -> Stamps,
) -> Result<Snapshot, ImportError> {
    if !path.is_file() {
        return Err(ImportError::NotFound(path.to_owned()));
    }
    // Every early return below drops `dir`, which removes it with whatever was copied into it.
    let dir = private_dir(temp_root)?;
    let copy = dir.path().join("snapshot.sqlite");
    copy_settled(path, &copy, stamps)?;
    // Opened read-write only so SQLite can replay the copied log into the copy; `query_only`
    // then refuses every write. The browser's own files are never opened.
    let conn = Connection::open_with_flags(
        &copy,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| ImportError::database(path, error))?;
    conn.pragma_update(None, "query_only", true)
        .map_err(|error| ImportError::database(path, error))?;
    let check: String = conn
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(|error| ImportError::database(path, error))?;
    if check != "ok" {
        return Err(ImportError::database(
            path,
            rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CORRUPT),
                Some(format!("quick_check: {check}")),
            ),
        ));
    }
    Ok(Snapshot {
        conn,
        path: path.to_owned(),
        _dir: dir,
    })
}

/// A temporary directory only this user can enter (on Unix; elsewhere the platform's per-user
/// temporary directory is already private).
fn private_dir(temp_root: &Path) -> Result<TempDir, ImportError> {
    let mut builder = tempfile::Builder::new();
    builder.prefix("mercury-import-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    builder
        .tempdir_in(temp_root)
        .map_err(|error| ImportError::io(temp_root, error))
}

/// Copies the database and its companions into `copy` until a copy is taken while nothing
/// changed, then makes the copied files readable by this user only.
fn copy_settled(
    path: &Path,
    copy: &Path,
    stamps: impl Fn(&Path) -> Stamps,
) -> Result<(), ImportError> {
    for attempt in 0..ATTEMPTS {
        if attempt > 0 {
            std::thread::sleep(PAUSE);
        }
        let before = stamps(path);
        copy_once(path, copy)?;
        if stamps(path) == before {
            return make_private(copy);
        }
    }
    log::debug!(
        "{} kept changing while it was copied {ATTEMPTS} times",
        path.display()
    );
    Err(ImportError::Locked(path.to_owned()))
}

/// Copies the database, then each companion that exists. What an earlier attempt copied is
/// removed first: on macOS `std::fs::copy` clones a file (atomically, on APFS) only when the
/// target does not exist yet, and otherwise falls back to copying it in chunks.
fn copy_once(path: &Path, copy: &Path) -> Result<(), ImportError> {
    let targets = std::iter::once(copy.to_owned())
        .chain(COMPANIONS.iter().map(|suffix| with_suffix(copy, suffix)));
    for target in targets {
        match std::fs::remove_file(&target) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(ImportError::io(&target, error)),
        }
    }
    std::fs::copy(path, copy).map_err(|error| ImportError::io(path, error))?;
    for suffix in COMPANIONS {
        let (companion, target) = (with_suffix(path, suffix), with_suffix(copy, suffix));
        match std::fs::copy(&companion, &target) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(ImportError::io(&companion, error)),
        }
    }
    Ok(())
}

/// `std::fs::copy` keeps the source's mode (often 0644); the copies are this user's alone. The
/// files SQLite creates beside the copy take the copy's mode.
fn make_private(copy: &Path) -> Result<(), ImportError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let files = std::iter::once(copy.to_owned())
            .chain(COMPANIONS.iter().map(|suffix| with_suffix(copy, suffix)));
        for file in files {
            match std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(ImportError::io(&file, error)),
            }
        }
    }
    #[cfg(not(unix))]
    let _ = copy;
    Ok(())
}

impl Snapshot {
    pub(crate) fn error(&self, error: rusqlite::Error) -> ImportError {
        ImportError::database(&self.path, error)
    }

    /// Fails with [`ImportError::Malformed`] unless `table` has all of `columns`. A damaged file
    /// fails here too, as [`ImportError::Database`].
    pub(crate) fn require(&self, table: &str, columns: &[&str]) -> Result<(), ImportError> {
        let mut statement = self
            .conn
            .prepare("SELECT name FROM pragma_table_info(?1)")
            .map_err(|error| self.error(error))?;
        let present = statement
            .query_map([table], |row| row.get::<_, String>(0))
            .and_then(|rows| rows.collect::<rusqlite::Result<Vec<_>>>())
            .map_err(|error| self.error(error))?;
        let missing: Vec<&str> = columns
            .iter()
            .copied()
            .filter(|column| !present.iter().any(|name| name.eq_ignore_ascii_case(column)))
            .collect();
        if present.is_empty() {
            return Err(ImportError::Malformed(format!(
                "{} has no table {table}",
                self.path.display()
            )));
        }
        if !missing.is_empty() {
            return Err(ImportError::Malformed(format!(
                "{} table {table} lacks {}",
                self.path.display(),
                missing.join(", ")
            )));
        }
        Ok(())
    }

    /// Runs `sql` and hands each row to `each` until it returns `false`.
    pub(crate) fn rows(
        &self,
        sql: &str,
        mut each: impl FnMut(&Row<'_>) -> rusqlite::Result<bool>,
    ) -> Result<(), ImportError> {
        let mut run = || -> rusqlite::Result<()> {
            let mut statement = self.conn.prepare(sql)?;
            let mut rows = statement.query([])?;
            while let Some(row) = rows.next()? {
                if !each(row)? {
                    break;
                }
            }
            Ok(())
        };
        run().map_err(|error| self.error(error))
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    /// A database with rows still in its write-ahead log, held open by the returned connection.
    fn database_with_log(dir: &Path) -> (PathBuf, Connection) {
        let path = dir.join("places.sqlite");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "PRAGMA journal_mode = WAL; PRAGMA wal_autocheckpoint = 0;
             CREATE TABLE t(x TEXT);
             INSERT INTO t VALUES ('one'), ('two');",
        )
        .unwrap();
        (path, conn)
    }

    fn leftovers(temp_root: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(temp_root)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect()
    }

    fn count(snapshot: &Snapshot) -> i64 {
        snapshot
            .conn
            .query_row("SELECT count(*) FROM t", [], |row| row.get(0))
            .unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn the_copy_is_private_to_this_user() {
        use std::os::unix::fs::PermissionsExt;

        let source = tempfile::tempdir().unwrap();
        let temp_root = tempfile::tempdir().unwrap();
        let (path, _browser) = database_with_log(source.path());
        for file in ["places.sqlite", "places.sqlite-wal"] {
            std::fs::set_permissions(
                source.path().join(file),
                std::fs::Permissions::from_mode(0o644),
            )
            .unwrap();
        }

        let snapshot = open_in(&path, temp_root.path(), stamps).unwrap();
        assert_eq!(count(&snapshot), 2, "the rows in the log are read");

        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        let dirs = leftovers(temp_root.path());
        assert_eq!(dirs.len(), 1);
        assert_eq!(mode(&dirs[0]), 0o700, "the directory is the user's alone");
        let files = leftovers(&dirs[0]);
        assert!(files.len() >= 2, "the copy and its log: {files:?}");
        for file in files {
            assert_eq!(mode(&file), 0o600, "{} is the user's alone", file.display());
        }
    }

    #[test]
    fn nothing_is_left_behind_on_any_path() {
        let source = tempfile::tempdir().unwrap();
        let temp_root = tempfile::tempdir().unwrap();
        let (path, _browser) = database_with_log(source.path());

        let snapshot = open_in(&path, temp_root.path(), stamps).unwrap();
        assert_eq!(count(&snapshot), 2);
        drop(snapshot);
        assert_eq!(
            leftovers(temp_root.path()),
            Vec::<PathBuf>::new(),
            "after a read"
        );

        let changing = Cell::new(0_u64);
        let never_settles = |_: &Path| {
            changing.set(changing.get() + 1);
            Stamps {
                files: vec![Some((changing.get(), None))],
                wal_header: None,
            }
        };
        assert!(matches!(
            open_in(&path, temp_root.path(), never_settles),
            Err(ImportError::Locked(_))
        ));
        assert_eq!(
            leftovers(temp_root.path()),
            Vec::<PathBuf>::new(),
            "after Locked"
        );

        let garbage = source.path().join("garbage.sqlite");
        std::fs::write(&garbage, b"not a database at all").unwrap();
        assert!(matches!(
            open_in(&garbage, temp_root.path(), stamps),
            Err(ImportError::Database { .. })
        ));
        assert_eq!(
            leftovers(temp_root.path()),
            Vec::<PathBuf>::new(),
            "after Database"
        );

        assert!(matches!(
            open_in(&source.path().join("missing"), temp_root.path(), stamps),
            Err(ImportError::NotFound(_))
        ));
        assert_eq!(
            leftovers(temp_root.path()),
            Vec::<PathBuf>::new(),
            "after NotFound"
        );
    }

    #[test]
    fn a_copy_taken_while_the_files_changed_is_taken_again() {
        let source = tempfile::tempdir().unwrap();
        let temp_root = tempfile::tempdir().unwrap();
        let (path, _browser) = database_with_log(source.path());
        let calls = Cell::new(0_u64);
        // The first copy sees the files change under it; the second does not.
        let settles_later = |path: &Path| {
            calls.set(calls.get() + 1);
            let mut seen = stamps(path);
            if calls.get() == 2 {
                seen.files.push(None);
            }
            seen
        };

        let snapshot = open_in(&path, temp_root.path(), settles_later).unwrap();

        assert_eq!(count(&snapshot), 2);
        assert_eq!(calls.get(), 4, "two stamps for each of two copies");
    }

    #[test]
    fn a_copy_with_damaged_pages_is_refused() {
        let source = tempfile::tempdir().unwrap();
        let temp_root = tempfile::tempdir().unwrap();
        let path = source.path().join("History");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE t(x TEXT);
             WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 2000)
             INSERT INTO t SELECT printf('row %d with some padding text', i) FROM n;",
        )
        .unwrap();
        let page_size: i64 = conn
            .query_row("PRAGMA page_size", [], |row| row.get(0))
            .unwrap();
        drop(conn);
        let mut bytes = std::fs::read(&path).unwrap();
        let page = usize::try_from(page_size).unwrap();
        // Damage the third page, a b-tree page of the table, past its header.
        for byte in &mut bytes[2 * page + 8..3 * page] {
            *byte = 0xA5;
        }
        std::fs::write(&path, bytes).unwrap();

        assert!(matches!(
            open_in(&path, temp_root.path(), stamps),
            Err(ImportError::Database { .. })
        ));
    }
    #[test]
    fn the_log_header_gives_the_checkpoint_sequence_and_salts() {
        let source = tempfile::tempdir().unwrap();
        let (path, browser) = database_with_log(source.path());
        let wal = with_suffix(&path, "-wal");
        let word = |header: [u8; 12], at: usize| {
            u32::from_be_bytes(header[at..at + 4].try_into().unwrap())
        };

        let first = wal_header(&wal).expect("a log with frames has a header");
        // A checkpoint that does not reset the log leaves its header alone.
        browser
            .execute_batch("PRAGMA wal_checkpoint(PASSIVE);")
            .unwrap();
        assert_eq!(wal_header(&wal), Some(first));
        // Truncated to nothing: no header.
        browser
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .unwrap();
        assert_eq!(std::fs::metadata(&wal).unwrap().len(), 0);
        assert_eq!(wal_header(&wal), None);
        // The next write starts the log again with a new header.
        browser
            .execute_batch("INSERT INTO t VALUES ('three');")
            .unwrap();
        let second = wal_header(&wal).expect("the log has a header again");
        assert_eq!(word(second, 0), word(first, 0) + 1, "checkpoint sequence");
        assert_eq!(word(second, 4), word(first, 4).wrapping_add(1), "salt-1");
        assert_ne!(second, first);

        let short = source.path().join("short-wal");
        std::fs::write(&short, [0x37, 0x7f, 0x06, 0x82, 0, 0, 0, 0]).unwrap();
        assert_eq!(wal_header(&short), None, "shorter than a header");
        assert_eq!(wal_header(&source.path().join("absent-wal")), None);
    }

    #[test]
    fn a_log_reset_during_the_copy_is_noticed_where_file_times_are_coarse() {
        let source = tempfile::tempdir().unwrap();
        let temp_root = tempfile::tempdir().unwrap();
        let (path, browser) = database_with_log(source.path());
        let calls = Cell::new(0_u64);
        // Sizes and times tell nothing, as on a file system with a coarse clock. Between the
        // first copy's two stamps the browser checkpoints, resets its log and writes again.
        let coarse = |path: &Path| {
            calls.set(calls.get() + 1);
            if calls.get() == 2 {
                browser
                    .execute_batch(
                        "PRAGMA wal_checkpoint(RESTART); INSERT INTO t VALUES ('three');",
                    )
                    .unwrap();
            }
            Stamps {
                files: Vec::new(),
                ..stamps(path)
            }
        };

        let snapshot = open_in(&path, temp_root.path(), coarse).unwrap();

        assert_eq!(
            calls.get(),
            4,
            "the copy taken across the reset is taken again"
        );
        assert_eq!(count(&snapshot), 3);
    }
}
