//! What the downloads list shows for each download: its status line, its progress bar and the
//! buttons it offers.

use super::format::{amount, is_executable_type, progress_line};
use super::model::{Item, ItemState};

/// No progress bar.
pub(crate) const NO_BAR: f32 = -1.0;
/// A bar without a known size.
pub(crate) const INDETERMINATE_BAR: f32 = 2.0;

/// The buttons a download offers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Actions {
    pub(crate) open: bool,
    pub(crate) show: bool,
    pub(crate) cancel: bool,
    pub(crate) resume: bool,
    pub(crate) retry: bool,
    pub(crate) remove: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ItemView {
    pub(crate) guid: String,
    pub(crate) name: String,
    pub(crate) status: String,
    /// 0 to 1, `NO_BAR` or `INDETERMINATE_BAR`.
    pub(crate) progress: f32,
    pub(crate) failed: bool,
    pub(crate) actions: Actions,
}

pub(crate) fn item_view(item: &Item) -> ItemView {
    let fraction = || match item.total {
        Some(total) if total > 0 => (item.received.min(total) as f64 / total as f64) as f32,
        _ => INDETERMINATE_BAR,
    };
    let (status, progress, actions) = match item.state {
        ItemState::Starting => (
            "Starting…".to_owned(),
            INDETERMINATE_BAR,
            Actions {
                cancel: true,
                ..Actions::default()
            },
        ),
        ItemState::InProgress => (
            progress_line(item.received, item.total, item.bytes_per_second),
            fraction(),
            Actions {
                cancel: true,
                ..Actions::default()
            },
        ),
        ItemState::Complete if item.file_exists => {
            let mut status = mercury_downloads::format_bytes(item.received);
            if let Some(host) = url::Url::parse(&item.url)
                .ok()
                .and_then(|url| url.host_str().map(str::to_owned))
            {
                status.push_str(" — ");
                status.push_str(&host);
            }
            let executable = item.target_path.as_deref().is_some_and(is_executable_type);
            (
                status,
                NO_BAR,
                Actions {
                    open: !executable,
                    show: true,
                    remove: true,
                    ..Actions::default()
                },
            )
        }
        ItemState::Complete => ("Deleted".to_owned(), NO_BAR, retry_or_remove()),
        ItemState::Cancelled => ("Cancelled".to_owned(), NO_BAR, retry_or_remove()),
        ItemState::Interrupted { resumable: true } => (
            format!("Interrupted — {}", amount(item.received, item.total)),
            match fraction() {
                INDETERMINATE_BAR => NO_BAR,
                fraction => fraction,
            },
            Actions {
                resume: true,
                cancel: true,
                remove: true,
                ..Actions::default()
            },
        ),
        ItemState::Interrupted { resumable: false } => (
            match &item.error {
                Some(error) => format!("Failed — {error}"),
                None => "Failed".to_owned(),
            },
            NO_BAR,
            retry_or_remove(),
        ),
    };
    ItemView {
        guid: item.guid.clone(),
        name: item.file_name.clone(),
        status,
        progress,
        failed: item.state == ItemState::Interrupted { resumable: false },
        actions,
    }
}

