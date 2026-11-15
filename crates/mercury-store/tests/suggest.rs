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

// ---- bookmark titles ----

const PAYROLL: &str = "https://intranet.acme.example/ss/x7";

fn found(store: &Store, input: &str, url: &str) -> Option<Suggestion> {
    store
        .suggest(input, 10, NOW)
        .unwrap()
        .into_iter()
        .find(|s| s.url == url)
}

#[test]
fn a_never_visited_bookmark_is_found_by_its_title() {
    let mut store = store();
    let toolbar = store.bookmark_root(BookmarkRoot::Toolbar).unwrap();
    store
        .add_bookmark(toolbar, None, "Payroll", PAYROLL, "bm-pay", NOW)
        .unwrap();
    let hit = found(&store, "payroll", PAYROLL).expect("found by a title word");
    assert_eq!(hit.title.as_deref(), Some("Payroll"));
    assert!(hit.is_bookmark);
    assert!(
        found(&store, "PAYR", PAYROLL).is_some(),
        "prefix of the last word"
    );
    assert!(
        found(&store, "intranet.acme", PAYROLL).is_some(),
        "host prefix"
    );
}

#[test]
fn bookmark_title_matches_follow_rename_and_removal() {
    let mut store = store();
    // Visited (without a title), so the page outlives its bookmark.
    store
        .record_visit(PAYROLL, None, VisitTransition::Link, NOW)
        .unwrap();
    let toolbar = store.bookmark_root(BookmarkRoot::Toolbar).unwrap();
    let id = store
        .add_bookmark(toolbar, None, "Payroll", PAYROLL, "bm-pay", NOW)
        .unwrap();
    assert!(found(&store, "payroll", PAYROLL).is_some());

    store.rename_bookmark(id, "Salaries", NOW).unwrap();
    assert!(found(&store, "payroll", PAYROLL).is_none(), "old title");
    assert!(found(&store, "salar", PAYROLL).is_some(), "new title");

    store.remove_bookmark(id, NOW).unwrap();
    assert!(found(&store, "salaries", PAYROLL).is_none(), "removed");
    assert!(found(&store, "intranet", PAYROLL).is_some(), "page kept");
}

// ---- stale scores and refresh_frecencies ----

/// Five pages with 30 link visits 100-129 days ago (stored 2550 when last visited, 300 now) and
/// one with 4 link visits today (400 both stored and now).
fn stale_and_fresh() -> Store {
    let mut store = Store::open_in_memory().unwrap();
    for page in 0..5 {
        let url = format!("https://stale-{page}.example/");
        for i in (0..30).rev() {
            store
                .record_visit(&url, None, VisitTransition::Link, NOW - (100 + i) * DAY_MS)
                .unwrap();
        }
    }
    for i in (0..4).rev() {
        store
            .record_visit(
                "https://fresh.example/",
                None,
                VisitTransition::Link,
                NOW - i * 1000,
            )
            .unwrap();
    }
    store
}

#[test]
fn a_stale_window_hides_a_fresh_page_until_refresh() {
    let mut store = stale_and_fresh();
    // limit 1 gives each source a window of 4 candidates by stored score: all stale pages.
    let before = store.suggest("example", 1, NOW).unwrap();
    assert_eq!(before.len(), 1);
    assert!(before[0].url.starts_with("https://stale-"), "{before:?}");
    assert_eq!(before[0].frecency, 300);

    assert_eq!(store.refresh_frecencies(NOW, 100).unwrap(), 5);
    let after = store.suggest("example", 1, NOW).unwrap();
    assert_eq!(after[0].url, "https://fresh.example/");
    assert_eq!(after[0].frecency, 400);
}

#[test]
fn ranking_within_the_window_uses_the_score_as_of_now() {
    let store = stale_and_fresh();
    let ranked = store.suggest("example", 10, NOW).unwrap();
    assert_eq!(ranked.len(), 6);
    assert_eq!(ranked[0].url, "https://fresh.example/");
    assert!(ranked[1..].iter().all(|s| s.frecency == 300));
}

#[test]
fn refresh_rescores_the_highest_stale_scores_first() {
    let mut f = common::Fixture::new("refresh");
    for (url, days) in [
        ("https://a.example/", 10),
        ("https://b.example/", 50),
        ("https://c.example/", 200),
    ] {
        f.store
            .record_visit(url, None, VisitTransition::Typed, NOW - days * DAY_MS)
            .unwrap();
    }
    // Each was scored at its own visit: 2000.
    assert_eq!(f.place("https://c.example/").unwrap().frecency, 2000);

    assert_eq!(f.store.refresh_frecencies(NOW, 2).unwrap(), 2);
    // Ties on the stored score: any two of the three were re-scored; one is still 2000.
    let scores: Vec<i64> = ["a", "b", "c"]
        .iter()
        .map(|p| f.place(&format!("https://{p}.example/")).unwrap().frecency)
        .collect();
    assert_eq!(
        scores.iter().filter(|&&s| s == 2000).count(),
        1,
        "{scores:?}"
    );

    assert_eq!(f.store.refresh_frecencies(NOW, 10).unwrap(), 1);
    // 10 days: 70 × 20 = 1400; 50 days: 30 × 20 = 600; 200 days: 10 × 20 = 200.
    assert_eq!(f.place("https://a.example/").unwrap().frecency, 1400);
    assert_eq!(f.place("https://b.example/").unwrap().frecency, 600);
    assert_eq!(f.place("https://c.example/").unwrap().frecency, 200);
    // Everything was scored less than a day ago: nothing to do.
    assert_eq!(f.store.refresh_frecencies(NOW + 1000, 10).unwrap(), 0);
    assert_eq!(f.store.refresh_frecencies(NOW, 0).unwrap(), 0);
}

