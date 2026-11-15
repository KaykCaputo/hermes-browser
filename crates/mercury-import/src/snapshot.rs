// Adapted from Zephium, https://github.com/zephium-browser/Zephium, commit
// 9026a1fffad74d6e201ef97fe8c02c1b01403cd2, crates/zephium-import/src/snapshot.rs (lines 1-45),
// licensed under MPL-2.0. Ported to rusqlite 0.38 and Mercury's errors; `require` and
// `rows` are new.

//! A running browser keeps its databases open, often under an exclusive lock, and may be
//! mid-write. Reading a private copy (with its write-ahead log or rollback journal) never
//! disturbs it and never sees a torn page: SQLite recovers the copy as it would after a crash.

use std::path::{Path, PathBuf};

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

/// Copies the database at `path` (and its `-wal` or `-journal`, if any) to a temporary directory
/// and opens the copy for queries only.
pub(crate) fn open(path: &Path) -> Result<Snapshot, ImportError> {
    if !path.is_file() {
        return Err(ImportError::NotFound(path.to_owned()));
    }
    let dir = tempfile::Builder::new()
        .prefix("mercury-import-")
        .tempdir()
        .map_err(|error| ImportError::io(&std::env::temp_dir(), error))?;
    let copy = dir.path().join("snapshot.sqlite");
    std::fs::copy(path, &copy).map_err(|error| ImportError::io(path, error))?;
    for suffix in ["-wal", "-journal"] {
        let mut companion = path.as_os_str().to_owned();
        companion.push(suffix);
        let mut target = copy.as_os_str().to_owned();
        target.push(suffix);
        match std::fs::copy(&companion, &target) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(ImportError::io(Path::new(&companion), error)),
        }
    }
    // Opened read-write only so SQLite can replay the copied log into the copy; `query_only`
    // then refuses every write. The browser's own files are never opened.
    let conn = Connection::open_with_flags(
        &copy,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| ImportError::database(path, error))?;
    conn.pragma_update(None, "query_only", true)
        .map_err(|error| ImportError::database(path, error))?;
    Ok(Snapshot {
        conn,
        path: path.to_owned(),
        _dir: dir,
    })
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