fn retry_or_remove() -> Actions {
    Actions {
        retry: true,
        remove: true,
        ..Actions::default()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Instant;

    use mercury_downloads::{DownloadEvent, DownloadId};
    use mercury_store::{DownloadRecord, DownloadRecordId, DownloadState};

    use super::*;
    use crate::downloads::model::{DownloadList, NewItem};

    fn list_with(state: DownloadState, target: &str, exists: bool) -> DownloadList {
        let mut list = DownloadList::default();
        list.merge_records(
            vec![DownloadRecord {
                id: DownloadRecordId(1),
                guid: "g".into(),
                url: "https://files.example.com/x".into(),
                final_url: None,
                referrer: None,
                target_path: target.into(),
                temp_path: Some(format!("{target}.part")),
                mime_type: None,
                total_bytes: Some(2048),
                received_bytes: 1024,
                state,
                error: Some("the server answered HTTP 404 Not Found".into()),
                etag: Some("\"e\"".into()),
                last_modified: None,
                started_at_ms: 1,
                ended_at_ms: Some(2),
            }],
            |_| exists,
        );
        list
    }

    #[test]
    fn a_running_download_shows_its_progress_and_can_be_cancelled() {
        let mut list = DownloadList::default();
        list.begin(NewItem {
            guid: "g".into(),
            url: "https://example.com/a.zip".into(),
            referrer: None,
            name_guess: "a.zip".into(),
            transfer: DownloadId(1),
            started_at_ms: 1,
        });
        let starting = item_view(&list.items()[0]);
        assert_eq!(starting.status, "Starting…");
        assert_eq!(starting.progress, INDETERMINATE_BAR);
        assert_eq!(
            starting.actions,
            Actions {
                cancel: true,
                ..Actions::default()
            }
        );

        let now = Instant::now();
        list.apply(
            DownloadEvent::Started {
                id: DownloadId(1),
                file_name: "a.zip".into(),
                target_path: PathBuf::from("/dl/a.zip"),
                partial_path: PathBuf::from("/dl/a.zip.part"),
                total_bytes: Some(4096),
                mime_type: None,
                final_url: url::Url::parse("https://example.com/a.zip").unwrap(),
                etag: None,
                last_modified: None,
            },
            now,
        );
        list.apply(
            DownloadEvent::Progress {
                id: DownloadId(1),
                received_bytes: 1024,
                total_bytes: Some(4096),
                bytes_per_second: 1024,
            },
            now,
        );
        let running = item_view(&list.items()[0]);
        assert_eq!(running.status, "1.0 KB of 4.0 KB — 1.0 KB/s, 3 s left");
        assert_eq!(running.progress, 0.25);
        assert!(running.actions.cancel && !running.actions.remove);
    }

    #[test]
    fn a_complete_download_can_be_opened_shown_and_removed() {
        let list = list_with(DownloadState::Complete, "/dl/report.pdf", true);
        let view = item_view(&list.items()[0]);
        assert_eq!(view.name, "report.pdf");
        assert_eq!(view.status, "1.0 KB — files.example.com");
        assert_eq!(view.progress, NO_BAR);
        assert!(!view.failed);
        assert_eq!(
            view.actions,
            Actions {
                open: true,
                show: true,
                remove: true,
                ..Actions::default()
            }
        );
    }

    #[test]
    fn a_program_is_never_one_click_from_running() {
        let list = list_with(DownloadState::Complete, "/dl/Setup.pkg", true);
        let actions = item_view(&list.items()[0]).actions;
        assert!(!actions.open);
        assert!(actions.show);
    }

    #[test]
    fn a_deleted_file_can_only_be_downloaded_again_or_removed() {
        let list = list_with(DownloadState::Complete, "/dl/report.pdf", false);
        let view = item_view(&list.items()[0]);
        assert_eq!(view.status, "Deleted");
        assert_eq!(
            view.actions,
            Actions {
                retry: true,
                remove: true,
                ..Actions::default()
            }
        );
    }

    #[test]
    fn interrupted_cancelled_and_failed_downloads() {
        let list = list_with(DownloadState::Interrupted, "/dl/big.iso", true);
        let view = item_view(&list.items()[0]);
        assert_eq!(view.status, "Interrupted — 1.0 KB of 2.0 KB");
        assert_eq!(view.progress, 0.5);
        assert_eq!(
            view.actions,
            Actions {
                resume: true,
                cancel: true,
                remove: true,
                ..Actions::default()
            }
        );

        let list = list_with(DownloadState::Interrupted, "/dl/big.iso", false);
        let view = item_view(&list.items()[0]);
        assert_eq!(
            view.status,
            "Failed — the server answered HTTP 404 Not Found"
        );
        assert!(view.failed);
        assert_eq!(
            view.actions,
            Actions {
                retry: true,
                remove: true,
                ..Actions::default()
            }
        );

        let list = list_with(DownloadState::Cancelled, "/dl/big.iso", false);
        let view = item_view(&list.items()[0]);
        assert_eq!(view.status, "Cancelled");
        assert_eq!(
            view.actions,
            Actions {
                retry: true,
                remove: true,
                ..Actions::default()
            }
        );
        let _ = ItemState::Cancelled;
    }
}
