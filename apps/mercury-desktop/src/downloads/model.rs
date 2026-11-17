//! The downloads list: every download of this run and the records of earlier runs, the state
//! each is in, and the database writes each change needs.
//!
//! Pure: no I/O, no clock. Times come in as arguments, file checks as closures, and every
//! change returns the `RecordOp`s that keep the profile database in step, for the caller to
//! send to the profile-data service.

use std::path::{Path, PathBuf};
use std::time::Instant;

use mercury_downloads::{DownloadEvent, DownloadId, ResumeFrom};
use mercury_store::{DownloadRecord, DownloadState};

use super::progress::ProgressWrites;
use crate::profile_data::downloads::DownloadStart;

/// Where a download stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ItemState {
    /// Asked for; the server has not answered yet.
    Starting,
    /// Receiving the file.
    InProgress,
    Complete,
    /// Cancelled by the user; nothing is left on disk.
    Cancelled,
    /// Stopped by an error or by quitting. `resumable` when the partial file was kept and the
    /// server gave what is needed to continue it.
    Interrupted {
        resumable: bool,
    },
}

/// Whether the download has a row in the database, and in which state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Record {
    /// Nothing stored: it has not started, or it failed before the server answered.
    None,
    /// Stored as in progress.
    Open,
    /// Stored as complete, cancelled or interrupted.
    Closed,
}

/// One download in the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Item {
    pub(crate) guid: String,
    pub(crate) url: String,
    pub(crate) referrer: Option<String>,
    /// The name shown: the file's name once known, a guess from the URL before.
    pub(crate) file_name: String,
    pub(crate) target_path: Option<PathBuf>,
    pub(crate) partial_path: Option<PathBuf>,
    pub(crate) mime_type: Option<String>,
    pub(crate) final_url: Option<String>,
    pub(crate) etag: Option<String>,
    pub(crate) last_modified: Option<String>,
    pub(crate) received: u64,
    pub(crate) total: Option<u64>,
    pub(crate) bytes_per_second: u64,
    pub(crate) state: ItemState,
    pub(crate) error: Option<String>,
    pub(crate) started_at_ms: i64,
    /// Started in this run (rather than loaded from an earlier run's record).
    pub(crate) this_run: bool,
    /// For a complete download: whether its file is still there when last checked.
    pub(crate) file_exists: bool,
    /// The running transfer, while there is one.
    pub(crate) transfer: Option<DownloadId>,
    record: Record,
    /// The name the page proposed, kept for Retry.
    pub(crate) suggested_name: Option<String>,
    /// Fetched again by Mercury after the engine could not show it (a page-started download).
    pub(crate) refetched: bool,
    /// Whether Retry makes sense (not for a form's POST or a `blob:` address).
    pub(crate) can_retry: bool,
    /// Device and inode of the finished file, recorded when it was published in this run.
    pub(crate) identity: Option<(u64, u64)>,
}

/// What `lstat` says about a path (links are not followed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FileFacts {
    /// A regular file (not a link, folder or anything else).
    pub(crate) regular: bool,
    pub(crate) len: u64,
    pub(crate) id: (u64, u64),
}

/// The status of a download the engine could not show, whose second request failed: the
/// site answered the first request only (a one-time link, an expired token).
pub(crate) const NOT_SENT_AGAIN: &str =
    "The site did not send the file again; download it from the page";

/// Where a Retry starts from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RetrySource {
    pub(crate) url: String,
    pub(crate) referrer: Option<String>,
    pub(crate) suggested_name: Option<String>,
    pub(crate) refetched: bool,
}

/// Whether the file at an item's target is still the one downloaded: a regular file reached
/// without a link, of the downloaded size, and (when recorded in this run) the same inode.
pub(crate) fn file_still_matches(item: &Item, facts: Option<FileFacts>) -> bool {
    let Some(facts) = facts else {
        return false;
    };
    facts.regular
        && facts.len == item.received
        && item.identity.is_none_or(|identity| identity == facts.id)
}

impl Item {
    pub(crate) fn is_active(&self) -> bool {
        matches!(self.state, ItemState::Starting | ItemState::InProgress)
    }

    /// Where an interrupted download continues from, if it can.
    pub(crate) fn resume_from(&self) -> Option<ResumeFrom> {
        if self.state != (ItemState::Interrupted { resumable: true }) {
            return None;
        }
        self.partial_file()
    }

    /// The partial file and what is known to continue it, whatever the state.
    fn partial_file(&self) -> Option<ResumeFrom> {
        Some(ResumeFrom {
            partial_path: self.partial_path.clone()?,
            target_path: self.target_path.clone()?,
            etag: self.etag.clone(),
            last_modified: self.last_modified.clone(),
        })
    }

    fn by_transfer(&self, id: DownloadId) -> bool {
        self.transfer == Some(id)
    }

    /// Final progress and the end state, if the record is open.
    fn close_record(&mut self, state: DownloadState, error: Option<String>) -> Vec<RecordOp> {
        if self.record != Record::Open {
            return Vec::new();
        }
        self.record = Record::Closed;
        let mut ops = Vec::new();
        if state != DownloadState::Cancelled {
            ops.push(RecordOp::Progress {
                guid: self.guid.clone(),
                received: to_i64(self.received),
                total: self.total.map(to_i64),
            });
        }
        ops.push(RecordOp::Finish {
            guid: self.guid.clone(),
            state,
            error,
        });
        ops
    }
}

