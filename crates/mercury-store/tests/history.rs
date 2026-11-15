mod common;

use common::{DAY_MS, Fixture, NOW};
use mercury_store::{BookmarkRoot, ClearRange, HistoryCursor, Store, VisitId, VisitTransition};

const A: &str = "https://example.com/a";
const B: &str = "https://example.org/b";

// ---- visits ----

#[test]
fn first_visit_creates_the_place() {
    let mut f = Fixture::new("first-visit");
    f.store
        .record_visit(A, Some("Alpha"), VisitTransition::Link, NOW)
        .unwrap();
    let place = f.place(A).unwrap();
    assert_eq!(place.title.as_deref(), Some("Alpha"));
    assert_eq!(place.visit_count, 1);
    assert_eq!(place.typed_count, 0);
    assert_eq!(place.last_visit_at, Some(NOW));
}

#[test]
fn second_visit_adds_a_row_and_increments_the_count() {
    let mut f = Fixture::new("second-visit");
    let v1 = f
        .store
        .record_visit(A, None, VisitTransition::Link, NOW)
        .unwrap();
    let v2 = f
        .store
        .record_visit(A, None, VisitTransition::Link, NOW + 1000)
        .unwrap();
    assert_ne!(v1, v2);
    assert_eq!(f.place(A).unwrap().visit_count, 2);
    assert_eq!(f.count("SELECT count(*) FROM visits"), 2);
    assert_eq!(f.count("SELECT count(*) FROM places"), 1);
}

#[test]
fn typed_visits_increment_typed_count() {
    let mut f = Fixture::new("typed");
    f.store
        .record_visit(A, None, VisitTransition::Typed, NOW)
        .unwrap();
    f.store
        .record_visit(A, None, VisitTransition::Link, NOW + 1)
        .unwrap();
    f.store
        .record_visit(A, None, VisitTransition::Typed, NOW + 2)
        .unwrap();
    let place = f.place(A).unwrap();
    assert_eq!((place.visit_count, place.typed_count), (3, 2));
}

#[test]
fn last_visit_follows_the_newest_visit() {
    let mut f = Fixture::new("last-visit");
    f.store
        .record_visit(A, None, VisitTransition::Link, NOW)
        .unwrap();
    f.store
        .record_visit(A, None, VisitTransition::Link, NOW + DAY_MS)
        .unwrap();
    assert_eq!(f.place(A).unwrap().last_visit_at, Some(NOW + DAY_MS));
    // A visit recorded late with an older time does not move it back.
    f.store
        .record_visit(A, None, VisitTransition::Link, NOW - DAY_MS)
        .unwrap();
    assert_eq!(f.place(A).unwrap().last_visit_at, Some(NOW + DAY_MS));
}

#[test]
fn a_later_title_replaces_and_none_never_erases() {
    let mut f = Fixture::new("titles");
    f.store
        .record_visit(A, Some("Old"), VisitTransition::Link, NOW)
        .unwrap();
    f.store
        .record_visit(A, Some("New"), VisitTransition::Link, NOW + 1)
        .unwrap();
    assert_eq!(f.place(A).unwrap().title.as_deref(), Some("New"));
    f.store
        .record_visit(A, None, VisitTransition::Link, NOW + 2)
        .unwrap();
    assert_eq!(f.place(A).unwrap().title.as_deref(), Some("New"));
}

#[test]
fn a_title_from_an_older_visit_does_not_replace_a_newer_one() {
    let mut f = Fixture::new("older-title");
    f.store
        .record_visit(A, Some("Today"), VisitTransition::Link, NOW)
        .unwrap();
    f.store
        .record_visit(
            A,
            Some("Last week"),
            VisitTransition::Link,
            NOW - 7 * DAY_MS,
        )
        .unwrap();
    assert_eq!(f.place(A).unwrap().title.as_deref(), Some("Today"));
    f.store
        .record_visit(A, Some("Later today"), VisitTransition::Link, NOW + 1)
        .unwrap();
    assert_eq!(f.place(A).unwrap().title.as_deref(), Some("Later today"));
}

#[test]
fn set_page_title_updates_known_pages_only() {
    let mut store = Store::open_in_memory().unwrap();
    store
        .record_visit(A, None, VisitTransition::Link, NOW)
        .unwrap();
    assert!(store.set_page_title(A, "Alpha").unwrap());
    assert!(!store.set_page_title(B, "Beta").unwrap());
    let entries = store.history(None, None, 10).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].title.as_deref(), Some("Alpha"));
}

// ---- listing and search ----

