mod common;

use common::NOW;
use mercury_store::{
    DownloadRecord, DownloadRecordId, DownloadState, NewDownload, Store, StoreError,
};

fn new(guid: &str, started_at_ms: i64) -> NewDownload<'_> {
    NewDownload {
        guid,
        url: "https://example.com/file.zip",
        referrer: Some("https://example.com/"),
        target_path: "/Users/me/Downloads/file.zip",
        temp_path: Some("/Users/me/Downloads/file.zip.part"),
        mime_type: Some("application/zip"),
        total_bytes: None,
        started_at_ms,
    }
}

fn find(store: &Store, id: DownloadRecordId) -> DownloadRecord {
    store
        .downloads(100)
        .unwrap()
        .into_iter()
        .find(|d| d.id == id)
        .expect("record listed")
}

#[test]
fn insert_progress_validators_finish_round_trip() {
    let mut store = Store::open_in_memory().unwrap();
    let id = store.insert_download(&new("d-1", NOW)).unwrap();
    assert_eq!(
        find(&store, id),
        DownloadRecord {
            id,
            guid: "d-1".into(),
            url: "https://example.com/file.zip".into(),
            final_url: None,
            referrer: Some("https://example.com/".into()),
            target_path: "/Users/me/Downloads/file.zip".into(),
            temp_path: Some("/Users/me/Downloads/file.zip.part".into()),
            mime_type: Some("application/zip".into()),
            total_bytes: None,
            received_bytes: 0,
            state: DownloadState::InProgress,
            error: None,
            etag: None,
            last_modified: None,
            started_at_ms: NOW,
            ended_at_ms: None,
        }
    );

    store.update_download_progress(id, 100, Some(1000)).unwrap();
    store.update_download_progress(id, 500, None).unwrap();
    store
        .set_download_validators(
            id,
            Some("https://cdn.example.com/file.zip"),
            Some("\"abc\""),
            Some("Sun, 15 Nov 2026 00:00:00 GMT"),
        )
        .unwrap();
    let record = find(&store, id);
    assert_eq!(
        (record.received_bytes, record.total_bytes),
        (500, Some(1000))
    );
    assert_eq!(
        record.final_url.as_deref(),
        Some("https://cdn.example.com/file.zip")
    );
    assert_eq!(record.etag.as_deref(), Some("\"abc\""));
    assert_eq!(
        record.last_modified.as_deref(),
        Some("Sun, 15 Nov 2026 00:00:00 GMT")
    );

    store
        .update_download_progress(id, 1000, Some(1000))
        .unwrap();
    store
        .finish_download(id, DownloadState::Complete, None, NOW + 5000)
        .unwrap();
    let record = find(&store, id);
    assert_eq!(record.state, DownloadState::Complete);
    assert_eq!(record.ended_at_ms, Some(NOW + 5000));
    assert_eq!(record.received_bytes, 1000);

    store.set_download_validators(id, None, None, None).unwrap();
    let record = find(&store, id);
    assert_eq!(
        (record.final_url, record.etag, record.last_modified),
        (None, None, None)
    );
}

#[test]
fn finishing_with_an_error_and_invalid_calls() {
    let mut store = Store::open_in_memory().unwrap();
    let id = store.insert_download(&new("d-1", NOW)).unwrap();
    store
        .finish_download(
            id,
            DownloadState::Interrupted,
            Some("network error"),
            NOW + 1,
        )
        .unwrap();
    let record = find(&store, id);
    assert_eq!(record.state, DownloadState::Interrupted);
    assert_eq!(record.error.as_deref(), Some("network error"));

    assert!(matches!(
        store.finish_download(id, DownloadState::InProgress, None, NOW),
        Err(StoreError::InvalidArgument(_))
    ));
    let missing = DownloadRecordId(999);
    assert!(matches!(
        store.update_download_progress(missing, 1, None),
        Err(StoreError::NotFound)
    ));
    assert!(matches!(
        store.set_download_validators(missing, None, None, None),
        Err(StoreError::NotFound)
    ));
    assert!(matches!(
        store.finish_download(missing, DownloadState::Complete, None, NOW),
        Err(StoreError::NotFound)
    ));
    assert!(matches!(
        store.insert_download(&new("d-1", NOW)),
        Err(StoreError::DuplicateGuid(_))
    ));
}

#[test]
fn listing_is_newest_first_and_limited() {
    let mut store = Store::open_in_memory().unwrap();
    let a = store.insert_download(&new("a", NOW)).unwrap();
    let b = store.insert_download(&new("b", NOW + 2)).unwrap();
    let c = store.insert_download(&new("c", NOW + 1)).unwrap();
    let ids: Vec<DownloadRecordId> = store.downloads(10).unwrap().iter().map(|d| d.id).collect();
    assert_eq!(ids, [b, c, a]);
    assert_eq!(store.downloads(2).unwrap().len(), 2);
    assert!(store.downloads(0).unwrap().is_empty());
}

#[test]
fn mark_unfinished_changes_only_in_progress_rows() {
    let mut store = Store::open_in_memory().unwrap();
    let running = store.insert_download(&new("running", NOW)).unwrap();
    let done = store.insert_download(&new("done", NOW)).unwrap();
    let cancelled = store.insert_download(&new("cancelled", NOW)).unwrap();
    store
        .finish_download(done, DownloadState::Complete, None, NOW + 1)
        .unwrap();
    store
        .finish_download(cancelled, DownloadState::Cancelled, None, NOW + 2)
        .unwrap();

    assert_eq!(
        store
            .mark_unfinished_downloads_interrupted(NOW + 100)
            .unwrap(),
        1
    );
    let record = find(&store, running);
    assert_eq!(
        (record.state, record.ended_at_ms),
        (DownloadState::Interrupted, Some(NOW + 100))
    );
    let record = find(&store, done);
    assert_eq!(
        (record.state, record.ended_at_ms),
        (DownloadState::Complete, Some(NOW + 1))
    );
    let record = find(&store, cancelled);
    assert_eq!(
        (record.state, record.ended_at_ms),
        (DownloadState::Cancelled, Some(NOW + 2))
    );
    assert_eq!(
        store
            .mark_unfinished_downloads_interrupted(NOW + 200)
            .unwrap(),
        0
    );
}

#[test]
fn removing_and_clearing() {
    let mut store = Store::open_in_memory().unwrap();
    let running = store.insert_download(&new("running", NOW)).unwrap();
    let done = store.insert_download(&new("done", NOW)).unwrap();
    let failed = store.insert_download(&new("failed", NOW)).unwrap();
    let gone = store.insert_download(&new("gone", NOW)).unwrap();
    store
        .finish_download(done, DownloadState::Complete, None, NOW)
        .unwrap();
    store
        .finish_download(failed, DownloadState::Interrupted, Some("x"), NOW)
        .unwrap();

    assert!(store.remove_download(gone).unwrap());
    assert!(!store.remove_download(gone).unwrap());
    assert_eq!(store.clear_finished_downloads().unwrap(), 2);
    let ids: Vec<DownloadRecordId> = store.downloads(10).unwrap().iter().map(|d| d.id).collect();
    assert_eq!(ids, [running]);
}