/// A write the profile database needs after a change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RecordOp {
    Insert(DownloadStart),
    Progress {
        guid: String,
        received: i64,
        total: Option<i64>,
    },
    Validators {
        guid: String,
        final_url: Option<String>,
        etag: Option<String>,
        last_modified: Option<String>,
    },
    TargetPath {
        guid: String,
        path: String,
    },
    Finish {
        guid: String,
        state: DownloadState,
        error: Option<String>,
    },
    Resume {
        guid: String,
    },
    Remove {
        guid: String,
    },
    Clear,
}

/// What a new download is, before the server answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NewItem {
    pub(crate) guid: String,
    pub(crate) url: String,
    pub(crate) referrer: Option<String>,
    pub(crate) name_guess: String,
    pub(crate) transfer: DownloadId,
    pub(crate) started_at_ms: i64,
    pub(crate) suggested_name: Option<String>,
    pub(crate) refetched: bool,
}

/// The list. Items are kept newest first.
#[derive(Debug, Default)]
pub(crate) struct DownloadList {
    items: Vec<Item>,
    progress_writes: ProgressWrites,
}

impl DownloadList {
    pub(crate) fn items(&self) -> &[Item] {
        &self.items
    }

    pub(crate) fn item(&self, guid: &str) -> Option<&Item> {
        self.items.iter().find(|item| item.guid == guid)
    }

    /// Downloads still running.
    pub(crate) fn active_count(&self) -> usize {
        self.items.iter().filter(|item| item.is_active()).count()
    }

    /// Whether anything was downloaded in this run (the toolbar button shows then).
    pub(crate) fn has_items_this_run(&self) -> bool {
        self.items.iter().any(|item| item.this_run)
    }

    /// The share of the active downloads received, from 0 to 1, over those with a known size;
    /// `None` when nothing is active or no active size is known.
    pub(crate) fn overall_progress(&self) -> Option<f32> {
        let (received, total) = self
            .items
            .iter()
            .filter(|item| item.is_active())
            .filter_map(|item| Some((item.received.min(item.total?), item.total?)))
            .fold((0_u64, 0_u64), |(received, total), (r, t)| {
                (received.saturating_add(r), total.saturating_add(t))
            });
        (total > 0).then(|| (received as f64 / total as f64) as f32)
    }

    /// The running transfers, for cancelling or interrupting them all.
    pub(crate) fn running_transfers(&self) -> Vec<DownloadId> {
        self.items.iter().filter_map(|item| item.transfer).collect()
    }

    /// A download was asked for and its transfer started.
    pub(crate) fn begin(&mut self, new: NewItem) {
        let item = Item {
            guid: new.guid,
            url: new.url,
            referrer: new.referrer,
            file_name: new.name_guess,
            target_path: None,
            partial_path: None,
            mime_type: None,
            final_url: None,
            etag: None,
            last_modified: None,
            received: 0,
            total: None,
            bytes_per_second: 0,
            state: ItemState::Starting,
            error: None,
            started_at_ms: new.started_at_ms,
            this_run: true,
            file_exists: false,
            transfer: Some(new.transfer),
            record: Record::None,
            suggested_name: new.suggested_name,
            refetched: new.refetched,
            can_retry: true,
            identity: None,
        };
        self.insert_sorted(item);
    }

    /// A download that could not even start (a form's POST, a `blob:` address, the engine
    /// not running): shown in this run's list with `message`, never stored, no Retry.
    pub(crate) fn add_note(
        &mut self,
        guid: String,
        url: String,
        name: String,
        message: String,
        started_at_ms: i64,
    ) {
        self.insert_sorted(Item {
            guid,
            url,
            referrer: None,
            file_name: name,
            target_path: None,
            partial_path: None,
            mime_type: None,
            final_url: None,
            etag: None,
            last_modified: None,
            received: 0,
            total: None,
            bytes_per_second: 0,
            state: ItemState::Interrupted { resumable: false },
            error: Some(message),
            started_at_ms,
            this_run: true,
            file_exists: false,
            transfer: None,
            record: Record::None,
            suggested_name: None,
            refetched: false,
            can_retry: false,
            identity: None,
        });
    }

    /// What Retry downloads again, if the item can be retried and is not running.
    pub(crate) fn retry_source(&self, guid: &str) -> Option<RetrySource> {
        let item = self.item(guid)?;
        (item.can_retry && !item.is_active()).then(|| RetrySource {
            url: item.url.clone(),
            referrer: item.referrer.clone(),
            suggested_name: item.suggested_name.clone(),
            refetched: item.refetched,
        })
    }

    /// The finished file's identity, read right after it was published.
    pub(crate) fn record_identity(&mut self, guid: &str, facts: Option<FileFacts>) {
        if let Some(item) = self.items.iter_mut().find(|item| item.guid == guid) {
            item.identity = facts.filter(|facts| facts.regular).map(|facts| facts.id);
        }
    }

