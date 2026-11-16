//! The Mercury Browser profile database: browsing history, bookmarks and download records in one
//! SQLite file (`places.sqlite`).
//!
//! The store owns a single connection and has a synchronous API; methods that write take
//! `&mut self`. It never reads the clock: every method whose effect depends on the current time
//! takes `now_ms`, milliseconds since the Unix epoch. GUIDs are supplied by the caller.

#![warn(missing_docs)]
#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)
)]

mod bookmarks;
mod downloads;
mod error;
mod frecency;
mod history;
mod import;
mod limits;
mod schema;
mod search;
mod suggest;

use std::path::Path;

use rusqlite::Connection;

pub use bookmarks::{BookmarkId, BookmarkKind, BookmarkNode, BookmarkRoot};
pub use downloads::{DownloadRecord, DownloadRecordId, DownloadState, NewDownload};
pub use error::{ImportRefusal, Result, StoreError};
pub use history::{ClearRange, HistoryCursor, HistoryEntry, VisitId, VisitTransition};
pub use import::{BOOKMARK_IMPORT_MAX_DEPTH, BookmarkImport, VisitImport};
pub use limits::{
    MAX_ERROR_CHARS, MAX_MIME_TYPE_CHARS, MAX_PATH_BYTES, MAX_TITLE_CHARS, MAX_URL_BYTES,
    MAX_VALIDATOR_BYTES,
};
pub use search::{MAX_SEARCH_INPUT_BYTES, MAX_SEARCH_WORDS};
pub use suggest::{
    BEST_PAGES_EXAMINED, CANDIDATES_PER_RESULT, FRECENCY_REFRESH_AGE_MS, INDEX_ROWS_EXAMINED,
    MAX_SUGGESTIONS, Suggestion,
};

/// The profile database. Owns one SQLite connection.
#[derive(Debug)]
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Opens (creating if needed) the database file at `path` and brings its schema up to date.
    ///
    /// A file written by a newer build is refused with
    /// [`StoreError::UnsupportedSchemaVersion`] and left unmodified.
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    /// Opens a private, empty database that lives only in memory.
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    /// The schema version stored in the database (`PRAGMA user_version`).
    pub fn schema_version(&self) -> Result<u32> {
        schema::user_version(&self.conn)
    }

    /// Copies what it can of the write-ahead log into the database file without waiting for
    /// readers or writers (`PRAGMA wal_checkpoint(PASSIVE)`), so the log does not keep growing
    /// between SQLite's automatic checkpoints. Returns the frames in the log and how many of
    /// them are now in the database file (both -1 when the database is not in WAL mode).
    pub fn checkpoint(&self) -> Result<(i64, i64)> {
        Ok(self
            .conn
            .query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |row| {
                Ok((row.get(1)?, row.get(2)?))
            })?)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        conn.busy_timeout(BUSY_TIMEOUT)?;
        conn.set_prepared_statement_cache_capacity(STATEMENT_CACHE_CAPACITY);
        schema::prepare(&mut conn)?;
        Ok(Self { conn })
    }
}

/// How long a statement waits for another connection's lock before failing with "busy".
const BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Prepared statements kept for reuse: more than the crate's `prepare_cached` call sites, each
/// of which prepares one fixed statement (both checked by a test).
const STATEMENT_CACHE_CAPACITY: usize = 128;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_timeout_is_five_seconds() {
        let store = Store::open_in_memory().unwrap();
        let ms: i64 = store
            .conn
            .pragma_query_value(None, "busy_timeout", |row| row.get(0))
            .unwrap();
        assert_eq!(ms, 5000);
    }

    #[test]
    fn writes_sync_at_checkpoints_only() {
        // `synchronous = NORMAL` (1) with WAL: a commit is durable once the WAL is synced at a
        // checkpoint; a power loss can lose the last commits but never corrupts the file.
        let store = Store::open_in_memory().unwrap();
        let synchronous: i64 = store
            .conn
            .pragma_query_value(None, "synchronous", |row| row.get(0))
            .unwrap();
        assert_eq!(synchronous, 1);
    }

    #[test]
    fn the_statement_cache_holds_every_cached_statement() {
        let sources = [
            include_str!("bookmarks.rs"),
            include_str!("downloads.rs"),
            include_str!("frecency.rs"),
            include_str!("history.rs"),
            include_str!("import.rs"),
            include_str!("schema.rs"),
            include_str!("search.rs"),
            include_str!("suggest.rs"),
        ];
        let call = concat!(".prepare_", "cached(");
        let mut call_sites = 0;
        for source in sources {
            for (at, _) in source.match_indices(call) {
                call_sites += 1;
                // Each call site prepares one fixed statement (a literal, a constant or a
                // `concat!` of literals), so call sites bound the distinct statements.
                let argument = source[at + call.len()..].trim_start();
                assert!(
                    argument.starts_with('"')
                        || argument.starts_with("concat!")
                        || argument.starts_with(|c: char| c.is_ascii_uppercase()),
                    "a call site prepares a statement that is not fixed: {}",
                    &argument[..argument.len().min(40)]
                );
            }
        }
        assert!(call_sites > 0);
        assert!(
            call_sites < STATEMENT_CACHE_CAPACITY,
            "{call_sites} call sites"
        );
    }
}
