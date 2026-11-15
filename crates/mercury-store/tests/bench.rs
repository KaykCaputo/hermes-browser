//! A timing benchmark on a large synthetic profile. Not part of the normal test run; run it
//! explicitly in release mode:
//!
//! ```text
//! cargo test --release -p mercury-store --test bench -- --ignored --nocapture
//! ```
//!
//! It builds, in a temporary directory, a database of 100,000 pages with about 300,000 visits
//! and 2,000 bookmarks from deterministic synthetic data shaped like a real history: most URLs
//! are `https://www.<site>.com/<path>`, a few sites hold many pages, and page titles share a
//! skewed vocabulary. The visits end three weeks before "now", so every stored score is stale.
//!
//! It then times `suggest` for the first keystrokes people type (`h` … `https`, `w` … `www`,
//! `c` … `com`), a common path word, common and rare title words, an 8-character word and an
//! input that matches nothing. The inputs are interleaved, one call each per round as
//! successive keystrokes would arrive, for 50 rounds; it prints the median and the worst call
//! per input (and the 90th percentile, to tell a slow call from a descheduled one). It does so with every stored score stale and again after refreshing them. It
//! also times `record_visit` and `refresh_frecencies(_, 1000)` with everything stale,
//! everything fresh and one page in a thousand stale, and prints the database size.
//!
//! With `MERCURY_BENCH_DB=<path>` the database is kept at that path (delete it before a run).

mod common;

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use common::{DAY_MS, NOW, TempDir};
use mercury_store::{BookmarkImport, BookmarkRoot, Store, VisitImport, VisitTransition};

const PAGES: usize = 100_000;
const BOOKMARKS: usize = 2_000;
const ROUNDS: usize = 50;

/// SplitMix64: a small deterministic generator.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    /// Skewed towards 0: a few values are common, most are rare.
    fn skewed(&mut self, n: usize) -> usize {
        let x = (self.next() >> 11) as f64 / (1u64 << 53) as f64;
        ((x * x * x) * n as f64) as usize
    }
}

const SYLLABLES: &[&str] = &[
    "ka", "lo", "mi", "ra", "te", "su", "no", "ve", "pa", "di", "ro", "ne", "ba", "li", "to", "ma",
    "se", "ga", "fu", "he", "ju", "wi", "co", "ze", "qu", "xo",
];

const PATH_WORDS: &[&str] = &[
    "index", "news", "article", "search", "products", "blog", "watch", "wiki", "docs", "help",
    "user", "post", "category", "about", "item",
];

const TLDS: &[(&str, usize)] = &[
    ("com", 60),
    ("org", 10),
    ("net", 7),
    ("de", 6),
    ("io", 5),
    ("co.uk", 4),
    ("fr", 3),
    ("edu", 2),
    ("gov", 2),
    ("nl", 1),
];

struct Data {
    pages: Vec<(String, String)>,
    /// (page index, visited at, transition), oldest first.
    visits: Vec<(usize, i64, VisitTransition)>,
    /// (folder, title, url)
    bookmarks: Vec<(usize, String, String)>,
    common_word: String,
    rare_word: String,
    long_word: String,
}

fn pick_tld(rng: &mut Rng) -> &'static str {
    let mut n = rng.below(100);
    for &(tld, weight) in TLDS {
        if n < weight {
            return tld;
        }
        n -= weight;
    }
    "com"
}