#[test]
fn a_future_dated_visit_counts_as_a_visit_now() {
    let mut store = Store::open_in_memory().unwrap();
    store
        .record_visit(
            "https://future.example/",
            None,
            VisitTransition::Link,
            NOW + 30 * DAY_MS,
        )
        .unwrap();
    // Age rounds to -30 days, which falls in the newest bucket: 100 × 100 / 100 = 100.
    let s = store.suggest("future", 10, NOW).unwrap();
    assert_eq!(s[0].frecency, 100);
}

// ---- round 3: bounds, ties, titles, host keys ----

#[test]
fn limit_is_clamped_to_the_maximum() {
    let mut store = Store::open_in_memory().unwrap();
    for i in 0..(mercury_store::MAX_SUGGESTIONS + 10) {
        store
            .record_visit(
                &format!("https://page{i}.example/"),
                None,
                VisitTransition::Link,
                NOW,
            )
            .unwrap();
    }
    assert_eq!(
        store.suggest("page", u32::MAX, NOW).unwrap().len(),
        mercury_store::MAX_SUGGESTIONS as usize
    );
}

#[test]
fn the_first_keystrokes_of_a_url_still_suggest_pages() {
    let store = store();
    for input in [
        "h", "ht", "https", "https://", "w", "www", "www.", "c", "co", "com",
    ] {
        assert!(
            !store.suggest(input, 10, NOW).unwrap().is_empty(),
            "{input:?} found nothing"
        );
    }
    assert_eq!(
        urls(&store.suggest("https://www.exa", 10, NOW).unwrap()),
        ["https://www.example.com/"]
    );
}

#[test]
fn a_rare_word_finds_its_page_however_low_its_score() {
    let mut store = Store::open_in_memory().unwrap();
    // 3,000 well-used pages, more than any source examines by score.
    let urls: Vec<String> = (0..3000)
        .map(|i| format!("https://busy{i}.example/"))
        .collect();
    let imports: Vec<mercury_store::VisitImport<'_>> = urls
        .iter()
        .map(|url| mercury_store::VisitImport {
            url,
            title: Some("Busy page"),
            visited_at_ms: NOW,
            transition: VisitTransition::Typed,
        })
        .collect();
    store.import_visits(&imports, NOW).unwrap();
    store
        .record_visit(
            "https://quiet.example/old",
            Some("Zebrafish anatomy"),
            VisitTransition::Reload,
            NOW - 400 * DAY_MS,
        )
        .unwrap();
    let hit = store.suggest("zebrafish", 10, NOW).unwrap();
    assert_eq!(urls_of(&hit), ["https://quiet.example/old"]);
    assert_eq!(hit[0].frecency, -1);
    assert_eq!(
        urls_of(&store.suggest("quiet.exa", 10, NOW).unwrap()),
        ["https://quiet.example/old"]
    );
}

fn urls_of(suggestions: &[Suggestion]) -> Vec<String> {
    suggestions.iter().map(|s| s.url.clone()).collect()
}

#[test]
fn equal_scores_enter_the_window_in_a_fixed_order() {
    // 200 never-visited bookmarks, all scored 140, matching only through their titles. They
    // are added in descending URL order, so id order and URL order disagree.
    let run = || {
        let mut store = Store::open_in_memory().unwrap();
        let toolbar = store.bookmark_root(BookmarkRoot::Toolbar).unwrap();
        for i in (0..200).rev() {
            store
                .add_bookmark(
                    toolbar,
                    None,
                    &format!("Gadget {i}"),
                    &format!("https://site-{i:03}.example/"),
                    &format!("g-{i}"),
                    NOW,
                )
                .unwrap();
        }
        urls_of(&store.suggest("gadget", 1, NOW).unwrap())
    };
    // Ties are broken by the newest row first, so the window holds the last four added
    // (site-000 … site-003), and the final ranking picks the smallest URL among them.
    assert_eq!(run(), ["https://site-000.example/"]);
    assert_eq!(run(), run());
}

#[test]
fn a_bookmarked_page_shows_its_bookmark_title() {
    let mut store = store();
    let toolbar = store.bookmark_root(BookmarkRoot::Toolbar).unwrap();
    store
        .add_bookmark(
            toolbar,
            None,
            "My shop",
            "https://exampleshop.net/",
            "bm-shop",
            NOW,
        )
        .unwrap();
    let hit = store.suggest("my shop", 10, NOW).unwrap();
    assert_eq!(urls_of(&hit), ["https://exampleshop.net/"]);
    assert_eq!(hit[0].title.as_deref(), Some("My shop"));
    let by_url = store.suggest("exampleshop", 10, NOW).unwrap();
    assert_eq!(by_url[0].title.as_deref(), Some("My shop"));
}

#[test]
fn host_keys_ignore_user_info_and_read_international_hosts() {
    let mut store = Store::open_in_memory().unwrap();
    store
        .record_visit(
            "https://user:secret@example.com/a",
            None,
            VisitTransition::Link,
            NOW,
        )
        .unwrap();
    store
        .record_visit(
            "https://www.xn--bcher-kva.test/",
            None,
            VisitTransition::Link,
            NOW,
        )
        .unwrap();
    assert_eq!(
        urls_of(&store.suggest("exam", 10, NOW).unwrap()),
        ["https://user:secret@example.com/a"]
    );
    for input in ["bü", "BÜCH", "bücher.test"] {
        assert_eq!(
            urls_of(&store.suggest(input, 10, NOW).unwrap()),
            ["https://www.xn--bcher-kva.test/"],
            "{input}"
        );
    }
}
