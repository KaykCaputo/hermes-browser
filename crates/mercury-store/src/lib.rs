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
mod schema;
mod search;

use std::path::Path;

use rusqlite::Connection;

pub use bookmarks::{BookmarkId, BookmarkKind, BookmarkNode, BookmarkRoot};
pub use downloads::{DownloadRecord, DownloadRecordId, DownloadState, NewDownload};
pub use error::{Result, StoreError};
pub use history::{ClearRange, HistoryCursor, HistoryEntry, Suggestion, VisitId, VisitTransition};

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

    fn init(mut conn: Connection) -> Result<Self> {
        schema::prepare(&mut conn)?;
        Ok(Self { conn })
    }
}
