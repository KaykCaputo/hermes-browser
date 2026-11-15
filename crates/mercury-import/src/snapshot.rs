// Adapted from Zephium, https://github.com/zephium-browser/Zephium, commit
// 9026a1fffad74d6e201ef97fe8c02c1b01403cd2, crates/zephium-import/src/snapshot.rs (lines 1-45),
// licensed under MPL-2.0. Ported to rusqlite 0.38 and Mercury's errors; `require` and
// `rows` are new.

//! A running browser keeps its databases open, often under an exclusive lock, and may be
//! mid-write. Reading a private copy (with its write-ahead log or rollback journal) never
//! disturbs it and never sees a torn page: SQLite recovers the copy as it would after a crash.

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
/// directory and opens the copy for queries only.
///
/// The files are copied one after the other, so a browser writing meanwhile could leave a copy
/// that mixes two states. Each file's size and modification time are therefore compared before
/// and after copying; a copy taken while anything changed is taken again, up to `ATTEMPTS`
/// times, and then the import reports [`ImportError::Locked`]. The copy must also pass SQLite's
/// `quick_check`, or the import reports [`ImportError::Database`].
pub(crate) fn open(path: &Path) -> Result<Snapshot, ImportError> {
    open_in(path, &std::env::temp_dir(), stamps)
}

/// What is known of the source files before and after a copy; any difference means the copy
/// may mix two states.
type Stamps = Vec<Option<(u64, Option<SystemTime>)>>;

fn stamps(path: &Path) -> Stamps {
    std::iter::once(path.to_owned())
        .chain(COMPANIONS.iter().map(|suffix| with_suffix(path, suffix)))
        .map(|file| {
            std::fs::metadata(file)
                .ok()
                .map(|metadata| (metadata.len(), metadata.modified().ok()))
        })
        .collect()
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

fn copy_once(path: &Path, copy: &Path) -> Result<(), ImportError> {
    std::fs::copy(path, copy).map_err(|error| ImportError::io(path, error))?;
    for suffix in COMPANIONS {
        let (companion, target) = (with_suffix(path, suffix), with_suffix(copy, suffix));
        match std::fs::copy(&companion, &target) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // A companion an earlier attempt copied may be gone from the source since.
                match std::fs::remove_file(&target) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(ImportError::io(&target, error)),
                }
            }
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
            vec![Some((changing.get(), None))]
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
                seen.push(None);
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
}
