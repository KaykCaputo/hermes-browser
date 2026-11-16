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

// ---- state transitions ----

#[test]
fn progress_and_finish_apply_only_in_progress() {
    let mut store = Store::open_in_memory().unwrap();
    let id = store.insert_download(&new("d-1", NOW)).unwrap();
    store.update_download_progress(id, 10, Some(100)).unwrap();
    store
        .finish_download(id, DownloadState::Complete, None, NOW + 1)
        .unwrap();
    let done = find(&store, id);

    // A late progress report or cancel must not rewrite the completed record.
    assert!(matches!(
        store.update_download_progress(id, 50, Some(200)),
        Err(StoreError::InvalidState(_))
    ));
    assert!(matches!(
        store.finish_download(id, DownloadState::Cancelled, Some("late"), NOW + 2),
        Err(StoreError::InvalidState(_))
    ));
    assert_eq!(find(&store, id), done);

    let cancelled = store.insert_download(&new("d-2", NOW)).unwrap();
    store
        .finish_download(cancelled, DownloadState::Cancelled, None, NOW + 1)
        .unwrap();
    assert!(matches!(
        store.finish_download(cancelled, DownloadState::Complete, None, NOW + 2),
        Err(StoreError::InvalidState(_))
    ));
    assert_eq!(find(&store, cancelled).state, DownloadState::Cancelled);
}

#[test]
fn an_interrupted_download_can_be_resumed() {
    let mut store = Store::open_in_memory().unwrap();
    let id = store.insert_download(&new("d-1", NOW)).unwrap();
    store.update_download_progress(id, 40, Some(100)).unwrap();
    store
        .finish_download(id, DownloadState::Interrupted, Some("reset"), NOW + 1)
        .unwrap();

    store.resume_download(id).unwrap();
    let record = find(&store, id);
    assert_eq!(record.state, DownloadState::InProgress);
    assert_eq!((record.ended_at_ms, record.error), (None, None));
    assert_eq!((record.received_bytes, record.total_bytes), (40, Some(100)));
    assert_eq!(record.started_at_ms, NOW);

    store.update_download_progress(id, 100, None).unwrap();
    store
        .finish_download(id, DownloadState::Complete, None, NOW + 3)
        .unwrap();
    assert_eq!(find(&store, id).state, DownloadState::Complete);
}

#[test]
fn only_an_interrupted_download_can_be_resumed() {
    let mut store = Store::open_in_memory().unwrap();
    let running = store.insert_download(&new("running", NOW)).unwrap();
    let complete = store.insert_download(&new("complete", NOW)).unwrap();
    let cancelled = store.insert_download(&new("cancelled", NOW)).unwrap();
    store
        .finish_download(complete, DownloadState::Complete, None, NOW)
        .unwrap();
    store
        .finish_download(cancelled, DownloadState::Cancelled, None, NOW)
        .unwrap();
    for id in [running, complete, cancelled] {
        let before = find(&store, id);
        assert!(matches!(
            store.resume_download(id),
            Err(StoreError::InvalidState(_))
        ));
        assert_eq!(find(&store, id), before);
    }
    assert!(matches!(
        store.resume_download(DownloadRecordId(999)),
        Err(StoreError::NotFound)
    ));
}

#[test]
fn equal_start_times_list_newest_record_first() {
    let mut store = Store::open_in_memory().unwrap();
    let ids: Vec<DownloadRecordId> = (0..5)
        .map(|i| store.insert_download(&new(&format!("d-{i}"), NOW)).unwrap())
        .collect();
    let listed: Vec<DownloadRecordId> = store.downloads(10).unwrap().iter().map(|d| d.id).collect();
    let mut expected = ids;
    expected.reverse();
    assert_eq!(listed, expected);
}

// ---- bounded text ----