    /// Newest first; among equal start times, the one added later first.
    fn insert_sorted(&mut self, item: Item) {
        let position = self
            .items
            .iter()
            .position(|other| other.started_at_ms <= item.started_at_ms)
            .unwrap_or(self.items.len());
        self.items.insert(position, item);
    }

    /// Applies an event of `mercury-downloads`. `now` throttles progress writes.
    pub(crate) fn apply(&mut self, event: DownloadEvent, now: Instant) -> Vec<RecordOp> {
        let id = event_id(&event);
        let Some(item) = self.items.iter_mut().find(|item| item.by_transfer(id)) else {
            return Vec::new();
        };
        match event {
            DownloadEvent::Started {
                file_name,
                target_path,
                partial_path,
                total_bytes,
                mime_type,
                final_url,
                etag,
                last_modified,
                ..
            } => {
                item.state = ItemState::InProgress;
                item.file_name = file_name;
                item.target_path = Some(target_path.clone());
                item.partial_path = Some(partial_path.clone());
                item.total = total_bytes;
                if mime_type.is_some() {
                    item.mime_type = mime_type;
                }
                item.final_url = Some(final_url.to_string());
                item.etag = etag;
                item.last_modified = last_modified;
                let mut ops = Vec::new();
                match item.record {
                    Record::None => ops.push(RecordOp::Insert(DownloadStart {
                        guid: item.guid.clone(),
                        url: item.url.clone(),
                        referrer: item.referrer.clone(),
                        target_path: path_text(&target_path),
                        temp_path: Some(path_text(&partial_path)),
                        mime_type: item.mime_type.clone(),
                        total_bytes: item.total.map(to_i64),
                        started_at_ms: item.started_at_ms,
                    })),
                    // A resumed record: the name stays the one it was resumed under.
                    Record::Open | Record::Closed => {}
                }
                item.record = Record::Open;
                ops.push(RecordOp::Validators {
                    guid: item.guid.clone(),
                    final_url: item.final_url.clone(),
                    etag: item.etag.clone(),
                    last_modified: item.last_modified.clone(),
                });
                ops
            }
            DownloadEvent::Progress {
                received_bytes,
                total_bytes,
                bytes_per_second,
                ..
            } => {
                item.received = received_bytes;
                if total_bytes.is_some() {
                    item.total = total_bytes;
                }
                item.bytes_per_second = bytes_per_second;
                if item.record == Record::Open && self.progress_writes.due(&item.guid, now) {
                    vec![RecordOp::Progress {
                        guid: item.guid.clone(),
                        received: to_i64(item.received),
                        total: item.total.map(to_i64),
                    }]
                } else {
                    Vec::new()
                }
            }
            DownloadEvent::Finished {
                target_path,
                received_bytes,
                ..
            } => {
                let mut ops = Vec::new();
                if item.record == Record::Open && item.target_path.as_ref() != Some(&target_path) {
                    ops.push(RecordOp::TargetPath {
                        guid: item.guid.clone(),
                        path: path_text(&target_path),
                    });
                }
                if let Some(name) = target_path.file_name() {
                    item.file_name = name.to_string_lossy().into_owned();
                }
                item.target_path = Some(target_path);
                item.received = received_bytes;
                item.total = Some(received_bytes);
                item.bytes_per_second = 0;
                item.state = ItemState::Complete;
                item.file_exists = true;
                item.transfer = None;
                ops.extend(item.close_record(DownloadState::Complete, None));
                self.progress_writes.forget(&item.guid);
                ops
            }
            DownloadEvent::Failed {
                error, resumable, ..
            } => {
                // A page-started download whose second request failed before the server sent
                // anything: most likely a link that works once.
                let error = if item.refetched && item.record == Record::None {
                    NOT_SENT_AGAIN.to_owned()
                } else {
                    error
                };
                item.state = ItemState::Interrupted { resumable };
                item.bytes_per_second = 0;
                item.transfer = None;
                item.error = Some(error.clone());
                if !resumable {
                    // Nothing is left on disk to continue from.
                    item.received = 0;
                }
                let ops = item.close_record(DownloadState::Interrupted, Some(error));
                self.progress_writes.forget(&item.guid);
                ops
            }
            DownloadEvent::Cancelled { .. } => {
                item.state = ItemState::Cancelled;
                item.bytes_per_second = 0;
                item.received = 0;
                item.transfer = None;
                let ops = item.close_record(DownloadState::Cancelled, None);
                self.progress_writes.forget(&item.guid);
                ops
            }
        }
    }

