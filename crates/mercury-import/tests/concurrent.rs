//! A browser writing while it is imported from. A writer thread commits continuously to a fixture
//! database while the importer reads it in a loop; every read must fail or be consistent.
//!
//! The writer's transaction `n` adds the page `https://n{n}.example/` and one visit to it at
//! time `n`, and rewrites some filler rows spread through the file. A consistent read therefore
//! returns exactly the visits `K, K-1, …, 1` for some `K`, and `K` never goes backwards between
//! reads. A copy that mixed two states could drop visits whose page row it did not see, which
//! breaks the sequence.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mercury_import::{ImportError, ImportedVisit, read_chromium_history, read_firefox_history};
use rusqlite::{Connection, params};

const READ_FOR: Duration = Duration::from_secs(2);
const FILLER_ROWS: i64 = 400;
const BURST: Duration = Duration::from_millis(40);
const GAP: Duration = Duration::from_millis(40);

struct Layout {
    pragmas: &'static str,
    schema: &'static str,
    add_page: &'static str,
    add_visit: &'static str,
    /// The stored time of visit `n`, which the reader returns as `n` milliseconds.
    time: fn(i64) -> i64,
}

const FIREFOX: Layout = Layout {
    pragmas: "PRAGMA journal_mode = WAL; PRAGMA wal_autocheckpoint = 8; PRAGMA synchronous = OFF;",
    schema: "CREATE TABLE moz_places (id INTEGER PRIMARY KEY, url LONGVARCHAR, title LONGVARCHAR);
             CREATE TABLE moz_historyvisits (id INTEGER PRIMARY KEY, place_id INTEGER,
                 visit_date INTEGER);",
    add_page: "INSERT INTO moz_places (id, url, title) VALUES (?1, ?2, 'Page')",
    add_visit: "INSERT INTO moz_historyvisits (place_id, visit_date) VALUES (?1, ?2)",
    time: |n| n * 1000,
};

const CHROMIUM: Layout = Layout {
    pragmas: "PRAGMA journal_mode = DELETE; PRAGMA synchronous = OFF;",
    schema: "CREATE TABLE urls (id INTEGER PRIMARY KEY, url LONGVARCHAR, title LONGVARCHAR);
             CREATE TABLE visits (id INTEGER PRIMARY KEY, url INTEGER NOT NULL,
                 visit_time INTEGER NOT NULL);",
    add_page: "INSERT INTO urls (id, url, title) VALUES (?1, ?2, 'Page')",
    add_visit: "INSERT INTO visits (url, visit_time) VALUES (?1, ?2)",
    time: |n| (n + 11_644_473_600_000) * 1000,
};

fn create(path: &Path, layout: &Layout) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(layout.pragmas).unwrap();
    conn.execute_batch(layout.schema).unwrap();
    conn.execute_batch(&format!(
        "CREATE TABLE filler (id INTEGER PRIMARY KEY, body BLOB);
         WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < {FILLER_ROWS})
         INSERT INTO filler SELECT i, randomblob(2000) FROM n;"
    ))
    .unwrap();
}

/// Commits transactions until `stop`: back to back for `BURST`, then quiet for `GAP`, as a
/// browser writes in bursts. The bursts make copies race with commits and checkpoints; the gaps
/// let a careful reader find a moment when nothing changes.
fn write_until(path: &Path, layout: &'static Layout, stop: &AtomicBool) -> i64 {
    let mut conn = Connection::open(path).unwrap();
    conn.execute_batch(layout.pragmas).unwrap();
    let mut n = 0;
    let mut burst_started = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        if burst_started.elapsed() > BURST {
            std::thread::sleep(GAP);
            burst_started = Instant::now();
        }
        n += 1;
        let tx = conn.transaction().unwrap();
        tx.execute(
            layout.add_page,
            params![n, format!("https://n{n}.example/")],
        )
        .unwrap();
        tx.execute(layout.add_visit, params![n, (layout.time)(n)])
            .unwrap();
        for row in [1 + n % FILLER_ROWS, FILLER_ROWS - n % FILLER_ROWS] {
            tx.execute(
                "UPDATE filler SET body = randomblob(2000) WHERE id = ?1",
                [row],
            )
            .unwrap();
        }
        tx.commit().unwrap();
    }
    n
}

fn check_consistent(visits: &[ImportedVisit], previous: &mut usize) {
    let newest = visits.len();
    for (index, visit) in visits.iter().enumerate() {
        let n = (newest - index) as i64;
        assert_eq!(visit.visited_at_ms, n, "visit {index} of {newest}");
        assert_eq!(visit.url, format!("https://n{n}.example/"));
    }
    assert!(newest >= *previous, "went back from {previous} to {newest}");
    *previous = newest;
}

fn read_while_writing(
    layout: &'static Layout,
    file: &str,
    read: fn(&Path, usize) -> Result<Vec<ImportedVisit>, ImportError>,
) {
    let profile = tempfile::tempdir().unwrap();
    let path = profile.path().join(file);
    create(&path, layout);
    let stop = Arc::new(AtomicBool::new(false));
    let writer = {
        let (path, stop) = (path.clone(), Arc::clone(&stop));
        std::thread::spawn(move || write_until(&path, layout, &stop))
    };

    let (mut oks, mut errors, mut previous) = (0, Vec::new(), 0);
    let started = Instant::now();
    while started.elapsed() < READ_FOR {
        match read(profile.path(), usize::MAX) {
            Ok(visits) => {
                check_consistent(&visits, &mut previous);
                oks += 1;
            }
            Err(error) => errors.push(error.to_string()),
        }
    }
    stop.store(true, Ordering::Relaxed);
    let written = writer.join().unwrap();

    eprintln!(
        "{file}: {written} transactions written, {oks} consistent reads (last saw {previous}), \
         {} errors: {:?}",
        errors.len(),
        errors.iter().take(3).collect::<Vec<_>>()
    );
    assert!(
        oks > 0,
        "at least one read succeeds while the browser writes"
    );
}

#[test]
fn reading_firefox_history_while_it_is_written_is_consistent_or_an_error() {
    read_while_writing(&FIREFOX, "places.sqlite", read_firefox_history);
}

#[test]
fn reading_chromium_history_while_it_is_written_is_consistent_or_an_error() {
    read_while_writing(&CHROMIUM, "History", read_chromium_history);
}
