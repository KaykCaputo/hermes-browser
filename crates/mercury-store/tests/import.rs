mod common;

use std::time::Instant;

use common::{DAY_MS, Fixture, NOW};
use mercury_store::{
    BOOKMARK_IMPORT_MAX_DEPTH, BookmarkImport, BookmarkKind, BookmarkRoot, ImportRefusal, Store,
    StoreError, VisitImport, VisitTransition,
};

// ---- visits ----

#[test]
fn twenty_thousand_visits_import_quickly_with_correct_counts() {
    let mut f = Fixture::new("import-visits");
    let urls: Vec<String> = (0..2000)
        .map(|i| format!("https://site{i}.example/"))
        .collect();
    let titles: Vec<String> = (0..2000).map(|i| format!("Site number {i}")).collect();
    // Page i gets 10 visits, one per day ending i % 30 days before NOW; the first one typed.
    let visits: Vec<VisitImport<'_>> = (0..20_000)
        .map(|n| {
            let page = n % 2000;
            let k = (n / 2000) as i64;
            VisitImport {
                url: &urls[page],
                title: Some(&titles[page]),
                visited_at_ms: NOW - ((page % 30) as i64 + k) * DAY_MS,
                transition: if k == 0 {
                    VisitTransition::Typed
                } else {
                    VisitTransition::Link
                },
            }
        })
        .collect();

    let start = Instant::now();
    assert_eq!(f.store.import_visits(&visits, NOW).unwrap(), 20_000);
    let elapsed = start.elapsed();
    println!(
        "imported 20,000 visits in {:.1} ms",
        elapsed.as_secs_f64() * 1000.0
    );
    if !cfg!(debug_assertions) {
        assert!(elapsed.as_millis() < 500, "took {elapsed:?}");
    }

    assert_eq!(f.count("SELECT count(*) FROM visits"), 20_000);
    assert_eq!(f.count("SELECT count(*) FROM places"), 2000);
    let place = f.place("https://site7.example/").unwrap();
    assert_eq!(place.title.as_deref(), Some("Site number 7"));
    assert_eq!((place.visit_count, place.typed_count), (10, 1));
    assert_eq!(place.last_visit_at, Some(NOW - 7 * DAY_MS));
    // Scored once, as of NOW: typed 7 days ago 70 × 20 = 1400, links 8-16 days ago:
    // 8-14 → 7 × 70 = 490, 15-16 → 2 × 50 = 100; ceil(10 × 1990 / 10) = 1990.
    assert_eq!(place.frecency, 1990);
    // Visits are searchable: "site1999" is a whole URL word only for that page.
    assert_eq!(
        f.store.history(Some("site1999"), None, 100).unwrap().len(),
        10
    );
}

#[test]
fn importing_visits_to_known_pages_adds_to_them() {
    let mut f = Fixture::new("import-known");
    f.store
        .record_visit("https://a.example/", Some("A"), VisitTransition::Link, NOW)
        .unwrap();
    let visits = [
        VisitImport {
            url: "https://a.example/",
            title: None,
            visited_at_ms: NOW - DAY_MS,
            transition: VisitTransition::Typed,
        },
        VisitImport {
            url: "https://a.example/",
            title: Some("Older title"),
            visited_at_ms: NOW - 2 * DAY_MS,
            transition: VisitTransition::Link,
        },
    ];
    assert_eq!(f.store.import_visits(&visits, NOW).unwrap(), 2);
    let place = f.place("https://a.example/").unwrap();
    assert_eq!((place.visit_count, place.typed_count), (3, 1));
    assert_eq!(place.last_visit_at, Some(NOW));
    // The stored title came from a newer visit than any titled visit in the import.
    assert_eq!(place.title.as_deref(), Some("A"));
    assert_eq!(f.store.import_visits(&[], NOW).unwrap(), 0);
}

// ---- titles: the most recent titled visit wins, whatever the slice order ----

fn titled<'a>(url: &'a str, title: Option<&'a str>, days_ago: i64) -> VisitImport<'a> {
    VisitImport {
        url,
        title,
        visited_at_ms: NOW - days_ago * DAY_MS,
        transition: VisitTransition::Link,
    }
}