    /// Records of earlier runs (and of this one) read from the database, newest first. Items
    /// already in the list are kept as they are. `exists` tells whether a file is on disk.
    pub(crate) fn merge_records(
        &mut self,
        records: Vec<DownloadRecord>,
        facts: impl Fn(&Path) -> Option<FileFacts>,
    ) {
        for record in records {
            if self.item(&record.guid).is_some() {
                continue;
            }
            let target_path = PathBuf::from(&record.target_path);
            let partial_path = record.temp_path.as_ref().map(PathBuf::from);
            let state = match record.state {
                DownloadState::Complete => ItemState::Complete,
                DownloadState::Cancelled => ItemState::Cancelled,
                // An in-progress record of another run cannot be running here; start-up marks
                // those interrupted before any request, so this is only a safeguard.
                DownloadState::Interrupted | DownloadState::InProgress => {
                    let resumable = (record.etag.is_some() || record.last_modified.is_some())
                        && partial_path
                            .as_deref()
                            .and_then(&facts)
                            .is_some_and(|facts| facts.regular);
                    ItemState::Interrupted { resumable }
                }
            };
            let file_name = target_path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| record.url.clone());
            let target_facts = facts(&target_path);
            let mut item = Item {
                guid: record.guid,
                url: record.url,
                referrer: record.referrer,
                file_name,
                file_exists: false,
                target_path: Some(target_path),
                partial_path,
                mime_type: record.mime_type,
                final_url: record.final_url,
                etag: record.etag,
                last_modified: record.last_modified,
                received: u64::try_from(record.received_bytes).unwrap_or(0),
                total: record
                    .total_bytes
                    .and_then(|total| u64::try_from(total).ok()),
                bytes_per_second: 0,
                state,
                error: record.error,
                started_at_ms: record.started_at_ms,
                this_run: false,
                transfer: None,
                record: Record::Closed,
                suggested_name: None,
                refetched: false,
                can_retry: true,
                identity: None,
            };
            item.file_exists =
                state == ItemState::Complete && file_still_matches(&item, target_facts);
            // Records come newest first; each goes after the items at least as new.
            let position = self
                .items
                .iter()
                .position(|other| other.started_at_ms < item.started_at_ms)
                .unwrap_or(self.items.len());
            self.items.insert(position, item);
        }
    }

    /// An interrupted, resumable download is being resumed by `transfer`. Returns the database
    /// writes, or `None` if it cannot be resumed.
    pub(crate) fn resume(&mut self, guid: &str, transfer: DownloadId) -> Option<Vec<RecordOp>> {
        let item = self.items.iter_mut().find(|item| item.guid == guid)?;
        item.resume_from()?;
        item.state = ItemState::Starting;
        item.error = None;
        item.transfer = Some(transfer);
        let mut ops = Vec::new();
        if item.record == Record::Closed {
            ops.push(RecordOp::Resume {
                guid: item.guid.clone(),
            });
            item.record = Record::Open;
        }
        Some(ops)
    }

    /// The user cancelled an interrupted download that is not running: it becomes cancelled,
    /// and its partial file (returned) is to be removed.
    pub(crate) fn cancel_interrupted(
        &mut self,
        guid: &str,
    ) -> Option<(Vec<RecordOp>, Option<ResumeFrom>)> {
        let item = self.items.iter_mut().find(|item| item.guid == guid)?;
        if !matches!(item.state, ItemState::Interrupted { .. }) {
            return None;
        }
        let partial = item.partial_file();
        item.state = ItemState::Cancelled;
        item.received = 0;
        item.error = None;
        let ops = if item.record == Record::None {
            Vec::new()
        } else {
            item.record = Record::Closed;
            vec![RecordOp::Finish {
                guid: item.guid.clone(),
                state: DownloadState::Cancelled,
                error: None,
            }]
        };
        Some((ops, partial))
    }

    /// Removes a download that is not running from the list (not its file). Returns the
    /// database writes and the partial file of an interrupted download, to be removed.
    pub(crate) fn remove(&mut self, guid: &str) -> Option<(Vec<RecordOp>, Option<ResumeFrom>)> {
        let index = self
            .items
            .iter()
            .position(|item| item.guid == guid && !item.is_active())?;
        let item = self.items.remove(index);
        let ops = if item.record == Record::None {
            Vec::new()
        } else {
            vec![RecordOp::Remove {
                guid: item.guid.clone(),
            }]
        };
        Some((ops, leftover_partial(&item)))
    }

    /// Removes every download that is not running. Returns the database writes and the partial
    /// files of interrupted downloads, to be removed.
    pub(crate) fn clear(&mut self) -> (Vec<RecordOp>, Vec<ResumeFrom>) {
        let (kept, removed): (Vec<Item>, Vec<Item>) = std::mem::take(&mut self.items)
            .into_iter()
            .partition(Item::is_active);
        self.items = kept;
        (
            vec![RecordOp::Clear],
            removed.iter().filter_map(leftover_partial).collect(),
        )
    }

    /// Marks a complete download's file as present or not.
    pub(crate) fn refresh_file_presence(&mut self, facts: impl Fn(&Path) -> Option<FileFacts>) {
        for item in &mut self.items {
            if item.state == ItemState::Complete {
                let found = item.target_path.as_deref().and_then(&facts);
                item.file_exists = file_still_matches(item, found);
            }
        }
    }
}

/// The partial file an interrupted download leaves behind, to remove with it.
fn leftover_partial(item: &Item) -> Option<ResumeFrom> {
    matches!(item.state, ItemState::Interrupted { .. })
        .then(|| item.partial_file())
        .flatten()
}

fn event_id(event: &DownloadEvent) -> DownloadId {
    match event {
        DownloadEvent::Started { id, .. }
        | DownloadEvent::Progress { id, .. }
        | DownloadEvent::Finished { id, .. }
        | DownloadEvent::Failed { id, .. }
        | DownloadEvent::Cancelled { id } => *id,
    }
}

