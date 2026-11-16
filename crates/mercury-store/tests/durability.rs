mod common;

use common::{NOW, TempDir};
use mercury_store::{BookmarkId, BookmarkRoot, Store, StoreError, VisitTransition};

fn layout(store: &Store, folder: BookmarkId) -> Vec<(String, u32)> {
    store
        .bookmark_children(folder)
        .unwrap()
        .into_iter()
        .map(|n| (n.title, n.position))
        .collect()
}

#[test]
fn a_move_that_fails_half_way_leaves_the_database_unchanged() {
    let dir = TempDir::new("failed-move");
    let (moved, toolbar, other) = {
        let mut store = Store::open(&dir.db()).unwrap();
        let toolbar = store.bookmark_root(BookmarkRoot::Toolbar).unwrap();
        let other = store.bookmark_root(BookmarkRoot::Other).unwrap();
        store
            .add_bookmark(toolbar, None, "A", "https://a.example/", "g-a", NOW)
            .unwrap();
        let moved = store
            .add_bookmark(toolbar, None, "Boom", "https://b.example/", "g-b", NOW)
            .unwrap();
        store
            .add_bookmark(toolbar, None, "C", "https://c.example/", "g-c", NOW)
            .unwrap();
        store
            .add_bookmark(other, None, "X", "https://x.example/", "g-x", NOW)
            .unwrap();
        (moved, toolbar, other)
    };

    // A trigger that aborts when the moved node itself is re-parented, which happens after the
    // sibling positions in both folders have already been shifted.
    {
        let conn = rusqlite::Connection::open(dir.db()).unwrap();
        conn.execute_batch(
            "CREATE TRIGGER fail_move BEFORE UPDATE OF parent_id ON bookmarks
             WHEN NEW.title = 'Boom'
             BEGIN SELECT RAISE(ABORT, 'forced failure'); END;",
        )
        .unwrap();
    }

    let mut store = Store::open(&dir.db()).unwrap();
    let before = (
        layout(&store, toolbar),
        layout(&store, other),
        store.bookmark(moved).unwrap(),
    );
    let result = store.move_bookmark(moved, other, Some(0), NOW + 1);
    match result {
        Err(StoreError::Sqlite(e)) => assert!(e.to_string().contains("forced failure"), "{e}"),
        other => panic!("the trigger must make the move fail, got {other:?}"),
    }
    let after = (
        layout(&store, toolbar),
        layout(&store, other),
        store.bookmark(moved).unwrap(),
    );
    assert_eq!(before, after);
    assert_eq!(
        after.0,
        [
            ("A".to_string(), 0),
            ("Boom".to_string(), 1),
            ("C".to_string(), 2)
        ]
    );
}

#[test]
fn a_visit_that_fails_half_way_leaves_the_database_unchanged() {
    let dir = TempDir::new("failed-visit");
    {
        let mut store = Store::open(&dir.db()).unwrap();
        store
            .record_visit("https://a.example/", Some("A"), VisitTransition::Link, NOW)
            .unwrap();
    }
    {
        let conn = rusqlite::Connection::open(dir.db()).unwrap();
        conn.execute_batch(
            "CREATE TRIGGER fail_visit BEFORE UPDATE OF visit_count ON places
             BEGIN SELECT RAISE(ABORT, 'forced failure'); END;",
        )
        .unwrap();
    }
    let mut store = Store::open(&dir.db()).unwrap();
    match store.record_visit(
        "https://a.example/",
        Some("New title"),
        VisitTransition::Link,
        NOW + 1,
    ) {
        Err(StoreError::Sqlite(e)) => assert!(e.to_string().contains("forced failure"), "{e}"),
        other => panic!("the trigger must make the visit fail, got {other:?}"),
    }
    let entries = store.history(None, None, 10).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].title.as_deref(), Some("A"));
}

#[test]
fn a_passive_checkpoint_moves_the_log_into_the_database_file() {
    let dir = common::TempDir::new("checkpoint");
    let mut store = mercury_store::Store::open(&dir.db()).unwrap();
    store
        .record_visit(
            "https://a.example/",
            None,
            mercury_store::VisitTransition::Link,
            common::NOW,
        )
        .unwrap();

    let (log, checkpointed) = store.checkpoint().unwrap();

    assert!(log > 0, "the write is in the log");
    assert_eq!(checkpointed, log);
}