fn sample() -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store
        .record_visit(
            "https://www.rust-lang.org/learn",
            Some("Learn Rust"),
            VisitTransition::Typed,
            NOW,
        )
        .unwrap();
    store
        .record_visit(
            "https://servo.org/",
            Some("Servo, the embeddable web engine"),
            VisitTransition::Link,
            NOW + 1000,
        )
        .unwrap();
    store
        .record_visit(
            "https://example.com/docs/getting-started",
            Some("Getting Started"),
            VisitTransition::Link,
            NOW + 2000,
        )
        .unwrap();
    store
        .record_visit(
            "https://servo.org/",
            None,
            VisitTransition::Reload,
            NOW + 3000,
        )
        .unwrap();
    store
}

fn urls(entries: &[mercury_store::HistoryEntry]) -> Vec<&str> {
    entries.iter().map(|e| e.url.as_str()).collect()
}

#[test]
fn history_is_newest_first_with_all_fields() {
    let store = sample();
    let entries = store.history(None, None, 10).unwrap();
    assert_eq!(
        urls(&entries),
        [
            "https://servo.org/",
            "https://example.com/docs/getting-started",
            "https://servo.org/",
            "https://www.rust-lang.org/learn",
        ]
    );
    assert_eq!(entries[0].visited_at_ms, NOW + 3000);
    assert_eq!(entries[0].transition, VisitTransition::Reload);
    assert_eq!(
        entries[0].title.as_deref(),
        Some("Servo, the embeddable web engine")
    );
    assert_eq!(entries[3].transition, VisitTransition::Typed);
    assert!(entries[0].visit_id > entries[3].visit_id);
}

#[test]
fn history_honours_the_cursor_and_limit() {
    let store = sample();
    let all = store.history(None, None, 10).unwrap();
    // Continue after the second entry (the visit at NOW + 2000).
    let entries = store.history(None, Some(all[1].cursor()), 10).unwrap();
    assert_eq!(
        urls(&entries),
        ["https://servo.org/", "https://www.rust-lang.org/learn"]
    );
    let cursor = HistoryCursor {
        visited_at_ms: NOW + 2000,
        visit_id: all[1].visit_id,
    };
    assert_eq!(all[1].cursor(), cursor);
    let entries = store.history(None, None, 2).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].visited_at_ms, NOW + 3000);
    assert!(store.history(None, None, 0).unwrap().is_empty());
}

#[test]
fn search_matches_title_words_case_insensitively() {
    let store = sample();
    let entries = store.history(Some("EMBEDDABLE web"), None, 10).unwrap();
    assert_eq!(urls(&entries), ["https://servo.org/", "https://servo.org/"]);
    let entries = store.history(Some("getting STARTED"), None, 10).unwrap();
    assert_eq!(urls(&entries), ["https://example.com/docs/getting-started"]);
}

#[test]
fn search_matches_parts_of_the_url() {
    let store = sample();
    assert_eq!(
        urls(&store.history(Some("rust-lang"), None, 10).unwrap()),
        ["https://www.rust-lang.org/learn"]
    );
    assert_eq!(
        urls(&store.history(Some("example.com"), None, 10).unwrap()),
        ["https://example.com/docs/getting-started"]
    );
    assert_eq!(
        urls(&store.history(Some("docs"), None, 10).unwrap()),
        ["https://example.com/docs/getting-started"]
    );
}

#[test]
fn only_the_last_word_is_a_prefix() {
    let store = sample();
    assert_eq!(
        urls(&store.history(Some("lear"), None, 10).unwrap()),
        ["https://www.rust-lang.org/learn"]
    );
    assert_eq!(
        urls(&store.history(Some("learn ru"), None, 10).unwrap()),
        ["https://www.rust-lang.org/learn"]
    );
    // "lear" is not the last word, so it must match a whole word.
    assert!(
        store
            .history(Some("lear rust"), None, 10)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn search_combines_with_before_and_limit() {
    let store = sample();
    let first = store.history(Some("servo"), None, 1).unwrap();
    assert_eq!(first[0].visited_at_ms, NOW + 3000);
    let entries = store
        .history(Some("servo"), Some(first[0].cursor()), 10)
        .unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].visited_at_ms, NOW + 1000);
    assert_eq!(store.history(Some("servo"), None, 1).unwrap().len(), 1);
}

#[test]
fn paging_returns_visits_that_share_a_millisecond_exactly_once() {
    let mut store = Store::open_in_memory().unwrap();
    let mut recorded = Vec::new();
    for i in 0..25 {
        let url = format!("https://example.com/{i}");
        recorded.push(
            store
                .record_visit(&url, None, VisitTransition::Link, NOW)
                .unwrap(),
        );
    }
    for query in [None, Some("example")] {
        let mut seen = Vec::new();
        let mut cursor = None;
        loop {
            let page = store.history(query, cursor, 10).unwrap();
            assert!(page.len() <= 10);
            let Some(last) = page.last() else { break };
            cursor = Some(last.cursor());
            seen.extend(page.iter().map(|e| e.visit_id));
        }
        let mut expected = recorded.clone();
        expected.reverse();
        assert_eq!(seen, expected, "query {query:?}");
    }
}