fn generate() -> Data {
    let mut rng = Rng(42);
    let words: Vec<String> = (0..3000)
        .map(|_| {
            let n = 2 + rng.below(3);
            (0..n)
                .map(|_| SYLLABLES[rng.skewed(SYLLABLES.len())])
                .collect()
        })
        .collect();
    let sites: Vec<String> = (0..5000)
        .map(|_| {
            let scheme = if rng.below(10) == 0 { "http" } else { "https" };
            let www = if rng.below(100) < 65 { "www." } else { "" };
            format!(
                "{scheme}://{www}{}{}.{}",
                words[rng.below(words.len())],
                words[rng.below(words.len())],
                pick_tld(&mut rng)
            )
        })
        .collect();
    let pages: Vec<(String, String)> = (0..PAGES)
        .map(|i| {
            let site = &sites[rng.skewed(sites.len())];
            let url = match rng.below(4) {
                0 => format!("{site}/{i}/index.html"),
                _ => format!(
                    "{site}/{}/{}-{i}",
                    PATH_WORDS[rng.skewed(PATH_WORDS.len())],
                    words[rng.below(words.len())]
                ),
            };
            let title = (0..3 + rng.below(6))
                .map(|_| words[rng.skewed(words.len())].as_str())
                .collect::<Vec<_>>()
                .join(" ");
            (url, title)
        })
        .collect();
    let first = NOW - 386 * DAY_MS;
    let span = 365 * DAY_MS;
    let mut visits = Vec::new();
    for page in 0..PAGES {
        for _ in 0..1 + rng.below(5) {
            let at = first + (rng.next() % span as u64) as i64;
            let transition = match rng.below(20) {
                0 | 1 => VisitTransition::Typed,
                2 => VisitTransition::Reload,
                _ => VisitTransition::Link,
            };
            visits.push((page, at, transition));
        }
    }
    visits.sort_by_key(|v| v.1);
    let bookmarks = (0..BOOKMARKS)
        .map(|i| {
            let url = if i % 10 == 0 {
                format!("https://intranet.example/unvisited/{i}")
            } else {
                pages[rng.below(PAGES)].0.clone()
            };
            let title = format!(
                "{} {}",
                words[rng.below(words.len())],
                words[rng.skewed(words.len())]
            );
            (i % 20, title, url)
        })
        .collect();

    let mut counts = BTreeMap::<&str, usize>::new();
    for (_, title) in &pages {
        for word in title.split(' ') {
            *counts.entry(word).or_default() += 1;
        }
    }
    let mut ranked: Vec<(&str, usize)> = counts.into_iter().collect();
    ranked.sort_by_key(|&(w, n)| (std::cmp::Reverse(n), w));
    let common_word = ranked[0].0.to_string();
    let rare_word = ranked
        .iter()
        .rev()
        .find(|(w, _)| w.starts_with("qu") && w.len() >= 6)
        .unwrap()
        .0
        .to_string();
    // An 8-character word of middling frequency.
    let long_word = ranked
        .iter()
        .filter(|(w, _)| w.len() == 8)
        .nth(50)
        .unwrap()
        .0
        .to_string();
    Data {
        pages,
        visits,
        bookmarks,
        common_word,
        rare_word,
        long_word,
    }
}

/// Loads the data as the browser would have: visits day by day, each day's frecency computed at
/// the end of that day.
fn load(store: &mut Store, data: &Data) {
    let mut day_start = 0;
    while day_start < data.visits.len() {
        let day = data.visits[day_start].1 / DAY_MS;
        let mut day_end = day_start;
        while day_end < data.visits.len() && data.visits[day_end].1 / DAY_MS == day {
            day_end += 1;
        }
        let batch: Vec<VisitImport<'_>> = data.visits[day_start..day_end]
            .iter()
            .map(|&(page, at, transition)| VisitImport {
                url: &data.pages[page].0,
                title: Some(&data.pages[page].1),
                visited_at_ms: at,
                transition,
            })
            .collect();
        store.import_visits(&batch, (day + 1) * DAY_MS).unwrap();
        day_start = day_end;
    }
    let folders: Vec<BookmarkImport> = (0..20)
        .map(|f| BookmarkImport::Folder {
            title: format!("Folder {f}"),
            guid: format!("folder-{f}"),
            added_at_ms: None,
            children: data
                .bookmarks
                .iter()
                .enumerate()
                .filter(|(_, b)| b.0 == f)
                .map(|(i, (_, title, url))| BookmarkImport::Bookmark {
                    title: title.clone(),
                    url: url.clone(),
                    guid: format!("bookmark-{i}"),
                    added_at_ms: None,
                })
                .collect(),
        })
        .collect();
    let other = store.bookmark_root(BookmarkRoot::Other).unwrap();
    store
        .import_bookmarks(other, &folders, NOW - 21 * DAY_MS)
        .unwrap();
}

fn summary(mut samples: Vec<Duration>) -> (String, String) {
    let (median, _, worst) = summary3(&mut samples);
    (median, worst)
}

/// Median, 90th percentile and worst, in milliseconds.
fn summary3(samples: &mut [Duration]) -> (String, String, String) {
    samples.sort();
    let ms = |d: Duration| format!("{:.2}", d.as_secs_f64() * 1000.0);
    let n = samples.len();
    (
        ms(samples[n / 2]),
        ms(samples[n * 9 / 10]),
        ms(samples[n - 1]),
    )
}