fn title_after(visits: &[VisitImport<'_>]) -> Option<String> {
    let mut store = Store::open_in_memory().unwrap();
    store.import_visits(visits, NOW).unwrap();
    store
        .history(None, None, 1)
        .unwrap()
        .into_iter()
        .next()
        .and_then(|e| e.title)
}

#[test]
fn the_newest_titled_visit_gives_the_title_in_any_order() {
    const U: &str = "https://t.example/";
    let newest_first = [
        titled(U, None, 0),
        titled(U, Some("Newest"), 1),
        titled(U, Some("Middle"), 5),
        titled(U, Some("Oldest"), 9),
    ];
    let mut oldest_first = newest_first;
    oldest_first.reverse();
    let shuffled = [
        newest_first[2],
        newest_first[0],
        newest_first[3],
        newest_first[1],
    ];
    for (name, visits) in [
        ("newest first", &newest_first),
        ("oldest first", &oldest_first),
        ("shuffled", &shuffled),
    ] {
        assert_eq!(title_after(visits).as_deref(), Some("Newest"), "{name}");
    }
}

#[test]
fn an_import_older_than_the_stored_title_leaves_it() {
    let mut f = Fixture::new("import-older-title");
    f.store
        .record_visit(
            "https://t.example/",
            Some("Seen today"),
            VisitTransition::Link,
            NOW,
        )
        .unwrap();
    f.store
        .import_visits(
            &[
                titled("https://t.example/", Some("Last year"), 365),
                titled("https://t.example/", Some("Last month"), 30),
            ],
            NOW,
        )
        .unwrap();
    assert_eq!(
        f.place("https://t.example/").unwrap().title.as_deref(),
        Some("Seen today")
    );
    // A newer titled visit in an import does replace it.
    f.store
        .import_visits(&[titled("https://t.example/", Some("Tomorrow"), -1)], NOW)
        .unwrap();
    assert_eq!(
        f.place("https://t.example/").unwrap().title.as_deref(),
        Some("Tomorrow")
    );
}

#[test]
fn a_failed_visit_import_changes_nothing() {
    let mut f = Fixture::new("import-visits-rollback");
    f.store
        .record_visit("https://a.example/", Some("A"), VisitTransition::Link, NOW)
        .unwrap();
    f.raw
        .execute_batch(
            "CREATE TRIGGER fail_import BEFORE INSERT ON visits WHEN NEW.visited_at = 999
             BEGIN SELECT RAISE(ABORT, 'forced failure'); END;",
        )
        .unwrap();
    let visits = [
        VisitImport {
            url: "https://a.example/",
            title: Some("Changed"),
            visited_at_ms: 1,
            transition: VisitTransition::Link,
        },
        VisitImport {
            url: "https://b.example/",
            title: None,
            visited_at_ms: 2,
            transition: VisitTransition::Link,
        },
        VisitImport {
            url: "https://c.example/",
            title: None,
            visited_at_ms: 999,
            transition: VisitTransition::Link,
        },
    ];
    assert!(matches!(
        f.store.import_visits(&visits, NOW),
        Err(StoreError::Sqlite(_))
    ));
    assert_eq!(f.count("SELECT count(*) FROM visits"), 1);
    assert_eq!(f.count("SELECT count(*) FROM places"), 1);
    assert_eq!(
        f.place("https://a.example/").unwrap().title.as_deref(),
        Some("A")
    );
}

// ---- bookmarks ----

fn bookmark(title: &str, url: &str, guid: &str) -> BookmarkImport {
    BookmarkImport::Bookmark {
        title: title.into(),
        url: url.into(),
        guid: guid.into(),
        added_at_ms: None,
    }
}

fn tree() -> Vec<BookmarkImport> {
    vec![
        bookmark("Rust", "https://www.rust-lang.org/", "i-rust"),
        BookmarkImport::Folder {
            title: "Work".into(),
            guid: "i-work".into(),
            added_at_ms: Some(NOW - 100 * DAY_MS),
            children: vec![
                BookmarkImport::Bookmark {
                    title: "Payroll".into(),
                    url: "https://intranet.acme.example/ss/x7".into(),
                    guid: "i-pay".into(),
                    added_at_ms: Some(NOW - 50 * DAY_MS),
                },
                BookmarkImport::Separator {
                    guid: "i-sep".into(),
                },
                BookmarkImport::Folder {
                    title: "Empty".into(),
                    guid: "i-empty".into(),
                    added_at_ms: None,
                    children: vec![],
                },
                BookmarkImport::Folder {
                    title: "Deeper".into(),
                    guid: "i-deeper".into(),
                    added_at_ms: None,
                    children: vec![bookmark("Servo", "https://servo.org/", "i-servo")],
                },
            ],
        },
        BookmarkImport::Separator {
            guid: "i-sep2".into(),
        },
    ]
}

fn shape(store: &Store, folder: mercury_store::BookmarkId) -> Vec<String> {
    let mut out = Vec::new();
    for (index, node) in store
        .bookmark_children(folder)
        .unwrap()
        .into_iter()
        .enumerate()
    {
        assert_eq!(node.position as usize, index);
        match node.kind {
            BookmarkKind::Bookmark { url } => out.push(format!("{} <{url}>", node.title)),
            BookmarkKind::Separator => out.push("---".into()),
            BookmarkKind::Folder => {
                out.push(format!("{}/", node.title));
                out.extend(shape(store, node.id).into_iter().map(|s| format!("  {s}")));
            }
        }
    }
    out
}

#[test]
fn a_tree_is_appended_with_dense_positions_and_dates() {
    let mut store = Store::open_in_memory().unwrap();
    let toolbar = store.bookmark_root(BookmarkRoot::Toolbar).unwrap();
    store
        .add_bookmark(
            toolbar,
            None,
            "Existing",
            "https://existing.example/",
            "e-1",
            NOW,
        )
        .unwrap();
    assert_eq!(store.import_bookmarks(toolbar, &tree(), NOW).unwrap(), 8);
    assert_eq!(
        shape(&store, toolbar),
        [
            "Existing <https://existing.example/>",
            "Rust <https://www.rust-lang.org/>",
            "Work/",
            "  Payroll <https://intranet.acme.example/ss/x7>",
            "  ---",
            "  Empty/",
            "  Deeper/",
            "    Servo <https://servo.org/>",
            "---",
        ]
    );
    let children = store.bookmark_children(toolbar).unwrap();
    let work = &children[2];
    assert_eq!(work.guid, "i-work");
    assert_eq!(
        (work.added_at_ms, work.modified_at_ms),
        (NOW - 100 * DAY_MS, NOW)
    );
    assert_eq!(children[1].added_at_ms, NOW);
    let payroll = &store.bookmark_children(work.id).unwrap()[0];
    assert_eq!(
        (payroll.added_at_ms, payroll.modified_at_ms),
        (NOW - 50 * DAY_MS, NOW)
    );
}

#[test]
fn imported_bookmarks_are_found_by_suggest_and_url() {
    let mut store = Store::open_in_memory().unwrap();
    let other = store.bookmark_root(BookmarkRoot::Other).unwrap();
    store.import_bookmarks(other, &tree(), NOW).unwrap();
    let hit = store.suggest("payroll", 10, NOW).unwrap();
    assert_eq!(hit.len(), 1);
    assert_eq!(hit[0].url, "https://intranet.acme.example/ss/x7");
    assert!(hit[0].is_bookmark);
    assert_eq!(hit[0].frecency, 140);
    assert!(
        store
            .bookmark_for_url("https://servo.org/")
            .unwrap()
            .is_some()
    );
}

#[test]
fn a_duplicate_guid_in_the_last_node_rolls_everything_back() {
    let mut f = Fixture::new("import-dup");
    let toolbar = f.store.bookmark_root(BookmarkRoot::Toolbar).unwrap();
    f.store
        .add_bookmark(
            toolbar,
            None,
            "Existing",
            "https://existing.example/",
            "taken",
            NOW,
        )
        .unwrap();
    let before_bookmarks = f.count("SELECT count(*) FROM bookmarks");
    let before_places = f.count("SELECT count(*) FROM places");

    let mut nodes = tree();
    nodes.push(bookmark("Last", "https://last.example/", "taken"));
    match f.store.import_bookmarks(toolbar, &nodes, NOW) {
        // The tree has 8 nodes, so the added one is entry 8 in pre-order.
        Err(StoreError::ImportEntryRefused {
            index: 8,
            reason: ImportRefusal::DuplicateGuid(guid),
        }) => assert_eq!(guid, "taken"),
        other => panic!("expected a refused entry, got {other:?}"),
    }
    // A GUID repeated within the import fails the same way.
    let twice = vec![
        bookmark("One", "https://one.example/", "same"),
        BookmarkImport::Folder {
            title: "F".into(),
            guid: "f".into(),
            added_at_ms: None,
            children: vec![BookmarkImport::Separator {
                guid: "same".into(),
            }],
        },
    ];
    assert!(matches!(
        f.store.import_bookmarks(toolbar, &twice, NOW),
        Err(StoreError::ImportEntryRefused {
            index: 2,
            reason: ImportRefusal::DuplicateGuid(_)
        })
    ));

    assert_eq!(f.count("SELECT count(*) FROM bookmarks"), before_bookmarks);
    assert_eq!(f.count("SELECT count(*) FROM places"), before_places);
    assert_eq!(
        shape(&f.store, toolbar),
        ["Existing <https://existing.example/>"]
    );
}

fn nested(depth: usize) -> Vec<BookmarkImport> {
    let mut nodes = vec![bookmark("Leaf", "https://leaf.example/", "leaf")];
    for level in (1..depth).rev() {
        nodes = vec![BookmarkImport::Folder {
            title: format!("Level {level}"),
            guid: format!("level-{level}"),
            added_at_ms: None,
            children: nodes,
        }];
    }
    nodes
}

#[test]
fn nesting_is_limited() {
    let mut store = Store::open_in_memory().unwrap();
    let other = store.bookmark_root(BookmarkRoot::Other).unwrap();
    assert!(matches!(
        store.import_bookmarks(other, &nested(BOOKMARK_IMPORT_MAX_DEPTH + 1), NOW),
        // MAX folders (entries 0 to MAX - 1), then the leaf one level too deep.
        Err(StoreError::ImportEntryRefused {
            index: BOOKMARK_IMPORT_MAX_DEPTH,
            reason: ImportRefusal::NestedTooDeeply
        })
    ));
    assert!(store.bookmark_children(other).unwrap().is_empty());
    assert_eq!(
        store
            .import_bookmarks(other, &nested(BOOKMARK_IMPORT_MAX_DEPTH), NOW)
            .unwrap(),
        BOOKMARK_IMPORT_MAX_DEPTH as u64
    );
}

#[test]
fn importing_under_a_non_folder_is_refused() {
    let mut store = Store::open_in_memory().unwrap();
    let toolbar = store.bookmark_root(BookmarkRoot::Toolbar).unwrap();
    let leaf = store
        .add_bookmark(toolbar, None, "A", "https://a.example/", "a", NOW)
        .unwrap();
    assert!(matches!(
        store.import_bookmarks(leaf, &tree(), NOW),
        Err(StoreError::NotAFolder)
    ));
    assert_eq!(store.import_bookmarks(toolbar, &[], NOW).unwrap(), 0);
}

#[test]
fn a_refused_visit_is_named_by_its_index() {
    let mut f = Fixture::new("import-visit-index");
    let long_url = format!(
        "https://example.com/{}",
        "a".repeat(mercury_store::MAX_URL_BYTES)
    );
    let visits = [
        titled("https://a.example/", Some("A"), 1),
        titled("https://b.example/", None, 2),
        titled(&long_url, None, 3),
    ];
    match f.store.import_visits(&visits, NOW) {
        Err(StoreError::ImportEntryRefused {
            index: 2,
            reason: ImportRefusal::UrlTooLong,
        }) => {}
        other => panic!("expected entry 2 refused, got {other:?}"),
    }
    assert_eq!(f.count("SELECT count(*) FROM places"), 0);
    let message = f.store.import_visits(&visits, NOW).unwrap_err().to_string();
    assert!(message.contains("entry 2"), "{message}");
}

// ---- several trees in one transaction ----

#[test]
fn several_trees_are_imported_together() {
    let mut store = Store::open_in_memory().unwrap();
    let toolbar = store.bookmark_root(BookmarkRoot::Toolbar).unwrap();
    let other = store.bookmark_root(BookmarkRoot::Other).unwrap();
    let bar = vec![bookmark("On the bar", "https://bar.example/", "s-bar")];
    let rest = tree();
    let added = store
        .import_bookmark_sets(&[(toolbar, &bar), (other, &rest)], NOW)
        .unwrap();
    assert_eq!(added, 9);
    assert_eq!(
        shape(&store, toolbar),
        ["On the bar <https://bar.example/>"]
    );
    assert_eq!(shape(&store, other).len(), 8);
    // Two sets under the same parent append in order.
    let more = vec![bookmark("More", "https://more.example/", "s-more")];
    store
        .import_bookmark_sets(&[(toolbar, &more), (toolbar, &more[..0])], NOW)
        .unwrap();
    assert_eq!(shape(&store, toolbar).len(), 2);
}

#[test]
fn a_failure_in_a_later_tree_rolls_back_the_earlier_ones() {
    let mut f = Fixture::new("import-sets-rollback");
    let toolbar = f.store.bookmark_root(BookmarkRoot::Toolbar).unwrap();
    let other = f.store.bookmark_root(BookmarkRoot::Other).unwrap();
    let before = f.count("SELECT count(*) FROM bookmarks");
    let bar = vec![bookmark("On the bar", "https://bar.example/", "s-bar")];
    let mut rest = tree();
    rest.push(bookmark("Clash", "https://clash.example/", "s-bar"));
    match f
        .store
        .import_bookmark_sets(&[(toolbar, &bar), (other, &rest)], NOW)
    {
        // Entry 0 is the toolbar bookmark; the other tree's 8 nodes are entries 1 to 8.
        Err(StoreError::ImportEntryRefused {
            index: 9,
            reason: ImportRefusal::DuplicateGuid(guid),
        }) => assert_eq!(guid, "s-bar"),
        other => panic!("expected entry 9 refused, got {other:?}"),
    }
    assert_eq!(f.count("SELECT count(*) FROM bookmarks"), before);
    assert_eq!(f.count("SELECT count(*) FROM places"), 0);
    assert!(f.store.bookmark_children(toolbar).unwrap().is_empty());
}
