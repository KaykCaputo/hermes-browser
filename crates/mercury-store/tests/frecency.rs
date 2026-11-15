//! Frecency, ported from application-services `components/places/src/frecency.rs`.
//!
//! The donor file has no tests of its own, so the worked examples below are derived by hand from
//! its formula and its default settings:
//! - each of the 10 most recent visits scores `weight(age) × bonus / 100`, where the weight is
//!   100 / 70 / 50 / 30 / 10 for ages (rounded to whole days) up to 4 / 14 / 31 / 90 / more days,
//!   and the bonus is 2000 for typed, 100 for link, 75 for a bookmark visit, 0 for a reload, plus
//!   75 for every sampled visit if the page is bookmarked;
//! - `frecency = ceil(visit_count × ceil(points) / sampled)`;
//! - an unvisited bookmark scores `ceil(100 × 140 / 100)`, plus `ceil(100 × 200 / 100)` if typed.

mod common;

use common::{DAY_MS, Fixture, NOW};
use mercury_store::{BookmarkRoot, Store, VisitTransition};

fn frecency(store: &Store, url: &str) -> i64 {
    store
        .suggest(url, 50, NOW)
        .unwrap()
        .into_iter()
        .find(|s| s.url == url)
        .unwrap_or_else(|| panic!("{url} not suggested"))
        .frecency
}

#[test]
fn worked_example_one_typed_visit_today_scores_2000() {
    // One typed visit, age 0 days: weight 100, bonus 2000.
    // points = 100 × 2000 / 100 = 2000; frecency = ceil(1 × ceil(2000) / 1) = 2000.
    let mut f = Fixture::new("typed-today");
    f.store
        .record_visit("https://example.com/", None, VisitTransition::Typed, NOW)
        .unwrap();
    assert_eq!(f.place("https://example.com/").unwrap().frecency, 2000);
}

#[test]
fn worked_example_bookmarked_link_visit_ten_days_ago_scores_123() {
    // One link visit, age 10 days: weight 70 (4 < 10 <= 14). Bookmarked, so the bonus is
    // link 100 + bookmark 75 = 175.
    // points = 70 × 175 / 100 = 122.5; frecency = ceil(1 × ceil(122.5) / 1) = 123.
    let mut f = Fixture::new("bookmarked-link");
    let url = "https://example.com/";
    f.store
        .record_visit(url, None, VisitTransition::Link, NOW - 10 * DAY_MS)
        .unwrap();
    let toolbar = f.store.bookmark_root(BookmarkRoot::Toolbar).unwrap();
    f.store
        .add_bookmark(toolbar, None, "Example", url, "bm-1", NOW)
        .unwrap();
    assert_eq!(f.place(url).unwrap().frecency, 123);
}

#[test]
fn worked_example_only_the_ten_newest_visits_are_sampled() {
    // 12 visits; the 10 newest are sampled:
    //   1 typed today          weight 100 × 2000 / 100 = 2000
    //   1 link  10 days ago    weight  70 ×  100 / 100 =   70
    //   3 link  20 days ago    weight  50 ×  100 / 100 =   50 each → 150
    //   5 link  60 days ago    weight  30 ×  100 / 100 =   30 each → 150
    //   2 link 200 days ago    not sampled (they would score 10 each)
    // points = 2370; frecency = ceil(12 × ceil(2370) / 10) = ceil(2844) = 2844.
    let mut f = Fixture::new("sampling");
    let url = "https://example.com/";
    let mut visits = vec![(200, VisitTransition::Link); 2];
    visits.extend([(60, VisitTransition::Link); 5]);
    visits.extend([(20, VisitTransition::Link); 3]);
    visits.push((10, VisitTransition::Link));
    visits.push((0, VisitTransition::Typed));
    for (days, transition) in visits {
        f.store
            .record_visit(url, None, transition, NOW - days * DAY_MS)
            .unwrap();
    }
    let place = f.place(url).unwrap();
    assert_eq!(place.visit_count, 12);
    assert_eq!(place.frecency, 2844);
}

#[test]
fn an_unvisited_bookmark_gets_the_bookmark_only_score() {
    // No visits: ceil(first bucket weight 100 × unvisited bookmark bonus 140 / 100) = 140.
    let mut store = Store::open_in_memory().unwrap();
    let toolbar = store.bookmark_root(BookmarkRoot::Toolbar).unwrap();
    store
        .add_bookmark(
            toolbar,
            None,
            "Never visited",
            "https://unvisited.example/",
            "bm-1",
            NOW,
        )
        .unwrap();
    assert_eq!(frecency(&store, "https://unvisited.example/"), 140);
}