fn time_suggestions(store: &Store, inputs: &[(String, String)], label: &str) {
    let mut samples: Vec<Vec<Duration>> = vec![Vec::new(); inputs.len()];
    let mut results = vec![0; inputs.len()];
    for _ in 0..ROUNDS {
        for (i, (_, input)) in inputs.iter().enumerate() {
            let start = Instant::now();
            results[i] = store.suggest(input, 10, NOW).unwrap().len();
            samples[i].push(start.elapsed());
        }
    }
    println!("\n{label}\n");
    println!("| input | median ms | p90 ms | worst ms | results |");
    println!("| --- | --- | --- | --- | --- |");
    for (i, (name, input)) in inputs.iter().enumerate() {
        let (median, p90, worst) = summary3(&mut samples[i]);
        println!(
            "| {name} `{input}` | {median} | {p90} | {worst} | {} |",
            results[i]
        );
    }
}

#[test]
#[ignore = "benchmark; run explicitly in release mode"]
fn bench_suggest_record_and_refresh() {
    let dir = TempDir::new("bench");
    let data = generate();
    let path = std::env::var_os("MERCURY_BENCH_DB").map_or_else(|| dir.db(), Into::into);
    let mut store = Store::open(&path).unwrap();
    let start = Instant::now();
    load(&mut store, &data);
    println!(
        "loaded {} pages, {} visits, {} bookmarks in {:.1} s",
        data.pages.len(),
        data.visits.len(),
        data.bookmarks.len(),
        start.elapsed().as_secs_f64()
    );
    let raw = rusqlite::Connection::open(&path).unwrap();
    let places: i64 = raw
        .query_row("SELECT count(*) FROM places", [], |row| row.get(0))
        .unwrap();
    println!("distinct pages stored: {places}");
    raw.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
        .unwrap();
    println!(
        "database size after load: {:.1} MiB",
        std::fs::metadata(&path).unwrap().len() as f64 / (1024.0 * 1024.0)
    );

    let mut inputs: Vec<(String, String)> = [
        "h", "ht", "htt", "https", "w", "ww", "www", "c", "co", "com", "index",
    ]
    .iter()
    .map(|s| ("keystroke".to_string(), s.to_string()))
    .collect();
    inputs.push(("common title word".into(), data.common_word.clone()));
    inputs.push(("rare title word".into(), data.rare_word.clone()));
    inputs.push(("8-character word".into(), data.long_word.clone()));
    inputs.push(("no match".into(), "zqxjv".into()));
    time_suggestions(&store, &inputs, "suggest, every stored score stale");

    let mut samples = Vec::new();
    for i in 0..ROUNDS {
        let page = &data.pages[(i * 7919) % data.pages.len()];
        let start = Instant::now();
        store
            .record_visit(
                &page.0,
                Some(&page.1),
                VisitTransition::Link,
                NOW + i as i64,
            )
            .unwrap();
        samples.push(start.elapsed());
    }
    let (median, worst) = summary(samples);
    println!("\n| call | median ms | worst ms | returned |\n| --- | --- | --- | --- |");
    println!("| record_visit | {median} | {worst} | |");

    let mut samples = Vec::new();
    let mut returned = 0;
    for _ in 0..ROUNDS {
        let start = Instant::now();
        returned = store.refresh_frecencies(NOW, 1000).unwrap();
        samples.push(start.elapsed());
    }
    let (median, worst) = summary(samples);
    println!("| refresh_frecencies(1000), everything stale | {median} | {worst} | {returned} |");
    while store.refresh_frecencies(NOW, 1000).unwrap() == 1000 {}

    let mut samples = Vec::new();
    for _ in 0..ROUNDS {
        let start = Instant::now();
        returned = store.refresh_frecencies(NOW, 1000).unwrap();
        samples.push(start.elapsed());
    }
    let (median, worst) = summary(samples);
    println!("| refresh_frecencies(1000), everything fresh | {median} | {worst} | {returned} |");

    let mut samples = Vec::new();
    for _ in 0..ROUNDS {
        raw.execute("UPDATE places SET frecency_at = 0 WHERE id % 1000 = 0", [])
            .unwrap();
        let start = Instant::now();
        returned = store.refresh_frecencies(NOW, 1000).unwrap();
        samples.push(start.elapsed());
    }
    let (median, worst) = summary(samples);
    println!("| refresh_frecencies(1000), 1 in 1000 stale | {median} | {worst} | {returned} |");

    time_suggestions(&store, &inputs, "suggest, after refreshing every score");
}
