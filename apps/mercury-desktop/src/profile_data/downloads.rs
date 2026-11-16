//! Download record requests. The transfer itself is `mercury-downloads`' work; these only keep
//! its state. See `mercury_store::DownloadState` for the transitions the store allows.

#![cfg_attr(not(test), allow(dead_code, reason = "called by the downloads work"))]

use mercury_store::{DownloadRecord, DownloadRecordId, DownloadState, NewDownload};

use super::bookmarks::guid_or_error;
use super::{ProfileData, ProfileDataError};

/// The fields known when a download starts (an owned `mercury_store::NewDownload`; the GUID is
/// made by the service).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DownloadStart {
    pub(crate) url: String,
    pub(crate) referrer: Option<String>,
    pub(crate) target_path: String,
    pub(crate) temp_path: Option<String>,
    pub(crate) mime_type: Option<String>,
    pub(crate) total_bytes: Option<i64>,
    pub(crate) started_at_ms: i64,
}

impl ProfileData {
    /// Records a new download in progress.
    pub(crate) fn insert_download(
        &self,
        start: DownloadStart,
        reply: impl FnOnce(Result<DownloadRecordId, ProfileDataError>) + 'static,
    ) {
        self.request(
            move |db, _| {
                let guid = guid_or_error()?;
                db.store().insert_download(&NewDownload {
                    guid: &guid,
                    url: &start.url,
                    referrer: start.referrer.as_deref(),
                    target_path: &start.target_path,
                    temp_path: start.temp_path.as_deref(),
                    mime_type: start.mime_type.as_deref(),
                    total_bytes: start.total_bytes,
                    started_at_ms: start.started_at_ms,
                })
            },
            reply,
        );
    }

    /// Stores the bytes received so far (`total_bytes: None` keeps the stored total). No reply:
    /// progress is frequent, and a late report for a finished download is refused and logged.
    pub(crate) fn update_download_progress(
        &self,
        id: DownloadRecordId,
        received_bytes: i64,
        total_bytes: Option<i64>,
    ) {
        self.submit("recording download progress", move |db, _| {
            db.store()
                .update_download_progress(id, received_bytes, total_bytes)
        });
    }

    /// Stores the final URL and the validators needed to resume.
    pub(crate) fn set_download_validators(
        &self,
        id: DownloadRecordId,
        final_url: Option<String>,
        etag: Option<String>,
        last_modified: Option<String>,
        reply: impl FnOnce(Result<(), ProfileDataError>) + 'static,
    ) {
        self.request(
            move |db, _| {
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

    /// Marks a download complete, cancelled or interrupted, at the service's current time.
    pub(crate) fn finish_download(
        &self,
        id: DownloadRecordId,
        state: DownloadState,
        error: Option<String>,
        reply: impl FnOnce(Result<(), ProfileDataError>) + 'static,
    ) {
        self.request(
            move |db, now| db.store().finish_download(id, state, error.as_deref(), now),
            reply,
        );
    }

    /// Puts an interrupted download back in progress.
    pub(crate) fn resume_download(
        &self,
        id: DownloadRecordId,
        reply: impl FnOnce(Result<(), ProfileDataError>) + 'static,
    ) {
        self.request(move |db, _| db.store().resume_download(id), reply);
    }

    /// Download records, newest first.
    pub(crate) fn downloads(
        &self,
        limit: u32,
        reply: impl FnOnce(Result<Vec<DownloadRecord>, ProfileDataError>) + 'static,
    ) {
        self.request(move |db, _| db.store().downloads(limit), reply);
    }

    /// Removes one record; `false` if it did not exist.
    pub(crate) fn remove_download(
        &self,
        id: DownloadRecordId,
        reply: impl FnOnce(Result<bool, ProfileDataError>) + 'static,
    ) {
        self.request(move |db, _| db.store().remove_download(id), reply);
    }

    /// Removes every record that is not in progress; replies with how many.
    pub(crate) fn clear_finished_downloads(
        &self,
        reply: impl FnOnce(Result<u64, ProfileDataError>) + 'static,
    ) {
        self.request(move |db, _| db.store().clear_finished_downloads(), reply);
    }
}