fn to_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use url::Url;

    use super::*;

    const T0: i64 = 1_800_000_000_000;

    fn new_item(guid: &str, id: u64, at: i64) -> NewItem {
        NewItem {
            guid: guid.into(),
            url: format!("https://example.com/{guid}.zip"),
            referrer: Some("https://example.com/".into()),
            name_guess: format!("{guid}.zip"),
            transfer: DownloadId(id),
            started_at_ms: at,
            suggested_name: None,
            refetched: false,
        }
    }

    /// Every path is a regular file of the records' 40 bytes.
    fn present(_: &Path) -> Option<FileFacts> {
        Some(FileFacts {
            regular: true,
            len: 40,
            id: (1, 1),
        })
    }

    fn started(id: u64, name: &str, total: Option<u64>) -> DownloadEvent {
        DownloadEvent::Started {
            id: DownloadId(id),
            file_name: name.into(),
            target_path: PathBuf::from(format!("/dl/{name}")),
            partial_path: PathBuf::from(format!("/dl/{name}.part")),
            total_bytes: total,
            mime_type: Some("application/zip".into()),
            final_url: Url::parse(&format!("https://cdn.example.com/{name}")).unwrap(),
            etag: Some("\"v1\"".into()),
            last_modified: None,
        }
    }

    fn progress(id: u64, received: u64, total: Option<u64>) -> DownloadEvent {
        DownloadEvent::Progress {
            id: DownloadId(id),
            received_bytes: received,
            total_bytes: total,
            bytes_per_second: 1000,
        }
    }

    fn record(guid: &str, state: DownloadState, at: i64) -> DownloadRecord {
        DownloadRecord {
            id: mercury_store::DownloadRecordId(at),
            guid: guid.into(),
            url: format!("https://example.com/{guid}"),
            final_url: None,
            referrer: None,
            target_path: format!("/dl/{guid}.bin"),
            temp_path: Some(format!("/dl/{guid}.bin.part")),
            mime_type: None,
            total_bytes: Some(100),
            received_bytes: 40,
            state,
            error: None,
            etag: Some("\"e\"".into()),
            last_modified: None,
            started_at_ms: at,
            ended_at_ms: Some(at + 1),
        }
    }

    #[test]
    fn a_download_goes_from_starting_to_complete_with_its_record_writes() {
        let now = Instant::now();
        let mut list = DownloadList::default();
        list.begin(new_item("a", 1, T0));
        assert_eq!(list.item("a").unwrap().state, ItemState::Starting);
        assert_eq!(list.active_count(), 1);
        assert!(list.has_items_this_run());

        let ops = list.apply(started(1, "a.zip", Some(100)), now);
        assert_eq!(
            ops,
            vec![
                RecordOp::Insert(DownloadStart {
                    guid: "a".into(),
                    url: "https://example.com/a.zip".into(),
                    referrer: Some("https://example.com/".into()),
                    target_path: "/dl/a.zip".into(),
                    temp_path: Some("/dl/a.zip.part".into()),
                    mime_type: Some("application/zip".into()),
                    total_bytes: Some(100),
                    started_at_ms: T0,
                }),
                RecordOp::Validators {
                    guid: "a".into(),
                    final_url: Some("https://cdn.example.com/a.zip".into()),
                    etag: Some("\"v1\"".into()),
                    last_modified: None,
                },
            ]
        );
        let item = list.item("a").unwrap();
        assert_eq!(item.state, ItemState::InProgress);
        assert_eq!(item.file_name, "a.zip");

        // The first progress report is written, the next within a second is not.
        assert_eq!(
            list.apply(progress(1, 10, Some(100)), now),
            vec![RecordOp::Progress {
                guid: "a".into(),
                received: 10,
                total: Some(100),
            }]
        );
        assert_eq!(
            list.apply(progress(1, 20, Some(100)), now + Duration::from_millis(300)),
            vec![]
        );
        assert_eq!(list.item("a").unwrap().received, 20);
        assert_eq!(list.overall_progress(), Some(0.2));

        // Finished under another name: the name is recorded, then the final count and state.
        let ops = list.apply(
            DownloadEvent::Finished {
                id: DownloadId(1),
                target_path: PathBuf::from("/dl/a (1).zip"),
                received_bytes: 100,
            },
            now + Duration::from_millis(400),
        );
        assert_eq!(
            ops,
            vec![
                RecordOp::TargetPath {
                    guid: "a".into(),
                    path: "/dl/a (1).zip".into(),
                },
                RecordOp::Progress {
                    guid: "a".into(),
                    received: 100,
                    total: Some(100),
                },
                RecordOp::Finish {
                    guid: "a".into(),
                    state: DownloadState::Complete,
                    error: None,
                },
            ]
        );
        let item = list.item("a").unwrap();
        assert_eq!(item.state, ItemState::Complete);
        assert_eq!(item.file_name, "a (1).zip");
        assert!(item.file_exists);
        assert_eq!(item.transfer, None);
        assert_eq!(list.active_count(), 0);
        assert_eq!(list.overall_progress(), None);
    }

    #[test]
    fn a_failure_before_the_answer_writes_nothing() {
        let mut list = DownloadList::default();
        list.begin(new_item("a", 1, T0));
        let ops = list.apply(
            DownloadEvent::Failed {
                id: DownloadId(1),
                error: "the server answered HTTP 404 Not Found".into(),
                resumable: false,
            },
            Instant::now(),
        );
        assert_eq!(ops, vec![]);
        let item = list.item("a").unwrap();
        assert_eq!(item.state, ItemState::Interrupted { resumable: false });
        assert_eq!(
            item.error.as_deref(),
            Some("the server answered HTTP 404 Not Found")
        );
        // It can only be removed from the list, which writes nothing either.
        assert_eq!(list.remove("a"), Some((vec![], None)));
        assert!(list.items().is_empty());
    }

    #[test]
    fn an_interrupted_download_is_recorded_and_resumes_from_its_partial_file() {
        let now = Instant::now();
        let mut list = DownloadList::default();
        list.begin(new_item("a", 1, T0));
        list.apply(started(1, "a.zip", Some(100)), now);
        list.apply(progress(1, 40, Some(100)), now);
        let ops = list.apply(
            DownloadEvent::Failed {
                id: DownloadId(1),
                error: "the download was interrupted".into(),
                resumable: true,
            },
            now,
        );
        assert_eq!(
            ops,
            vec![
                RecordOp::Progress {
                    guid: "a".into(),
                    received: 40,
                    total: Some(100),
                },
                RecordOp::Finish {
                    guid: "a".into(),
                    state: DownloadState::Interrupted,
                    error: Some("the download was interrupted".into()),
                },
            ]
        );
        assert_eq!(
            list.item("a").unwrap().resume_from(),
            Some(ResumeFrom {
                partial_path: "/dl/a.zip.part".into(),
                target_path: "/dl/a.zip".into(),
                etag: Some("\"v1\"".into()),
                last_modified: None,
            })
        );

        // Resuming reopens the record; the next `Started` updates it instead of inserting.
        assert_eq!(
            list.resume("a", DownloadId(2)),
            Some(vec![RecordOp::Resume { guid: "a".into() }])
        );
        assert_eq!(list.item("a").unwrap().state, ItemState::Starting);
        assert_eq!(list.item("a").unwrap().error, None);
        let ops = list.apply(started(2, "a.zip", Some(100)), now);
        assert_eq!(
            ops,
            vec![RecordOp::Validators {
                guid: "a".into(),
                final_url: Some("https://cdn.example.com/a.zip".into()),
                etag: Some("\"v1\"".into()),
                last_modified: None,
            }]
        );
        assert_eq!(list.item("a").unwrap().state, ItemState::InProgress);
        // Resuming twice is refused while it runs.
        assert_eq!(list.resume("a", DownloadId(3)), None);
    }

    #[test]
    fn a_download_that_cannot_resume_is_not_offered_resuming() {
        let now = Instant::now();
        let mut list = DownloadList::default();
        list.begin(new_item("a", 1, T0));
        list.apply(started(1, "a.zip", None), now);
        list.apply(
            DownloadEvent::Failed {
                id: DownloadId(1),
                error: "network error".into(),
                resumable: false,
            },
            now,
        );
        assert_eq!(
            list.item("a").unwrap().state,
            ItemState::Interrupted { resumable: false }
        );
        assert_eq!(list.item("a").unwrap().resume_from(), None);
        assert_eq!(list.resume("a", DownloadId(2)), None);
    }

    #[test]
    fn cancelling_a_running_download_records_it_cancelled() {
        let now = Instant::now();
        let mut list = DownloadList::default();
        list.begin(new_item("a", 1, T0));
        list.apply(started(1, "a.zip", None), now);
        assert_eq!(list.running_transfers(), vec![DownloadId(1)]);
        let ops = list.apply(DownloadEvent::Cancelled { id: DownloadId(1) }, now);
        assert_eq!(
            ops,
            vec![RecordOp::Finish {
                guid: "a".into(),
                state: DownloadState::Cancelled,
                error: None,
            }]
        );
        assert_eq!(list.item("a").unwrap().state, ItemState::Cancelled);
        assert!(list.running_transfers().is_empty());
    }

    #[test]
    fn events_of_unknown_transfers_are_ignored() {
        let mut list = DownloadList::default();
        assert_eq!(
            list.apply(progress(9, 1, None), Instant::now()),
            Vec::<RecordOp>::new()
        );
    }

    #[test]
    fn earlier_records_merge_newest_first_without_replacing_this_run() {
        let mut list = DownloadList::default();
        list.begin(new_item("now", 1, T0 + 100));
        list.merge_records(
            vec![
                record("now", DownloadState::InProgress, T0 + 100),
                record("old", DownloadState::Complete, T0 - 10),
                record("older", DownloadState::Interrupted, T0 - 20),
                record("gone", DownloadState::Complete, T0 - 5),
            ],
            |path| present(path).filter(|_| path != Path::new("/dl/gone.bin")),
        );
        let guids: Vec<&str> = list.items().iter().map(|item| item.guid.as_str()).collect();
        assert_eq!(guids, ["now", "gone", "old", "older"]);
        assert_eq!(list.item("now").unwrap().state, ItemState::Starting);
        assert!(list.item("now").unwrap().this_run);
        let old = list.item("old").unwrap();
        assert_eq!(
            (old.state, old.this_run, old.file_exists),
            (ItemState::Complete, false, true)
        );
        assert!(!list.item("gone").unwrap().file_exists);
        // Interrupted with a validator and its partial file present: resumable.
        assert_eq!(
            list.item("older").unwrap().state,
            ItemState::Interrupted { resumable: true }
        );
        assert_eq!(list.item("old").unwrap().file_name, "old.bin");

        // Only this run's downloads make the toolbar button appear.
        let mut earlier_only = DownloadList::default();
        earlier_only.merge_records(vec![record("old", DownloadState::Complete, T0)], present);
        assert!(!earlier_only.has_items_this_run());
    }

    #[test]
    fn an_earlier_interrupted_download_without_its_partial_file_cannot_resume() {
        let mut list = DownloadList::default();
        list.merge_records(
            vec![record("older", DownloadState::Interrupted, T0)],
            |path| present(path).filter(|_| path != Path::new("/dl/older.bin.part")),
        );
        assert_eq!(
            list.item("older").unwrap().state,
            ItemState::Interrupted { resumable: false }
        );
        let mut no_validator = record("plain", DownloadState::Interrupted, T0);
        no_validator.etag = None;
        list.merge_records(vec![no_validator], present);
        assert_eq!(
            list.item("plain").unwrap().state,
            ItemState::Interrupted { resumable: false }
        );
    }

    #[test]
    fn an_earlier_interrupted_download_resumes_by_reopening_its_record() {
        let mut list = DownloadList::default();
        list.merge_records(
            vec![record("older", DownloadState::Interrupted, T0)],
            present,
        );
        assert_eq!(
            list.resume("older", DownloadId(5)),
            Some(vec![RecordOp::Resume {
                guid: "older".into()
            }])
        );
        assert_eq!(list.active_count(), 1);
        // A failure that leaves nothing to continue from closes the reopened record again, with
        // nothing received.
        let ops = list.apply(
            DownloadEvent::Failed {
                id: DownloadId(5),
                error: "the server answered HTTP 404".into(),
                resumable: false,
            },
            Instant::now(),
        );
        assert_eq!(
            ops,
            vec![
                RecordOp::Progress {
                    guid: "older".into(),
                    received: 0,
                    total: Some(100),
                },
                RecordOp::Finish {
                    guid: "older".into(),
                    state: DownloadState::Interrupted,
                    error: Some("the server answered HTTP 404".into()),
                }
            ]
        );
    }

    #[test]
    fn cancelling_an_interrupted_download_discards_its_partial_file() {
        let mut list = DownloadList::default();
        list.merge_records(
            vec![record("older", DownloadState::Interrupted, T0)],
            present,
        );
        let (ops, partial) = list.cancel_interrupted("older").unwrap();
        assert_eq!(
            ops,
            vec![RecordOp::Finish {
                guid: "older".into(),
                state: DownloadState::Cancelled,
                error: None,
            }]
        );
        assert_eq!(
            partial.map(|resume| resume.partial_path),
            Some(PathBuf::from("/dl/older.bin.part"))
        );
        assert_eq!(list.item("older").unwrap().state, ItemState::Cancelled);
        // Not interrupted any more.
        assert!(list.cancel_interrupted("older").is_none());
    }

    #[test]
    fn running_downloads_are_never_removed_or_cleared() {
        let now = Instant::now();
        let mut list = DownloadList::default();
        list.begin(new_item("running", 1, T0 + 3));
        list.apply(started(1, "running.zip", None), now);
        list.begin(new_item("done", 2, T0 + 2));
        list.apply(started(2, "done.zip", None), now);
        list.apply(
            DownloadEvent::Finished {
                id: DownloadId(2),
                target_path: "/dl/done.zip".into(),
                received_bytes: 5,
            },
            now,
        );
        list.merge_records(
            vec![record("older", DownloadState::Interrupted, T0)],
            present,
        );

        assert_eq!(list.remove("running"), None);
        let (ops, partials) = list.clear();
        assert_eq!(ops, vec![RecordOp::Clear]);
        assert_eq!(
            partials
                .into_iter()
                .map(|resume| resume.partial_path)
                .collect::<Vec<_>>(),
            vec![PathBuf::from("/dl/older.bin.part")]
        );
        let guids: Vec<&str> = list.items().iter().map(|item| item.guid.as_str()).collect();
        assert_eq!(guids, ["running"]);
    }

    #[test]
    fn removing_a_finished_download_removes_its_record_only() {
        let now = Instant::now();
        let mut list = DownloadList::default();
        list.begin(new_item("a", 1, T0));
        list.apply(started(1, "a.zip", None), now);
        list.apply(
            DownloadEvent::Finished {
                id: DownloadId(1),
                target_path: "/dl/a.zip".into(),
                received_bytes: 5,
            },
            now,
        );
        assert_eq!(
            list.remove("a"),
            Some((vec![RecordOp::Remove { guid: "a".into() }], None))
        );
        assert!(list.item("a").is_none());
        assert_eq!(list.remove("a"), None);
    }

    #[test]
    fn a_deleted_file_is_noticed() {
        let now = Instant::now();
        let mut list = DownloadList::default();
        list.begin(new_item("a", 1, T0));
        list.apply(started(1, "a.zip", None), now);
        list.apply(
            DownloadEvent::Finished {
                id: DownloadId(1),
                target_path: "/dl/a.zip".into(),
                received_bytes: 5,
            },
            now,
        );
        list.refresh_file_presence(|_| None);
        assert!(!list.item("a").unwrap().file_exists);
    }

    #[test]
    fn overall_progress_counts_only_active_downloads_of_known_size() {
        let now = Instant::now();
        let mut list = DownloadList::default();
        list.begin(new_item("a", 1, T0));
        list.begin(new_item("b", 2, T0 + 1));
        list.begin(new_item("c", 3, T0 + 2));
        // Starting downloads have no size yet.
        assert_eq!(list.overall_progress(), None);
        list.apply(started(1, "a.zip", Some(100)), now);
        list.apply(started(2, "b.zip", Some(300)), now);
        list.apply(started(3, "c.zip", None), now);
        list.apply(progress(1, 100, Some(100)), now);
        list.apply(progress(2, 100, Some(300)), now);
        list.apply(progress(3, 999, None), now);
        assert_eq!(list.overall_progress(), Some(0.5));
        // Newest first.
        let guids: Vec<&str> = list.items().iter().map(|item| item.guid.as_str()).collect();
        assert_eq!(guids, ["c", "b", "a"]);
    }

    // Review round 1.

    #[test]
    fn retry_keeps_the_suggested_name_and_how_it_was_fetched() {
        let mut list = DownloadList::default();
        list.begin(NewItem {
            suggested_name: Some("photo.png".into()),
            refetched: true,
            ..new_item("a", 1, T0)
        });
        // Not while it runs.
        assert_eq!(list.retry_source("a"), None);
        list.apply(
            DownloadEvent::Cancelled { id: DownloadId(1) },
            Instant::now(),
        );
        assert_eq!(
            list.retry_source("a"),
            Some(RetrySource {
                url: "https://example.com/a.zip".into(),
                referrer: Some("https://example.com/".into()),
                suggested_name: Some("photo.png".into()),
                refetched: true,
            })
        );
    }

    #[test]
    fn a_refetch_that_fails_before_the_answer_says_the_site_did_not_send_it_again() {
        let mut list = DownloadList::default();
        list.begin(NewItem {
            refetched: true,
            ..new_item("a", 1, T0)
        });
        list.apply(
            DownloadEvent::Failed {
                id: DownloadId(1),
                error: "the server answered HTTP 410 Gone".into(),
                resumable: false,
            },
            Instant::now(),
        );
        assert_eq!(
            list.item("a").unwrap().error.as_deref(),
            Some(NOT_SENT_AGAIN)
        );
        // A download the user asked for keeps the server's reason.
        list.begin(new_item("b", 2, T0 + 1));
        list.apply(
            DownloadEvent::Failed {
                id: DownloadId(2),
                error: "the server answered HTTP 410 Gone".into(),
                resumable: false,
            },
            Instant::now(),
        );
        assert_eq!(
            list.item("b").unwrap().error.as_deref(),
            Some("the server answered HTTP 410 Gone")
        );
    }

    #[test]
    fn a_note_is_shown_but_never_stored_or_retried() {
        let mut list = DownloadList::default();
        list.add_note(
            "n".into(),
            "https://example.com/report".into(),
            "report".into(),
            "This file was sent in answer to a form and cannot be saved".into(),
            T0,
        );
        let item = list.item("n").unwrap();
        assert_eq!(item.state, ItemState::Interrupted { resumable: false });
        assert!(item.this_run);
        assert_eq!(list.retry_source("n"), None);
        assert_eq!(list.remove("n"), Some((vec![], None)));
    }

    #[test]
    fn a_finished_file_must_still_be_the_one_downloaded() {
        let now = Instant::now();
        let mut list = DownloadList::default();
        list.begin(new_item("a", 1, T0));
        list.apply(started(1, "a.zip", Some(5)), now);
        list.apply(
            DownloadEvent::Finished {
                id: DownloadId(1),
                target_path: "/dl/a.zip".into(),
                received_bytes: 5,
            },
            now,
        );
        let same = FileFacts {
            regular: true,
            len: 5,
            id: (7, 42),
        };
        list.record_identity("a", Some(same));
        let item = list.item("a").unwrap().clone();
        assert!(file_still_matches(&item, Some(same)));
        // Replaced by another file, grown, a link, or gone.
        assert!(!file_still_matches(
            &item,
            Some(FileFacts {
                id: (7, 43),
                ..same
            })
        ));
        assert!(!file_still_matches(
            &item,
            Some(FileFacts { len: 6, ..same })
        ));
        assert!(!file_still_matches(
            &item,
            Some(FileFacts {
                regular: false,
                ..same
            })
        ));
        assert!(!file_still_matches(&item, None));
        list.refresh_file_presence(|_| Some(FileFacts { id: (1, 1), ..same }));
        assert!(!list.item("a").unwrap().file_exists);
    }
}
