//! The full-text mirrors (`places_fts`, `bookmarks_fts`) follow every change to their tables.

mod common;

use common::{DAY_MS, Fixture, NOW};
use mercury_store::{
    BookmarkImport, BookmarkRoot, ClearRange, Store, VisitImport, VisitTransition,
};

const A: &str = "https://a.example/";

fn found(store: &Store, query: &str) -> usize {
    store.history(Some(query), None, 100).unwrap().len()
}

fn integrity_check(f: &Fixture) {
    for table in ["places_fts", "bookmarks_fts"] {
        // With rank 1 the check also compares the index with the content table. The table name
        // cannot be bound, so the two statements are spelled out.
        let sql = match table {
            "places_fts" => {
                "INSERT INTO places_fts(places_fts, rank) VALUES ('integrity-check', 1)"
            }
            _ => "INSERT INTO bookmarks_fts(bookmarks_fts, rank) VALUES ('integrity-check', 1)",
        };
        f.raw
            .execute(sql, [])
            .unwrap_or_else(|e| panic!("{table} integrity-check failed: {e}"));
    }
}

#[test]
fn a_changed_title_is_found_by_its_new_words_only() {
    let mut store = Store::open_in_memory().unwrap();
    store
        .record_visit(A, Some("Original heading"), VisitTransition::Link, NOW)
        .unwrap();
    assert_eq!(found(&store, "original"), 1);
    store
        .record_visit(
            A,
            Some("Replacement heading"),
            VisitTransition::Link,
            NOW + 1,
        )
        .unwrap();
    assert_eq!(found(&store, "original"), 0);
    assert_eq!(found(&store, "replacement"), 2);
    assert!(store.set_page_title(A, "Third version").unwrap());
    assert_eq!(found(&store, "replacement"), 0);
    assert_eq!(found(&store, "third"), 2);
}

#[test]
fn a_title_set_later_is_found() {
    let mut store = Store::open_in_memory().unwrap();
    store
        .record_visit(A, None, VisitTransition::Link, NOW)
        .unwrap();
    assert_eq!(found(&store, "arrived"), 0);
    assert!(store.set_page_title(A, "Arrived late").unwrap());
    assert_eq!(found(&store, "arrived"), 1);
}

#[test]
fn the_mirrors_pass_integrity_check_after_mixed_changes() {
    let mut f = Fixture::new("fts-integrity");
    let toolbar = f.store.bookmark_root(BookmarkRoot::Toolbar).unwrap();
    let other = f.store.bookmark_root(BookmarkRoot::Other).unwrap();
    for i in 0..20 {
        let url = format!("https://site{i}.example/page");
        f.store
            .record_visit(
                &url,
                Some(&format!("Site {i}")),
                VisitTransition::Link,
                NOW - i * DAY_MS,
            )
            .unwrap();
    }
    f.store
        .set_page_title("https://site3.example/page", "Renamed")
        .unwrap();
    f.store
        .record_visit(
            "https://site4.example/page",
            Some("Retitled"),
            VisitTransition::Typed,
            NOW,
        )
        .unwrap();
    let entries = f.store.history(None, None, 5).unwrap();
    f.store.delete_visit(entries[0].visit_id, NOW).unwrap();
    f.store
        .delete_page("https://site5.example/page", NOW)
        .unwrap();
    f.store
        .clear_history(
            ClearRange::Since {
                from_ms: NOW - 3 * DAY_MS,
            },
            NOW,
        )
        .unwrap();

    let folder = f
        .store
        .add_bookmark_folder(toolbar, None, "Folder", "f", NOW)
        .unwrap();
    let a = f
        .store
        .add_bookmark(
            folder,
            None,
            "Alpha",
            "https://site10.example/page",
            "a",
            NOW,
        )
        .unwrap();
    let b = f
        .store
        .add_bookmark(toolbar, None, "Beta", "https://never.example/", "b", NOW)
        .unwrap();
    f.store
        .add_bookmark_separator(toolbar, None, "s", NOW)
        .unwrap();
    f.store.rename_bookmark(a, "Alpha renamed", NOW).unwrap();
    f.store.rename_bookmark(toolbar, "Bar", NOW).unwrap();
    f.store
        .set_bookmark_url(b, "https://site11.example/page", NOW)
        .unwrap();
    f.store.move_bookmark(a, other, None, NOW).unwrap();
    f.store
        .import_visits(
            &[VisitImport {
                url: "https://imported.example/",
                title: Some("Imported"),
                visited_at_ms: NOW,
                transition: VisitTransition::Link,
            }],
            NOW,
        )
        .unwrap();
    f.store
        .import_bookmarks(
            other,
            &[BookmarkImport::Folder {
                title: "Imported folder".into(),
                guid: "if".into(),
                added_at_ms: None,
                children: vec![BookmarkImport::Bookmark {
                    title: "Imported bookmark".into(),
                    url: "https://imported.example/".into(),
                    guid: "ib".into(),
                    added_at_ms: None,
                }],
            }],
            NOW,
        )
        .unwrap();
    f.store.remove_bookmark(folder, NOW).unwrap();
    f.store.clear_history(ClearRange::All, NOW).unwrap();

    integrity_check(&f);
    assert_eq!(f.store.suggest("renamed", 10, NOW).unwrap().len(), 1);
    assert_eq!(
        f.store.suggest("imported bookmark", 10, NOW).unwrap().len(),
        1
    );
}
