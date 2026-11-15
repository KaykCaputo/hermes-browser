//! Download records. The transfer itself is done elsewhere; this only persists its state.

use rusqlite::params;

use crate::Store;
use crate::error::{Result, StoreError};
use crate::limits::{
    MAX_ERROR_CHARS, MAX_MIME_TYPE_CHARS, MAX_VALIDATOR_BYTES, check_path, check_url, cut,
};

/// Identifies one download record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DownloadRecordId(pub i64);

/// The state of a download. Uses Chromium's numbering (`download_constants.h`).
///
/// Transitions (anything else fails with [`StoreError::InvalidState`] and changes nothing):
///
/// | From | To | By |
/// | --- | --- | --- |
/// | (new) | `InProgress` | [`Store::insert_download`] |
/// | `InProgress` | `InProgress` (bytes) | [`Store::update_download_progress`] |
/// | `InProgress` | `Complete`, `Cancelled`, `Interrupted` | [`Store::finish_download`] |
/// | `InProgress` | `Interrupted` | [`Store::mark_unfinished_downloads_interrupted`] |
/// | `Interrupted` | `InProgress` | [`Store::resume_download`] |
/// | `Interrupted` | `Cancelled` | [`Store::finish_download`] |
///
/// `Complete` and `Cancelled` are final. [`Store::set_download_validators`] is allowed in
/// every state.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DownloadState {
    /// Still transferring.
    InProgress = 0,
    /// Finished successfully.
    Complete = 1,
    /// Cancelled by the user.
    Cancelled = 2,
    /// Stopped by an error or by the browser exiting; may be resumable.
    Interrupted = 4,
}

impl DownloadState {
    fn from_i64(value: i64) -> Option<Self> {
        match value {
            0 => Some(Self::InProgress),
            1 => Some(Self::Complete),
            2 => Some(Self::Cancelled),
            4 => Some(Self::Interrupted),
            _ => None,
        }
    }
}

/// The fields known when a download starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewDownload<'a> {
    /// Globally unique identifier, supplied by the caller.
    pub guid: &'a str,
    /// The requested URL.
    pub url: &'a str,
    /// The page that linked to it.
    pub referrer: Option<&'a str>,
    /// Where the finished file goes.
    pub target_path: &'a str,
    /// Where the partial file is written while transferring.
    pub temp_path: Option<&'a str>,
    /// The MIME type, if known.
    pub mime_type: Option<&'a str>,
    /// The expected size in bytes, if known.
    pub total_bytes: Option<i64>,
    /// When it started, in milliseconds since the Unix epoch.
    pub started_at_ms: i64,
}

/// One row of the `downloads` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadRecord {
    /// The record.
    pub id: DownloadRecordId,
    /// Globally unique identifier.
    pub guid: String,
    /// The requested URL.
    pub url: String,
    /// The URL after redirects, if known.
    pub final_url: Option<String>,
    /// The page that linked to it.
    pub referrer: Option<String>,
    /// Where the finished file goes.
    pub target_path: String,
    /// Where the partial file is written while transferring.
    pub temp_path: Option<String>,
    /// The MIME type, if known.
    pub mime_type: Option<String>,
    /// The expected size in bytes, if known.
    pub total_bytes: Option<i64>,
    /// Bytes received so far.
    pub received_bytes: i64,
    /// The current state.
    pub state: DownloadState,
    /// Why it was interrupted, if it was.
    pub error: Option<String>,
    /// The server's `ETag`, for resuming.
    pub etag: Option<String>,
    /// The server's `Last-Modified`, for resuming.
    pub last_modified: Option<String>,
    /// When it started, in milliseconds since the Unix epoch.
    pub started_at_ms: i64,
    /// When it finished, was cancelled or was interrupted, in milliseconds since the Unix epoch.
    pub ended_at_ms: Option<i64>,
}