#[test]
fn an_empty_query_lists_everything() {
    let store = sample();
    assert_eq!(store.history(Some(""), None, 10).unwrap().len(), 4);
    assert_eq!(store.history(Some("   "), None, 10).unwrap().len(), 4);
}

#[test]
fn hostile_queries_do_not_fail() {
    let store = sample();
    let long = "a".repeat(2000);
    let long_words = "word ".repeat(400);
    let hostile = [
        "\"",
        "\"\"",
        "*",
        "(",
        ")",
        "NEAR",
        "NEAR(servo learn)",
        "-",
        "- servo",
        "servo -",
        "AND",
        "OR NOT",
        "title:servo",
        "{url}: servo",
        "^servo",
        "servo\"*",
        "'; DROP TABLE places; --",
        "\u{0}servo",
        "\u{301}",
        "servo \u{301}",
        "\u{1F600}",
        long.as_str(),
        long_words.as_str(),
    ];
    for query in hostile {
        store
            .history(Some(query), None, 10)
            .unwrap_or_else(|e| panic!("query {query:?} failed: {e}"));
    }
    assert_eq!(store.history(None, None, 10).unwrap().len(), 4);
    // Syntax characters are ignored, not interpreted.
    assert_eq!(
        store.history(Some("NEAR(servo"), None, 10).unwrap().len(),
        0
    );
    assert_eq!(store.history(Some("\"servo\""), None, 10).unwrap().len(), 2);
}

// ---- deletion ----

#[test]
fn deleting_a_visit_updates_count_and_last_visit() {
    let mut f = Fixture::new("delete-visit");
    f.store
        .record_visit(A, None, VisitTransition::Typed, NOW)
        .unwrap();
    let newest = f
        .store
        .record_visit(A, None, VisitTransition::Link, NOW + DAY_MS)
        .unwrap();
    assert!(f.store.delete_visit(newest, NOW + DAY_MS).unwrap());
    let place = f.place(A).unwrap();
    assert_eq!(place.visit_count, 1);
    assert_eq!(place.typed_count, 1);
    assert_eq!(place.last_visit_at, Some(NOW));
    assert!(!f.store.delete_visit(newest, NOW + DAY_MS).unwrap());
    assert!(!f.store.delete_visit(VisitId(9999), NOW).unwrap());
}

#[test]
fn deleting_the_last_visit_of_an_unbookmarked_page_removes_the_place() {
    let mut f = Fixture::new("delete-last");
    let visit = f
        .store
        .record_visit(A, Some("A"), VisitTransition::Link, NOW)
        .unwrap();
    assert!(f.store.delete_visit(visit, NOW).unwrap());
    assert_eq!(f.place(A), None);
    assert_eq!(
        f.count("SELECT count(*) FROM places_fts WHERE places_fts MATCH 'example'"),
        0
    );
}

#[test]
fn a_bookmarked_page_survives_with_zero_visits() {
    let mut f = Fixture::new("delete-bookmarked");
    let visit = f
        .store
        .record_visit(A, None, VisitTransition::Typed, NOW)
        .unwrap();
    let toolbar = f.store.bookmark_root(BookmarkRoot::Toolbar).unwrap();
    f.store
        .add_bookmark(toolbar, None, "A", A, "bm-a", NOW)
        .unwrap();
    assert!(f.store.delete_visit(visit, NOW).unwrap());
    let place = f.place(A).unwrap();
    assert_eq!(
        (place.visit_count, place.typed_count, place.last_visit_at),
        (0, 0, None)
    );
}

#[test]
fn delete_page_removes_all_its_visits() {
    let mut f = Fixture::new("delete-page");
    f.store
        .record_visit(A, None, VisitTransition::Link, NOW)
        .unwrap();
    f.store
        .record_visit(A, None, VisitTransition::Link, NOW + 1)
        .unwrap();
    f.store
        .record_visit(B, None, VisitTransition::Link, NOW + 2)
        .unwrap();
    assert!(f.store.delete_page(A, NOW + 3).unwrap());
    assert_eq!(f.place(A), None);
    assert_eq!(urls(&f.store.history(None, None, 10).unwrap()), [B]);
    assert!(!f.store.delete_page(A, NOW + 3).unwrap());
}

#[test]
fn delete_page_keeps_a_bookmarked_place() {
    let mut f = Fixture::new("delete-page-bookmarked");
    f.store
        .record_visit(A, None, VisitTransition::Link, NOW)
        .unwrap();
    let other = f.store.bookmark_root(BookmarkRoot::Other).unwrap();
    f.store
        .add_bookmark(other, None, "A", A, "bm-a", NOW)
        .unwrap();
    assert!(f.store.delete_page(A, NOW).unwrap());
    assert_eq!(f.place(A).unwrap().visit_count, 0);
    assert!(f.store.bookmark_for_url(A).unwrap().is_some());
}

