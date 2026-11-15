//! What the readers log about rows they leave out: rows that could not be read at `warn`,
//! ordinary omissions (visits without a usable time, refused addresses) at `debug`.

use std::path::Path;
use std::sync::{Mutex, Once};

use log::{Level, LevelFilter, Log, Metadata, Record};
use mercury_import::{read_chromium_history, read_firefox_bookmarks};
use rusqlite::Connection;

struct Capture(Mutex<Vec<(Level, String)>>);

impl Log for Capture {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &Record<'_>) {
        if record.target().starts_with("mercury_import") {
            self.0
                .lock()
                .unwrap()
                .push((record.level(), record.args().to_string()));
        }
    }

    fn flush(&self) {}
}

static CAPTURE: Capture = Capture(Mutex::new(Vec::new()));

/// The records that name `dir`, so tests running at the same time do not see each other's.
fn records_about(dir: &Path) -> Vec<(Level, String)> {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        log::set_logger(&CAPTURE).unwrap();
        log::set_max_level(LevelFilter::Trace);
    });
    let dir = dir.display().to_string();
    CAPTURE
        .0
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, message)| message.contains(&dir))
        .cloned()
        .collect()
}

fn levels_and_counts(records: &[(Level, String)]) -> Vec<(Level, String)> {
    records
        .iter()
        .map(|(level, message)| {
            let tail = message
                .rsplit_once(": ")
                .map_or(message.as_str(), |(_, tail)| tail);
            (*level, tail.to_owned())
        })
        .collect()
}

#[test]
fn unreadable_history_rows_warn_and_visits_without_a_time_do_not() {
    let profile = tempfile::tempdir().unwrap();
    records_about(profile.path());
    let conn = Connection::open(profile.path().join("History")).unwrap();
    conn.execute_batch(
        "CREATE TABLE urls (id INTEGER PRIMARY KEY, url LONGVARCHAR, title LONGVARCHAR);
         CREATE TABLE visits (id INTEGER PRIMARY KEY, url INTEGER, visit_time INTEGER);
         INSERT INTO urls VALUES
             (1, 'https://a.example/', 'A'),
             (2, X'68747470733a2f2f00ff', 'Blob address'),
             (3, 'javascript:void(0)', 'Script');
         INSERT INTO visits (url, visit_time) VALUES
             (1, 13344473600000000), (2, 13344473600000001), (1, 0), (1, 1000000),
             (3, 13344473600000002);",
    )
    .unwrap();
    drop(conn);

    assert_eq!(read_chromium_history(profile.path(), 10).unwrap().len(), 1);

    assert_eq!(
        levels_and_counts(&records_about(profile.path())),
        vec![
            (
                Level::Debug,
                "left out 1 visits to addresses that cannot be imported".to_owned()
            ),
            (
                Level::Debug,
                "left out 2 visits without a usable time".to_owned()
            ),
            (
                Level::Warn,
                "skipped 1 history rows that could not be read".to_owned()
            ),
        ]
    );
}

#[test]
fn a_bookmark_whose_address_cannot_be_read_is_counted_like_a_refused_one() {
    let profile = tempfile::tempdir().unwrap();
    records_about(profile.path());
    let conn = Connection::open(profile.path().join("places.sqlite")).unwrap();
    conn.execute_batch(
        "CREATE TABLE moz_places (id INTEGER PRIMARY KEY, url LONGVARCHAR, title LONGVARCHAR);
         CREATE TABLE moz_bookmarks (id INTEGER PRIMARY KEY, type INTEGER, fk INTEGER,
             parent INTEGER, position INTEGER, title LONGVARCHAR, dateAdded INTEGER, guid TEXT);
         INSERT INTO moz_places VALUES
             (1, 'https://a.example/', 'A'), (2, X'ff', 'Blob'), (3, 'place:sort=8', 'Query');
         INSERT INTO moz_bookmarks (id, type, fk, parent, position, title, guid) VALUES
             (1, 2, NULL, 0, 0, '', 'root________'),
             (3, 2, NULL, 1, 1, 'toolbar', 'toolbar_____'),
             (10, 1, 1, 3, 0, 'A', 'a___________'),
             (11, 1, 2, 3, 1, 'Blob', 'b___________'),
             (12, 1, 3, 3, 2, 'Query', 'c___________');",
    )
    .unwrap();
    drop(conn);

    assert_eq!(read_firefox_bookmarks(profile.path()).unwrap().len(), 1);

    assert_eq!(
        levels_and_counts(&records_about(profile.path())),
        vec![(
            Level::Debug,
            "left out 2 bookmarks with addresses that cannot be imported".to_owned()
        )]
    );
}
