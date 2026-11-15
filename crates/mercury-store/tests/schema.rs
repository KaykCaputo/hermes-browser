mod common;

use common::{NOW, TempDir};
use mercury_store::{BookmarkRoot, Store, StoreError, VisitTransition};

fn names(conn: &rusqlite::Connection, kind: &str) -> Vec<String> {
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = ?1 ORDER BY name")
        .unwrap();
    stmt.query_map([kind], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

#[test]
fn in_memory_store_is_at_schema_version_1() {
    let store = Store::open_in_memory().unwrap();
    assert_eq!(store.schema_version().unwrap(), 1);
}

#[test]
fn opening_creates_tables_indexes_triggers_and_roots() {
    let dir = TempDir::new("creates");
    drop(Store::open(&dir.db()).unwrap());

    let conn = rusqlite::Connection::open(dir.db()).unwrap();
    let tables = names(&conn, "table");
    for table in [
        "places",
        "visits",
        "bookmarks",
        "downloads",
        "places_fts",
        "bookmarks_fts",
    ] {
        assert!(tables.iter().any(|t| t == table), "missing table {table}");
    }
    let indexes = names(&conn, "index");
    for index in [
        "visits_by_place",
        "visits_by_date",
        "bookmarks_by_parent",
        "bookmarks_by_place",
        "places_by_url_key",
        "places_by_frecency",
    ] {
        assert!(indexes.iter().any(|i| i == index), "missing index {index}");
    }
    assert_eq!(
        names(&conn, "trigger"),
        [
            "bookmarks_fts_ad",
            "bookmarks_fts_ai",
            "bookmarks_fts_au",
            "places_fts_ad",
            "places_fts_ai",
            "places_fts_au",
        ]
    );

    let roots: Vec<(String, String, i64, Option<i64>)> = conn
        .prepare("SELECT guid, title, kind, parent_id FROM bookmarks ORDER BY position")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        roots,
        [
            ("toolbar_____".into(), "Bookmarks Bar".into(), 2, None),
            ("menu________".into(), "Other Bookmarks".into(), 2, None),
        ]
    );

    let journal: String = conn
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(journal, "wal");
    let version: u32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 1);
}

#[test]
fn reopening_keeps_data_and_does_not_duplicate_roots() {
    let dir = TempDir::new("reopen");
    {
        let mut store = Store::open(&dir.db()).unwrap();
        store
            .record_visit(
                "https://example.com/",
                Some("Example"),
                VisitTransition::Link,
                NOW,
            )
            .unwrap();
        let toolbar = store.bookmark_root(BookmarkRoot::Toolbar).unwrap();
        store
            .add_bookmark(
                toolbar,
                None,
                "Example",
                "https://example.com/",
                "guid-1",
                NOW,
            )
            .unwrap();
    }
    let store = Store::open(&dir.db()).unwrap();
    assert_eq!(store.schema_version().unwrap(), 1);
    assert_eq!(store.history(None, None, 10).unwrap().len(), 1);
    let toolbar = store.bookmark_root(BookmarkRoot::Toolbar).unwrap();
    assert_eq!(store.bookmark_children(toolbar).unwrap().len(), 1);
    drop(store);

    let conn = rusqlite::Connection::open(dir.db()).unwrap();
    let folders: i64 = conn
        .query_row(
            "SELECT count(*) FROM bookmarks WHERE parent_id IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(folders, 2);
}

#[test]
fn a_newer_schema_is_refused_and_left_unmodified() {
    let dir = TempDir::new("newer");
    {
        let conn = rusqlite::Connection::open(dir.db()).unwrap();
        conn.execute_batch("CREATE TABLE future (x INTEGER); PRAGMA user_version = 7;")
            .unwrap();
    }
    let before = std::fs::read(dir.db()).unwrap();

    match Store::open(&dir.db()) {
        Err(StoreError::UnsupportedSchemaVersion { found, supported }) => {
            assert_eq!((found, supported), (7, 1));
        }
        Err(other) => panic!("unexpected error {other}"),
        Ok(_) => panic!("a newer schema must be refused"),
    }
    assert_eq!(std::fs::read(dir.db()).unwrap(), before);
    assert!(!dir.path().join("places.sqlite-wal").exists());
}
