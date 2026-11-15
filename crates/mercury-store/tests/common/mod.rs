#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// Milliseconds in one day.
pub const DAY_MS: i64 = 86_400_000;

/// A fixed "now" for tests: 2026-11-15T00:00:00Z.
pub const NOW: i64 = 1_794_700_800_000;

/// A directory under the system temporary directory that is removed on drop.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("mercury-store-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        Self(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn db(&self) -> PathBuf {
        self.0.join("places.sqlite")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A store on a temporary file plus a second, plain connection for inspecting rows that the
/// public API does not expose.
pub struct Fixture {
    pub store: mercury_store::Store,
    pub raw: rusqlite::Connection,
    _dir: TempDir,
}

/// The counters of one `places` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub title: Option<String>,
    pub visit_count: i64,
    pub typed_count: i64,
    pub last_visit_at: Option<i64>,
    pub frecency: i64,
}

impl Fixture {
    pub fn new(name: &str) -> Self {
        let dir = TempDir::new(name);
        let store = mercury_store::Store::open(&dir.db()).expect("open store");
        let raw = rusqlite::Connection::open(dir.db()).expect("open raw");
        Self {
            store,
            raw,
            _dir: dir,
        }
    }

    pub fn place(&self, url: &str) -> Option<Place> {
        use rusqlite::OptionalExtension;
        self.raw
            .query_row(
                "SELECT title, visit_count, typed_count, last_visit_at, frecency
                 FROM places WHERE url = ?1",
                [url],
                |r| {
                    Ok(Place {
                        title: r.get(0)?,
                        visit_count: r.get(1)?,
                        typed_count: r.get(2)?,
                        last_visit_at: r.get(3)?,
                        frecency: r.get(4)?,
                    })
                },
            )
            .optional()
            .expect("query place")
    }

    pub fn count(&self, table_sql: &str) -> i64 {
        self.raw
            .query_row(table_sql, [], |r| r.get(0))
            .expect("count")
    }
}
