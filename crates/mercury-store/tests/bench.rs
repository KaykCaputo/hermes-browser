//! A timing benchmark on a large synthetic profile. Not part of the normal test run; run it
//! explicitly in release mode:
//!
//! ```text
//! cargo test --release -p mercury-store --test bench -- --ignored --nocapture
//! ```
//!
//! It builds, in a temporary directory, a database of 100,000 pages with about 300,000 visits
//! and 2,000 bookmarks from deterministic synthetic data. The visits end three weeks before
//! "now", so every stored score is stale, as after a holiday away from the browser. It then
//! times 50 calls of `suggest` for inputs of 1, 2, 4 and 8 characters (a common and a rare
//! word, plus one that matches nothing), of `record_visit`, and of `refresh_frecencies(_, 1000)`,
//! and prints the median and the worst call. With `MERCURY_BENCH_DB=<path>` the database is kept
//! at that path and reused by later runs (the timed calls change it, so delete it for a clean run).

mod common;

use std::time::{Duration, Instant};

use common::{DAY_MS, NOW, TempDir};
use mercury_store::{BookmarkImport, BookmarkRoot, Store, VisitImport, VisitTransition};

const PAGES: usize = 100_000;
const BOOKMARKS: usize = 2_000;
const CALLS: usize = 50;

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

struct Data {
    words: Vec<String>,
    pages: Vec<(String, String)>,
    /// (page index, visited at, transition), oldest first.
    visits: Vec<(usize, i64, VisitTransition)>,
    /// (folder, title, url)
    bookmarks: Vec<(usize, String, String)>,
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
    let tlds = ["com", "org", "net", "io", "de"];
    let hosts: Vec<String> = (0..4000)
        .map(|i| {
            let www = if rng.below(10) < 4 { "www." } else { "" };
            format!(
                "{www}{}{}.{}",
                words[rng.skewed(words.len())],
                words[rng.below(words.len())],
                tlds[i % tlds.len()]
            )
        })
        .collect();
    let pages: Vec<(String, String)> = (0..PAGES)
        .map(|i| {
            let host = &hosts[rng.skewed(hosts.len())];
            let url = format!(
                "https://{host}/{}/{}-{i}",
                words[rng.skewed(words.len())],
                words[rng.below(words.len())]
            );
            let title = (0..3 + rng.below(4))
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
    Data {
        words,
        pages,
        visits,
        bookmarks,
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

fn time<F: FnMut(usize)>(mut f: F) -> (Duration, Duration) {
    let mut samples: Vec<Duration> = (0..CALLS)
        .map(|i| {
            let start = Instant::now();
            f(i);
            start.elapsed()
        })
        .collect();
    samples.sort();
    (samples[CALLS / 2], samples[CALLS - 1])
}

fn ms(d: Duration) -> String {
    format!("{:.2}", d.as_secs_f64() * 1000.0)
}

/// The most and least frequent title words of at least 8 characters.
fn common_and_rare(data: &Data) -> (String, String) {
    let mut counts = std::collections::HashMap::<&str, usize>::new();
    for (_, title) in &data.pages {
        for word in title.split(' ') {
            *counts.entry(word).or_default() += 1;
        }
    }
    let mut long: Vec<(&str, usize)> = counts.into_iter().filter(|(w, _)| w.len() >= 8).collect();
    long.sort_by_key(|&(w, n)| (std::cmp::Reverse(n), w));
    let common = long[0].0.to_string();
    // "q" starts few words (the syllable "qu" is the second rarest).
    let rare = long
        .iter()
        .rev()
        .find(|(w, _)| w.starts_with("qu"))
        .unwrap()
        .0
        .to_string();
    let _ = &data.words;
    (common, rare)
}

#[test]
#[ignore = "benchmark; run explicitly in release mode"]
fn bench_suggest_record_and_refresh() {
    let dir = TempDir::new("bench");
    let data = generate();
    // MERCURY_BENCH_DB=<path> keeps the database there and reuses it on the next run.
    let path = std::env::var_os("MERCURY_BENCH_DB").map_or_else(|| dir.db(), Into::into);
    let reuse = path.exists();
    let mut store = Store::open(&path).unwrap();
    let start = Instant::now();
    if !reuse {
        load(&mut store, &data);
    }
    println!(
        "loaded {} pages, {} visits, {} bookmarks in {:.1} s",
        data.pages.len(),
        data.visits.len(),
        data.bookmarks.len(),
        start.elapsed().as_secs_f64()
    );

    let (common, rare) = common_and_rare(&data);
    println!("| call | median ms | worst ms | results |");
    println!("| --- | --- | --- | --- |");
    for (kind, word) in [("common", &common), ("rare", &rare)] {
        for n in [1, 2, 4, 8] {
            let input = &word[..n];
            let results = store.suggest(input, 10, NOW).unwrap().len();
            let (median, worst) = time(|_| {
                store.suggest(input, 10, NOW).unwrap();
            });
            println!(
                "| suggest {kind} {n} `{input}` | {} | {} | {results} |",
                ms(median),
                ms(worst)
            );
        }
    }
    let input = "zqxjv";
    let results = store.suggest(input, 10, NOW).unwrap().len();
    let (median, worst) = time(|_| {
        store.suggest(input, 10, NOW).unwrap();
    });
    println!(
        "| suggest no match `{input}` | {} | {} | {results} |",
        ms(median),
        ms(worst)
    );

    let (median, worst) = time(|i| {
        let page = &data.pages[(i * 7919) % data.pages.len()];
        store
            .record_visit(
                &page.0,
                Some(&page.1),
                VisitTransition::Link,
                NOW + i as i64,
            )
            .unwrap();
    });
    println!("| record_visit | {} | {} | |", ms(median), ms(worst));

    let (median, worst) = time(|_| {
        store.refresh_frecencies(NOW, 1000).unwrap();
    });
    println!(
        "| refresh_frecencies(1000) | {} | {} | |",
        ms(median),
        ms(worst)
    );
}