#[test]
fn clear_history_since_removes_only_newer_visits() {
    let mut f = Fixture::new("clear-since");
    f.store
        .record_visit(A, None, VisitTransition::Typed, NOW - 2 * DAY_MS)
        .unwrap();
    f.store
        .record_visit(A, None, VisitTransition::Typed, NOW)
        .unwrap();
    f.store
        .record_visit(A, None, VisitTransition::Link, NOW + 1)
        .unwrap();
    f.store
        .record_visit(B, None, VisitTransition::Link, NOW + 2)
        .unwrap();

    assert_eq!(
        f.store
            .clear_history(ClearRange::Since { from_ms: NOW }, NOW + 3)
            .unwrap(),
        3
    );
    let place = f.place(A).unwrap();
    assert_eq!(
        (place.visit_count, place.typed_count, place.last_visit_at),
        (1, 1, Some(NOW - 2 * DAY_MS))
    );
    assert_eq!(f.place(B), None);
    assert_eq!(f.store.history(None, None, 10).unwrap().len(), 1);
}

#[test]
fn clear_history_all_removes_everything_but_bookmarked_places() {
    let mut f = Fixture::new("clear-all");
    f.store
        .record_visit(A, None, VisitTransition::Link, NOW)
        .unwrap();
    f.store
        .record_visit(B, None, VisitTransition::Link, NOW + 1)
        .unwrap();
    let toolbar = f.store.bookmark_root(BookmarkRoot::Toolbar).unwrap();
    f.store
        .add_bookmark(toolbar, None, "B", B, "bm-b", NOW)
        .unwrap();

    assert_eq!(f.store.clear_history(ClearRange::All, NOW + 2).unwrap(), 2);
    assert!(f.store.history(None, None, 10).unwrap().is_empty());
    assert_eq!(f.place(A), None);
    assert_eq!(f.place(B).unwrap().visit_count, 0);
    assert_eq!(f.store.clear_history(ClearRange::All, NOW + 2).unwrap(), 0);
}

// ---- words the tokenizer drops ----

#[test]
fn a_word_of_separator_characters_is_ignored() {
    let store = sample();
    // U+0345 is a combining mark (category Mn): a separator for FTS5's unicode61 tokenizer.
    assert_eq!(
        store.history(Some("servo \u{345}"), None, 10).unwrap(),
        store.history(Some("servo"), None, 10).unwrap()
    );
    assert_eq!(
        store
            .history(Some("\u{345} servo"), None, 10)
            .unwrap()
            .len(),
        2
    );
    assert!(store.history(Some("\u{345}"), None, 10).unwrap().is_empty());
}

// ---- bounded text ----

#[test]
fn long_titles_are_cut_and_long_urls_refused() {
    use mercury_store::{MAX_TITLE_CHARS, MAX_URL_BYTES, StoreError};
    let mut f = Fixture::new("bounded");
    let title = "é".repeat(MAX_TITLE_CHARS + 100);
    f.store
        .record_visit(A, Some(&title), VisitTransition::Link, NOW)
        .unwrap();
    let stored = f.place(A).unwrap().title.unwrap();
    assert_eq!(stored.chars().count(), MAX_TITLE_CHARS);
    assert!(title.starts_with(&stored));

    f.store
        .set_page_title(A, &"x".repeat(MAX_TITLE_CHARS * 2))
        .unwrap();
    assert_eq!(f.place(A).unwrap().title.unwrap().len(), MAX_TITLE_CHARS);

    let long_url = format!("https://example.com/{}", "a".repeat(MAX_URL_BYTES));
    assert!(matches!(
        f.store
            .record_visit(&long_url, None, VisitTransition::Link, NOW),
        Err(StoreError::InvalidArgument(_))
    ));
    let at_limit = format!("https://e.com/{}", "a".repeat(MAX_URL_BYTES - 14));
    assert_eq!(at_limit.len(), MAX_URL_BYTES);
    f.store
        .record_visit(&at_limit, None, VisitTransition::Link, NOW)
        .unwrap();
    assert!(matches!(
        f.store.import_visits(
            &[mercury_store::VisitImport {
                url: &long_url,
                title: None,
                visited_at_ms: NOW,
                transition: VisitTransition::Link,
            }],
            NOW
        ),
        Err(StoreError::ImportEntryRefused {
            index: 0,
            reason: mercury_store::ImportRefusal::UrlTooLong
        })
    ));
    assert_eq!(f.count("SELECT count(*) FROM places"), 2);
}