impl Store {
    /// Records a new download in progress. `guid` must be unique.
    ///
    /// URLs longer than [`MAX_URL_BYTES`](crate::MAX_URL_BYTES) and paths longer than
    /// [`MAX_PATH_BYTES`](crate::MAX_PATH_BYTES) are refused; the MIME type is cut to
    /// [`MAX_MIME_TYPE_CHARS`].
    pub fn insert_download(&mut self, download: &NewDownload<'_>) -> Result<DownloadRecordId> {
        check_url(download.url)?;
        download.referrer.map(check_url).transpose()?;
        check_path(download.target_path)?;
        download.temp_path.map(check_path).transpose()?;
        let mime_type = download.mime_type.map(|m| cut(m, MAX_MIME_TYPE_CHARS));
        let tx = self.conn.transaction()?;
        let taken: bool = tx
            .prepare_cached("SELECT EXISTS (SELECT 1 FROM downloads WHERE guid = ?1)")?
            .query_row([download.guid], |row| row.get(0))?;
        if taken {
            return Err(StoreError::DuplicateGuid(download.guid.to_owned()));
        }
        tx.prepare_cached(
            "INSERT INTO downloads (guid, url, referrer, target_path, temp_path, mime_type,
                                    total_bytes, received_bytes, state, started_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, ?8, ?9)",
        )?
        .execute(params![
            download.guid,
            download.url,
            download.referrer,
            download.target_path,
            download.temp_path,
            mime_type,
            download.total_bytes,
            DownloadState::InProgress as u8,
            download.started_at_ms,
        ])?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        Ok(DownloadRecordId(id))
    }

    /// Stores the bytes received so far. `total_bytes: None` leaves the stored total unchanged.
    /// Only a download in progress can make progress; otherwise this fails with
    /// [`StoreError::InvalidState`] and changes nothing.
    pub fn update_download_progress(
        &mut self,
        id: DownloadRecordId,
        received_bytes: i64,
        total_bytes: Option<i64>,
    ) -> Result<()> {
        let changed = self
            .conn
            .prepare_cached(
                "UPDATE downloads SET received_bytes = ?2, total_bytes = coalesce(?3, total_bytes)
                 WHERE id = ?1 AND state = 0",
            )?
            .execute(params![id.0, received_bytes, total_bytes])?;
        self.check_transition(id, changed, "the download is not in progress")
    }

    /// Stores the final URL and the validators needed to resume, in any state. `None` clears a
    /// value. A final URL longer than [`MAX_URL_BYTES`](crate::MAX_URL_BYTES) is refused; a
    /// validator longer than [`MAX_VALIDATOR_BYTES`] is stored as `None`.
    pub fn set_download_validators(
        &mut self,
        id: DownloadRecordId,
        final_url: Option<&str>,
        etag: Option<&str>,
        last_modified: Option<&str>,
    ) -> Result<()> {
        final_url.map(check_url).transpose()?;
        let fits = |v: &&str| v.len() <= MAX_VALIDATOR_BYTES;
        let changed = self
            .conn
            .prepare_cached(
                "UPDATE downloads SET final_url = ?2, etag = ?3, last_modified = ?4 WHERE id = ?1",
            )?
            .execute(params![
                id.0,
                final_url,
                etag.filter(fits),
                last_modified.filter(fits)
            ])?;
        if changed == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    /// Marks a download in progress complete, cancelled or interrupted, or cancels an
    /// interrupted one (see the table on [`DownloadState`]). `state` must not be
    /// [`DownloadState::InProgress`]. Any other transition leaves the record unchanged and
    /// fails with [`StoreError::InvalidState`], so a late cancel cannot rewrite a completed
    /// record. `error` is cut to [`MAX_ERROR_CHARS`].
    pub fn finish_download(
        &mut self,
        id: DownloadRecordId,
        state: DownloadState,
        error: Option<&str>,
        ended_at_ms: i64,
    ) -> Result<()> {
        if state == DownloadState::InProgress {
            return Err(StoreError::InvalidArgument(
                "a download cannot be finished as in progress",
            ));
        }
        let changed = self
            .conn
            .prepare_cached(
                "UPDATE downloads SET state = ?2, error = ?3, ended_at = ?4
                 WHERE id = ?1 AND (state = 0 OR (state = 4 AND ?2 = 2))",
            )?
            .execute(params![
                id.0,
                state as u8,
                error.map(|e| cut(e, MAX_ERROR_CHARS)),
                ended_at_ms
            ])?;
        self.check_transition(
            id,
            changed,
            "only a download in progress can finish; an interrupted one can only be cancelled",
        )
    }

    /// Puts an interrupted download back in progress, clearing its end time and error, so that
    /// a resumed transfer can be tracked. Bytes received, validators and start time are kept.
    /// Any other state fails with [`StoreError::InvalidState`] and changes nothing.
    pub fn resume_download(&mut self, id: DownloadRecordId) -> Result<()> {
        let changed = self
            .conn
            .prepare_cached(
                "UPDATE downloads SET state = 0, ended_at = NULL, error = NULL
                 WHERE id = ?1 AND state = 4",
            )?
            .execute([id.0])?;
        self.check_transition(id, changed, "only an interrupted download can be resumed")
    }

    /// Download records, newest first.
    pub fn downloads(&self, limit: u32) -> Result<Vec<DownloadRecord>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, guid, url, final_url, referrer, target_path, temp_path, mime_type,
                    total_bytes, received_bytes, state, error, etag, last_modified, started_at,
                    ended_at
             FROM downloads ORDER BY started_at DESC, id DESC LIMIT ?1",
        )?;
        let records = stmt
            .query_map([limit], |row| {
                let state: i64 = row.get(10)?;
                Ok(DownloadRecord {
                    id: DownloadRecordId(row.get(0)?),
                    guid: row.get(1)?,
                    url: row.get(2)?,
                    final_url: row.get(3)?,
                    referrer: row.get(4)?,
                    target_path: row.get(5)?,
                    temp_path: row.get(6)?,
                    mime_type: row.get(7)?,
                    total_bytes: row.get(8)?,
                    received_bytes: row.get(9)?,
                    state: DownloadState::from_i64(state).ok_or_else(|| {
                        rusqlite::Error::FromSqlConversionFailure(
                            10,
                            rusqlite::types::Type::Integer,
                            format!("unknown download state {state}").into(),
                        )
                    })?,
                    error: row.get(11)?,
                    etag: row.get(12)?,
                    last_modified: row.get(13)?,
                    started_at_ms: row.get(14)?,
                    ended_at_ms: row.get(15)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(records)
    }

    /// Removes one record. Returns `false` if it did not exist.
    pub fn remove_download(&mut self, id: DownloadRecordId) -> Result<bool> {
        let removed = self
            .conn
            .prepare_cached("DELETE FROM downloads WHERE id = ?1")?
            .execute([id.0])?;
        Ok(removed > 0)
    }

    /// Removes every record that is not in progress and returns how many were removed.
    pub fn clear_finished_downloads(&mut self) -> Result<u64> {
        let removed = self
            .conn
            .prepare_cached("DELETE FROM downloads WHERE state != ?1")?
            .execute([DownloadState::InProgress as u8])?;
        Ok(removed as u64)
    }

    /// Called once at start-up: anything still marked in progress was cut off by the last exit.
    /// Marks those records interrupted, ending at `now_ms`, and returns how many there were.
    pub fn mark_unfinished_downloads_interrupted(&mut self, now_ms: i64) -> Result<u64> {
        let changed = self
            .conn
            .prepare_cached("UPDATE downloads SET state = ?1, ended_at = ?2 WHERE state = ?3")?
            .execute(params![
                DownloadState::Interrupted as u8,
                now_ms,
                DownloadState::InProgress as u8
            ])?;
        Ok(changed as u64)
    }

    /// After an update guarded by a state condition that changed `changed` rows: success if it
    /// changed the record, [`StoreError::NotFound`] if the record does not exist, otherwise
    /// [`StoreError::InvalidState`]`(refusal)`.
    fn check_transition(
        &self,
        id: DownloadRecordId,
        changed: usize,
        refusal: &'static str,
    ) -> Result<()> {
        if changed > 0 {
            return Ok(());
        }
        let exists: bool = self
            .conn
            .prepare_cached("SELECT EXISTS (SELECT 1 FROM downloads WHERE id = ?1)")?
            .query_row([id.0], |row| row.get(0))?;
        Err(if exists {
            StoreError::InvalidState(refusal)
        } else {
            StoreError::NotFound
        })
    }
}