#[test]
fn web_supplied_text_is_bounded() {
    use mercury_store::{
        MAX_ERROR_CHARS, MAX_MIME_TYPE_CHARS, MAX_PATH_BYTES, MAX_URL_BYTES, MAX_VALIDATOR_BYTES,
    };
    let mut store = Store::open_in_memory().unwrap();
    let long_url = format!("https://example.com/{}", "a".repeat(MAX_URL_BYTES));
    let long_path = format!("/tmp/{}", "p".repeat(MAX_PATH_BYTES));
    let mime = "é".repeat(MAX_MIME_TYPE_CHARS + 10);
    let download = NewDownload {
        mime_type: Some(&mime),
        ..new("d-1", NOW)
    };
    let id = store.insert_download(&download).unwrap();
    assert_eq!(
        find(&store, id).mime_type.unwrap().chars().count(),
        MAX_MIME_TYPE_CHARS
    );

    for bad in [
        NewDownload {
            url: &long_url,
            ..new("bad-1", NOW)
        },
        NewDownload {
            referrer: Some(&long_url),
            ..new("bad-2", NOW)
        },
        NewDownload {
            target_path: &long_path,
            ..new("bad-3", NOW)
        },
        NewDownload {
            temp_path: Some(&long_path),
            ..new("bad-4", NOW)
        },
    ] {
        assert!(matches!(
            store.insert_download(&bad),
            Err(StoreError::InvalidArgument(_))
        ));
    }
    assert!(matches!(
        store.set_download_validators(id, Some(&long_url), None, None),
        Err(StoreError::InvalidArgument(_))
    ));

    // A validator too long to store is dropped (a cut one would never match).
    let etag = "x".repeat(MAX_VALIDATOR_BYTES + 1);
    store
        .set_download_validators(id, None, Some(&etag), Some("Sun, 15 Nov 2026 00:00:00 GMT"))
        .unwrap();
    let record = find(&store, id);
    assert_eq!(record.etag, None);
    assert_eq!(
        record.last_modified.as_deref(),
        Some("Sun, 15 Nov 2026 00:00:00 GMT")
    );

    let error = "ü".repeat(MAX_ERROR_CHARS * 2);
    store
        .finish_download(id, DownloadState::Interrupted, Some(&error), NOW)
        .unwrap();
    assert_eq!(
        find(&store, id).error.unwrap().chars().count(),
        MAX_ERROR_CHARS
    );
    assert_eq!(store.downloads(10).unwrap().len(), 1);
}

#[test]
fn an_interrupted_download_can_be_cancelled_but_not_completed() {
    let mut store = Store::open_in_memory().unwrap();
    let id = store.insert_download(&new("d-1", NOW)).unwrap();
    store
        .finish_download(id, DownloadState::Interrupted, Some("reset"), NOW + 1)
        .unwrap();
    for state in [DownloadState::Complete, DownloadState::Interrupted] {
        let before = find(&store, id);
        assert!(matches!(
            store.finish_download(id, state, None, NOW + 2),
            Err(StoreError::InvalidState(_))
        ));
        assert_eq!(find(&store, id), before);
    }
    store
        .finish_download(id, DownloadState::Cancelled, None, NOW + 3)
        .unwrap();
    let record = find(&store, id);
    assert_eq!(record.state, DownloadState::Cancelled);
    assert_eq!((record.ended_at_ms, record.error), (Some(NOW + 3), None));
    assert!(matches!(
        store.resume_download(id),
        Err(StoreError::InvalidState(_))
    ));
}

#[test]
fn the_finished_file_can_be_recorded_under_another_name() {
    use mercury_store::MAX_PATH_BYTES;
    let mut store = Store::open_in_memory().unwrap();
    let id = store.insert_download(&new("d-1", NOW)).unwrap();
    store
        .set_download_target_path(id, "/Users/me/Downloads/file (1).zip")
        .unwrap();
    assert_eq!(
        find(&store, id).target_path,
        "/Users/me/Downloads/file (1).zip"
    );

    // Too long a path is refused and changes nothing; an unknown record is not found.
    let long_path = format!("/tmp/{}", "p".repeat(MAX_PATH_BYTES));
    assert!(matches!(
        store.set_download_target_path(id, &long_path),
        Err(StoreError::InvalidArgument(_))
    ));
    assert_eq!(
        find(&store, id).target_path,
        "/Users/me/Downloads/file (1).zip"
    );
    assert!(matches!(
        store.set_download_target_path(DownloadRecordId(999), "/tmp/x"),
        Err(StoreError::NotFound)
    ));
}