#[test]
fn reload_only_visits_score_minus_one() {
    // Every sampled visit has a zero bonus, so the donor returns -1 ("still show it").
    let mut f = Fixture::new("reload-only");
    f.store
        .record_visit("https://example.com/", None, VisitTransition::Reload, NOW)
        .unwrap();
    assert_eq!(f.place("https://example.com/").unwrap().frecency, -1);
}

#[test]
fn a_page_typed_today_outranks_one_linked_100_days_ago() {
    let mut store = Store::open_in_memory().unwrap();
    store
        .record_visit(
            "https://old.example/",
            None,
            VisitTransition::Link,
            NOW - 100 * DAY_MS,
        )
        .unwrap();
    store
        .record_visit("https://new.example/", None, VisitTransition::Typed, NOW)
        .unwrap();
    let urls: Vec<String> = store
        .suggest("example", 10, NOW)
        .unwrap()
        .into_iter()
        .map(|s| s.url)
        .collect();
    assert_eq!(urls, ["https://new.example/", "https://old.example/"]);
}

#[test]
fn many_old_visits_lose_to_a_few_recent_ones() {
    // Old page: 30 link visits 100 to 129 days before now. At its last visit its stored score was
    // high, but suggestions are ranked as of `now_ms`: the 10 newest visits are 100-109 days old,
    // weight 10 each → 100 points; ceil(30 × 100 / 10) = 300.
    // Recent page: 4 link visits today → 400 points; ceil(4 × 400 / 4) = 400.
    let mut store = Store::open_in_memory().unwrap();
    for i in (0..30).rev() {
        store
            .record_visit(
                "https://old.example/",
                None,
                VisitTransition::Link,
                NOW - (100 + i) * DAY_MS,
            )
            .unwrap();
    }
    for i in (0..4).rev() {
        store
            .record_visit(
                "https://recent.example/",
                None,
                VisitTransition::Link,
                NOW - i * 1000,
            )
            .unwrap();
    }
    let ranked: Vec<(String, i64)> = store
        .suggest("example", 10, NOW)
        .unwrap()
        .into_iter()
        .map(|s| (s.url, s.frecency))
        .collect();
    assert_eq!(
        ranked,
        [
            ("https://recent.example/".to_string(), 400),
            ("https://old.example/".to_string(), 300),
        ]
    );
}

#[test]
fn the_stored_score_is_as_of_the_last_change() {
    // Stored when the last of 30 daily link visits was recorded: the 10 newest are 0-9 days old,
    // 5 × 100 + 5 × 70 = 850 points; ceil(30 × 850 / 10) = 2550. Suggestions recompute it.
    let mut f = Fixture::new("stored-score");
    for i in (0..30).rev() {
        f.store
            .record_visit(
                "https://old.example/",
                None,
                VisitTransition::Link,
                NOW - (100 + i) * DAY_MS,
            )
            .unwrap();
    }
    assert_eq!(f.place("https://old.example/").unwrap().frecency, 2550);
    assert_eq!(frecency(&f.store, "https://old.example/"), 300);
}

#[test]
fn frecency_is_recomputed_on_delete_and_on_bookmark_changes() {
    let mut f = Fixture::new("recompute");
    let url = "https://example.com/";
    f.store
        .record_visit(url, None, VisitTransition::Link, NOW - 10 * DAY_MS)
        .unwrap();
    let typed = f
        .store
        .record_visit(url, None, VisitTransition::Typed, NOW)
        .unwrap();
    // typed today 2000 + link 10 days 70 = 2070; ceil(2 × 2070 / 2) = 2070.
    assert_eq!(f.place(url).unwrap().frecency, 2070);

    f.store.delete_visit(typed, NOW).unwrap();
    // One link visit 10 days old: 70.
    assert_eq!(f.place(url).unwrap().frecency, 70);

    let toolbar = f.store.bookmark_root(BookmarkRoot::Toolbar).unwrap();
    let bookmark = f
        .store
        .add_bookmark(toolbar, None, "E", url, "bm-1", NOW)
        .unwrap();
    assert_eq!(f.place(url).unwrap().frecency, 123);

    f.store.remove_bookmark(bookmark, NOW).unwrap();
    assert_eq!(f.place(url).unwrap().frecency, 70);

    let other = f
        .store
        .add_bookmark(toolbar, None, "E", url, "bm-2", NOW)
        .unwrap();
    f.store
        .set_bookmark_url(other, "https://elsewhere.example/", NOW)
        .unwrap();
    assert_eq!(f.place(url).unwrap().frecency, 70);
    assert_eq!(f.place("https://elsewhere.example/").unwrap().frecency, 140);
}
