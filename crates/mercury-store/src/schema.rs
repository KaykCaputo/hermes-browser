//! Connection setup and the `PRAGMA user_version` migration ladder.
//!
//! Adapted in part from Zephium, <https://github.com/zephium-browser/Zephium>, commit
//! `9026a1fffad74d6e201ef97fe8c02c1b01403cd2`, `crates/zephium-store/src/migrations.rs:1944-1966`
//! (the FTS5 mirror and its insert/delete/update triggers), MPL-2.0.

use rusqlite::Connection;

use crate::error::{Result, StoreError};

/// Migration `n` (1-based) brings the schema from version `n - 1` to `n`.
const MIGRATIONS: &[&str] = &[
    // Version 1: the schema of research report R2 §7.3.
    "CREATE TABLE places (
        id            INTEGER PRIMARY KEY,
        url           TEXT NOT NULL UNIQUE,
        title         TEXT,
        visit_count   INTEGER NOT NULL DEFAULT 0,
        typed_count   INTEGER NOT NULL DEFAULT 0,
        last_visit_at INTEGER,
        frecency      INTEGER NOT NULL DEFAULT -1,
        hidden        INTEGER NOT NULL DEFAULT 0,
        url_key       TEXT NOT NULL,             -- lower-case URL without scheme and www.
        frecency_at   INTEGER,                   -- when `frecency` was computed
        title_at      INTEGER                    -- when the visit that gave `title` happened
    ) STRICT;
    CREATE INDEX places_by_url_key  ON places(url_key, frecency);
    CREATE INDEX places_by_frecency ON places(frecency);
    CREATE INDEX places_frecency_by_id ON places(id, frecency);

    CREATE TABLE visits (
        id         INTEGER PRIMARY KEY,
        place_id   INTEGER NOT NULL REFERENCES places(id) ON DELETE CASCADE,
        visited_at INTEGER NOT NULL,
        transition INTEGER NOT NULL
    ) STRICT;
    CREATE INDEX visits_by_place ON visits(place_id, visited_at);
    CREATE INDEX visits_by_date  ON visits(visited_at);

    CREATE TABLE bookmarks (
        id          INTEGER PRIMARY KEY,
        guid        TEXT NOT NULL UNIQUE,
        kind        INTEGER NOT NULL,
        parent_id   INTEGER REFERENCES bookmarks(id) ON DELETE CASCADE,
        position    INTEGER NOT NULL,
        title       TEXT,
        place_id    INTEGER REFERENCES places(id),
        added_at    INTEGER NOT NULL,
        modified_at INTEGER NOT NULL
    ) STRICT;
    CREATE INDEX bookmarks_by_parent ON bookmarks(parent_id, position);
    CREATE INDEX bookmarks_by_place  ON bookmarks(place_id);

    CREATE TABLE downloads (
        id             INTEGER PRIMARY KEY,
        guid           TEXT NOT NULL UNIQUE,
        url            TEXT NOT NULL,
        final_url      TEXT,
        referrer       TEXT,
        target_path    TEXT NOT NULL,
        temp_path      TEXT,
        mime_type      TEXT,
        total_bytes    INTEGER,
        received_bytes INTEGER NOT NULL DEFAULT 0,
        state          INTEGER NOT NULL,
        error          TEXT,
        etag           TEXT,
        last_modified  TEXT,
        started_at     INTEGER NOT NULL,
        ended_at       INTEGER
    ) STRICT;

    CREATE VIRTUAL TABLE places_fts USING fts5(
        url, title, content='places', content_rowid='id', prefix='1 2 3'
    );
    CREATE TRIGGER places_fts_ai AFTER INSERT ON places BEGIN
        INSERT INTO places_fts(rowid, url, title) VALUES (new.id, new.url, new.title);
    END;
    CREATE TRIGGER places_fts_ad AFTER DELETE ON places BEGIN
        INSERT INTO places_fts(places_fts, rowid, url, title)
        VALUES ('delete', old.id, old.url, old.title);
    END;
    CREATE TRIGGER places_fts_au AFTER UPDATE OF url, title ON places BEGIN
        INSERT INTO places_fts(places_fts, rowid, url, title)
        VALUES ('delete', old.id, old.url, old.title);
        INSERT INTO places_fts(rowid, url, title) VALUES (new.id, new.url, new.title);
    END;

    CREATE VIRTUAL TABLE bookmarks_fts USING fts5(
        title, content='bookmarks', content_rowid='id', prefix='1 2 3'
    );
    CREATE TRIGGER bookmarks_fts_ai AFTER INSERT ON bookmarks BEGIN
        INSERT INTO bookmarks_fts(rowid, title) VALUES (new.id, new.title);
    END;
    CREATE TRIGGER bookmarks_fts_ad AFTER DELETE ON bookmarks BEGIN
        INSERT INTO bookmarks_fts(bookmarks_fts, rowid, title) VALUES ('delete', old.id, old.title);
    END;
    CREATE TRIGGER bookmarks_fts_au AFTER UPDATE OF title ON bookmarks BEGIN
        INSERT INTO bookmarks_fts(bookmarks_fts, rowid, title) VALUES ('delete', old.id, old.title);
        INSERT INTO bookmarks_fts(rowid, title) VALUES (new.id, new.title);
    END;

    -- The roots, after the triggers so that the full-text mirror includes them.
    INSERT INTO bookmarks (guid, kind, parent_id, position, title, place_id, added_at, modified_at)
    VALUES ('toolbar_____', 2, NULL, 0, 'Bookmarks Bar', NULL, 0, 0),
           ('menu________', 2, NULL, 1, 'Other Bookmarks', NULL, 0, 0);",
];

/// The newest schema version this build understands.
pub(crate) const CURRENT_VERSION: u32 = MIGRATIONS.len() as u32;

/// Reads `PRAGMA user_version`.
pub(crate) fn user_version(conn: &Connection) -> Result<u32> {
    Ok(conn.pragma_query_value(None, "user_version", |row| row.get(0))?)
}

/// Configures the connection and migrates the schema to the newest version.
///
/// The version is checked before anything is written, so a file from a newer build is left
/// exactly as it was (not even switched to WAL).
pub(crate) fn prepare(conn: &mut Connection) -> Result<()> {
    let found = user_version(conn)?;
    if found > CURRENT_VERSION {
        return Err(StoreError::UnsupportedSchemaVersion {
            found,
            supported: CURRENT_VERSION,
        });
    }
    let journal_mode: String =
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))?;
    if !journal_mode.eq_ignore_ascii_case("wal") && !journal_mode.eq_ignore_ascii_case("memory") {
        log::warn!("profile database is in {journal_mode} journal mode, not WAL");
    }
    conn.pragma_update(None, "foreign_keys", true)?;

    for (index, migration) in MIGRATIONS.iter().enumerate().skip(found as usize) {
        let version = index as u32 + 1;
        let tx = conn.transaction()?;
        tx.execute_batch(migration)?;
        tx.pragma_update(None, "user_version", version)?;
        tx.commit()?;
        log::info!("profile database migrated to schema version {version}");
    }
    Ok(())
}
