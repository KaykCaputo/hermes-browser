//! Download record requests. The transfer itself is `mercury-downloads`' work; these only keep
//! its state. See `mercury_store::DownloadState` for the transitions the store allows.
//!
//! Records are addressed by their GUID, which the caller makes with
//! [`new_guid`](super::new_guid) before `insert_download`: every later request can be sent at
//! once, without waiting for the insert's reply (requests run in order). Records of earlier
//! runs are addressed by the `guid` of their `DownloadRecord`. A GUID that is not found is
//! remembered (until it is inserted), so later requests for it fail at once instead of scanning
//! the table again.
//!
//! Inserting, finishing, resuming and removing a record notify [`Area::Downloads`] with its
//! GUID; clearing notifies without one. Progress and validators notify nothing: they do not
//! change the list, and live progress is `mercury-downloads`' in-memory state.

#![cfg_attr(not(test), expect(dead_code, reason = "called by the downloads work"))]

use mercury_store::{DownloadRecord, DownloadRecordId, DownloadState, NewDownload, StoreError};

use super::{Area, Change, Database, ProfileData, ProfileDataError};

/// The fields known when a download starts (an owned `mercury_store::NewDownload`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DownloadStart {
    pub(crate) guid: String,
    pub(crate) url: String,
    pub(crate) referrer: Option<String>,
    pub(crate) target_path: String,
    pub(crate) temp_path: Option<String>,
    pub(crate) mime_type: Option<String>,
    pub(crate) total_bytes: Option<i64>,
    pub(crate) started_at_ms: i64,
}

impl Database {
    /// The record with this GUID: one inserted or listed in this run, or found by listing.
    fn download_id(&mut self, guid: &str) -> mercury_store::Result<DownloadRecordId> {
        if let Some(id) = self.download_ids.get(guid) {
            return Ok(*id);
        }
        if !self.download_misses.contains(guid) {
            self.download_scans += 1;
            for record in self.store.downloads(u32::MAX)? {
                self.download_ids.insert(record.guid, record.id);
            }
            if let Some(id) = self.download_ids.get(guid) {
                return Ok(*id);
            }
            self.download_misses.insert(guid.to_owned());
        }
        log::debug!(
            "no download record {guid} (the table was listed {} times in this run)",
            self.download_scans
        );
        Err(StoreError::NotFound)
    }
}

impl ProfileData {
    /// Records a new download in progress under `start.guid`.
    pub(crate) fn insert_download(
        &self,
        start: DownloadStart,
        reply: impl FnOnce(Result<(), ProfileDataError>) + 'static,
    ) {
        self.request(
            Some(Area::Downloads),
            move |db, _| {
                let id = db.store().insert_download(&NewDownload {
                    guid: &start.guid,
                    url: &start.url,
                    referrer: start.referrer.as_deref(),
                    target_path: &start.target_path,
                    temp_path: start.temp_path.as_deref(),
                    mime_type: start.mime_type.as_deref(),
                    total_bytes: start.total_bytes,
                    started_at_ms: start.started_at_ms,
                })?;
                db.download_misses.remove(&start.guid);
                db.changed(Change::download(&start.guid));
                db.download_ids.insert(start.guid, id);
                Ok(())
            },
            reply,
        );
    }

    /// Stores the bytes received so far (`total_bytes: None` keeps the stored total). No reply:
    /// progress is frequent, and a late report for a finished download is refused and logged.
    pub(crate) fn update_download_progress(
        &self,
        guid: String,
        received_bytes: i64,
        total_bytes: Option<i64>,
    ) {
        self.submit(None, "recording download progress", move |db, _| {
            let id = db.download_id(&guid)?;
            db.store()
                .update_download_progress(id, received_bytes, total_bytes)
        });
    }

    /// Stores the final URL and the validators needed to resume.
    pub(crate) fn set_download_validators(
        &self,
        guid: String,
        final_url: Option<String>,
        etag: Option<String>,
        last_modified: Option<String>,
        reply: impl FnOnce(Result<(), ProfileDataError>) + 'static,
    ) {
        self.request(
            None,
            move |db, _| {
                let id = db.download_id(&guid)?;
                db.store().set_download_validators(
                    id,
                    final_url.as_deref(),
                    etag.as_deref(),
                    last_modified.as_deref(),
                )
            },
            reply,
        );
    }

    /// Marks a download complete, cancelled or interrupted, at the service's current time, then
    /// runs a passive checkpoint so the log does not keep growing during a long download
    /// session.
    pub(crate) fn finish_download(
        &self,
        guid: String,
        state: DownloadState,
        error: Option<String>,
        reply: impl FnOnce(Result<(), ProfileDataError>) + 'static,
    ) {
        self.request(
            Some(Area::Downloads),
            move |db, now| {
                let id = db.download_id(&guid)?;
                db.store()
                    .finish_download(id, state, error.as_deref(), now)?;
                db.changed(Change::download(&guid));
                if let Err(error) = db.store().checkpoint() {
                    log::debug!("checkpoint after a download failed: {error}");
                }
                Ok(())
            },
            reply,
        );
    }

    /// Puts an interrupted download back in progress.
    pub(crate) fn resume_download(
        &self,
        guid: String,
        reply: impl FnOnce(Result<(), ProfileDataError>) + 'static,
    ) {
        self.request(
            Some(Area::Downloads),
            move |db, _| {
                let id = db.download_id(&guid)?;
                db.changed(Change::download(&guid));
                db.store().resume_download(id)
            },
            reply,
        );
    }

    /// Download records, newest first.
    pub(crate) fn downloads(
        &self,
        limit: u32,
        reply: impl FnOnce(Result<Vec<DownloadRecord>, ProfileDataError>) + 'static,
    ) {
        self.request(
            None,
            move |db, _| {
                let records = db.store().downloads(limit)?;
                for record in &records {
                    db.download_ids.insert(record.guid.clone(), record.id);
                }
                Ok(records)
            },
            reply,
        );
    }

    /// Removes one record; `false` if it did not exist.
    pub(crate) fn remove_download(
        &self,
        guid: String,
        reply: impl FnOnce(Result<bool, ProfileDataError>) + 'static,
    ) {
        self.request(
            Some(Area::Downloads),
            move |db, _| {
                let id = match db.download_id(&guid) {
                    Ok(id) => id,
                    Err(StoreError::NotFound) => return Ok(false),
                    Err(error) => return Err(error),
                };
                db.download_ids.remove(&guid);
                db.download_misses.insert(guid.clone());
                db.changed(Change::download(&guid));
                db.store().remove_download(id)
            },
            reply,
        );
    }

    /// Removes every record that is not in progress; replies with how many.
    pub(crate) fn clear_finished_downloads(
        &self,
        reply: impl FnOnce(Result<u64, ProfileDataError>) + 'static,
    ) {
        self.request(
            Some(Area::Downloads),
            move |db, _| {
                let removed = db.store().clear_finished_downloads()?;
                db.download_ids.clear();
                Ok(removed)
            },
            reply,
        );
    }
}
