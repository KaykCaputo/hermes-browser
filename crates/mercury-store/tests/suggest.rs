mod common;

use common::{DAY_MS, NOW};
use mercury_store::{BookmarkRoot, Store, Suggestion, VisitTransition};

fn urls(suggestions: &[Suggestion]) -> Vec<&str> {
    suggestions.iter().map(|s| s.url.as_str()).collect()
}

fn store() -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store
        .record_visit(
            "https://www.example.com/",
            Some("Example Domain"),
            VisitTransition::Typed,
            NOW,
        )
        .unwrap();
    store
        .record_visit(
            "https://example.org/news",
            Some("Daily News"),
            VisitTransition::Link,
            NOW - DAY_MS,
        )
        .unwrap();
    store
        .record_visit(
            "https://docs.rs/rusqlite/latest",
            Some("rusqlite documentation"),
            VisitTransition::Link,
            NOW - 20 * DAY_MS,
        )
        .unwrap();
    store
        .record_visit(
            "https://exampleshop.net/",
            Some("Shop"),
            VisitTransition::Link,
            NOW - 50 * DAY_MS,
        )
        .unwrap();
    store
}

#[test]
fn suggestions_match_host_prefix_with_or_without_www() {
    let store = store();
    assert_eq!(
        urls(&store.suggest("exam", 10, NOW).unwrap()),
        [
            "https://www.example.com/",
            "https://example.org/news",
            "https://exampleshop.net/",
        ]
    );
    assert_eq!(
        urls(&store.suggest("www.exam", 10, NOW).unwrap()),
        ["https://www.example.com/"]
    );
    assert_eq!(
        urls(&store.suggest("example.com", 10, NOW).unwrap()),
        ["https://www.example.com/"]
    );
    assert_eq!(
        urls(&store.suggest("EXAMPLE.ORG", 10, NOW).unwrap()),
        ["https://example.org/news"]
    );
}

#[test]
fn suggestions_match_title_words() {
    let store = store();
    assert_eq!(
        urls(&store.suggest("daily new", 10, NOW).unwrap()),
        ["https://example.org/news"]
    );
    assert_eq!(
        urls(&store.suggest("Documentation", 10, NOW).unwrap()),
        ["https://docs.rs/rusqlite/latest"]
    );
}

#[test]
fn suggestions_match_url_substrings() {
    let store = store();
    assert_eq!(
        urls(&store.suggest("rusqlite/lat", 10, NOW).unwrap()),
        ["https://docs.rs/rusqlite/latest"]
    );
    assert_eq!(
        urls(&store.suggest("ple.org/ne", 10, NOW).unwrap()),
        ["https://example.org/news"]
    );
}

#[test]
fn suggestions_are_ranked_by_frecency() {
    let store = store();
    let suggestions = store.suggest("https", 10, NOW).unwrap();
    assert_eq!(suggestions.len(), 4);
    assert!(
        suggestions
            .windows(2)
            .all(|w| w[0].frecency >= w[1].frecency)
    );
    assert_eq!(suggestions[0].url, "https://www.example.com/");
    assert_eq!(suggestions[0].frecency, 2000);
    assert_eq!(suggestions[0].title.as_deref(), Some("Example Domain"));
}

#[test]
fn bookmarks_are_included_and_flagged() {
    let mut store = store();
    let toolbar = store.bookmark_root(BookmarkRoot::Toolbar).unwrap();
    store
        .add_bookmark(
            toolbar,
            None,
            "Example Bookmark",
            "https://bookmarked.example/",
            "bm-1",
            NOW,
        )
        .unwrap();
    store
        .add_bookmark(
            toolbar,
            None,
            "Shop",
            "https://exampleshop.net/",
            "bm-2",
            NOW,
        )
        .unwrap();
    let suggestions = store.suggest("example", 10, NOW).unwrap();
    let flagged: Vec<(&str, bool)> = suggestions
        .iter()
        .map(|s| (s.url.as_str(), s.is_bookmark))
        .collect();
    assert!(flagged.contains(&("https://bookmarked.example/", true)));
    assert!(flagged.contains(&("https://exampleshop.net/", true)));
    assert!(flagged.contains(&("https://www.example.com/", false)));
    let bookmarked = suggestions
        .iter()
        .find(|s| s.url == "https://bookmarked.example/")
        .unwrap();
    // A page with no title of its own shows its bookmark's title.
    assert_eq!(bookmarked.title.as_deref(), Some("Example Bookmark"));
    assert_eq!(bookmarked.frecency, 140);
}

#[test]
fn suggestions_are_unique_by_url() {
    let mut store = store();
    let toolbar = store.bookmark_root(BookmarkRoot::Toolbar).unwrap();
    let other = store.bookmark_root(BookmarkRoot::Other).unwrap();
    store
        .add_bookmark(
            toolbar,
            None,
            "Example",
            "https://www.example.com/",
            "bm-1",
            NOW,
        )
        .unwrap();
    store
        .add_bookmark(
            other,
            None,
            "Example again",
            "https://www.example.com/",
            "bm-2",
            NOW,
        )
        .unwrap();
    store
        .record_visit("https://www.example.com/", None, VisitTransition::Link, NOW)
        .unwrap();
    let suggestions = store.suggest("example", 10, NOW).unwrap();
    let mut seen: Vec<&str> = urls(&suggestions);
    let total = seen.len();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), total);
}

#[test]
fn limit_is_honoured_and_empty_input_returns_nothing() {
    let store = store();
    assert_eq!(store.suggest("https", 2, NOW).unwrap().len(), 2);
    assert!(store.suggest("https", 0, NOW).unwrap().is_empty());
    assert!(store.suggest("", 10, NOW).unwrap().is_empty());
    assert!(store.suggest("   ", 10, NOW).unwrap().is_empty());
}

#[test]
fn hostile_input_does_not_fail() {
    let store = store();
    let long = "x".repeat(2000);
    for input in [
        "\"",
        "*",
        "(",
        "NEAR",
        "-",
        "%",
        "_",
        "\\",
        "a%b",
        long.as_str(),
    ] {
        store
            .suggest(input, 10, NOW)
            .unwrap_or_else(|e| panic!("input {input:?} failed: {e}"));
    }
    // LIKE wildcards are literal.
    assert!(store.suggest("%", 10, NOW).unwrap().is_empty());
    assert!(store.suggest("_", 10, NOW).unwrap().is_empty());
}
