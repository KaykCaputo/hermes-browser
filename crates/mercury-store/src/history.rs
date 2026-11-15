//! Browsing history: pages, visits, full-text search and address-bar suggestions.

use rusqlite::{Connection, OptionalExtension, params};

use crate::Store;
use crate::error::Result;
use crate::frecency;
use crate::search::{fts_query, url_key};

/// How the user reached a page. Uses Firefox's numbering (`nsINavHistoryService.idl`).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VisitTransition {
    /// Followed a link.
    Link = 1,
    /// Typed into the address bar.
    Typed = 2,
    /// Opened from a bookmark.
    Bookmark = 3,
    /// Reloaded.
    Reload = 9,
}

impl VisitTransition {
    /// The transition stored as `value`, if it is one Mercury records.
    pub(crate) fn from_i64(value: i64) -> Option<Self> {
        match value {
            1 => Some(Self::Link),
            2 => Some(Self::Typed),
            3 => Some(Self::Bookmark),
            9 => Some(Self::Reload),
            _ => None,
        }
    }
}

/// Identifies one visit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VisitId(pub i64);

/// One visit, as listed in the history panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    /// The visit.
    pub visit_id: VisitId,
    /// The page's URL.
    pub url: String,
    /// The page's current title, if known.
    pub title: Option<String>,
    /// When the visit happened, in milliseconds since the Unix epoch.
    pub visited_at_ms: i64,
    /// How the page was reached.
    pub transition: VisitTransition,
}

impl HistoryEntry {
    /// The cursor that continues a listing after this entry.
    pub fn cursor(&self) -> HistoryCursor {
        HistoryCursor {
            visited_at_ms: self.visited_at_ms,
            visit_id: self.visit_id,
        }
    }
}

/// A position in the history listing, which is ordered by visit time and then visit id, both
/// descending. [`Store::history`] lists the visits strictly after it in that order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HistoryCursor {
    /// The visit time of the last entry already listed.
    pub visited_at_ms: i64,
    /// The id of the last entry already listed.
    pub visit_id: VisitId,
}

/// Which visits [`Store::clear_history`] removes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClearRange {
    /// Every visit.
    All,
    /// Visits at or after `from_ms` (milliseconds since the Unix epoch).
    Since {
        /// The start of the range, inclusive.
        from_ms: i64,
    },
}

const HISTORY_SQL: &str = "
    SELECT v.id, p.url, p.title, v.visited_at, v.transition
    FROM visits v JOIN places p ON p.id = v.place_id
    WHERE (v.visited_at, v.id) < (?1, ?2)
    ORDER BY v.visited_at DESC, v.id DESC
    LIMIT ?3";

const HISTORY_SEARCH_SQL: &str = "
    SELECT v.id, p.url, p.title, v.visited_at, v.transition
    FROM visits v JOIN places p ON p.id = v.place_id
    WHERE (v.visited_at, v.id) < (?1, ?2)
      AND v.place_id IN (SELECT rowid FROM places_fts WHERE places_fts MATCH ?4)
    ORDER BY v.visited_at DESC, v.id DESC
    LIMIT ?3";

impl Store {
    /// Records a visit to `url` at `now_ms`, creating the page if needed. A `title` replaces the
    /// stored one; `None` leaves it unchanged.
    pub fn record_visit(
        &mut self,
        url: &str,
        title: Option<&str>,
        transition: VisitTransition,
        now_ms: i64,
    ) -> Result<VisitId> {
        let tx = self.conn.transaction()?;
        tx.prepare_cached(
            "INSERT INTO places (url, title, url_key) VALUES (?1, ?2, ?3)
             ON CONFLICT (url) DO UPDATE SET title = excluded.title
             WHERE excluded.title IS NOT NULL AND excluded.title IS NOT places.title",
        )?
        .execute(params![url, title, url_key(url, true)])?;
        let place_id: i64 = tx
            .prepare_cached("SELECT id FROM places WHERE url = ?1")?
            .query_row([url], |row| row.get(0))?;
        tx.prepare_cached(
            "INSERT INTO visits (place_id, visited_at, transition) VALUES (?1, ?2, ?3)",
        )?
        .execute(params![place_id, now_ms, transition as u8])?;
        let visit_id = tx.last_insert_rowid();
        tx.prepare_cached(
            "UPDATE places SET visit_count = visit_count + 1,
                               typed_count = typed_count + ?2,
                               last_visit_at = max(coalesce(last_visit_at, ?3), ?3)
             WHERE id = ?1",
        )?
        .execute(params![
            place_id,
            i64::from(transition == VisitTransition::Typed),
            now_ms
        ])?;
        frecency::update_frecency(&tx, place_id, now_ms)?;
        tx.commit()?;
        Ok(VisitId(visit_id))
    }

    /// Sets the title of a known page. Returns `false` if the URL is unknown.
    pub fn set_page_title(&mut self, url: &str, title: &str) -> Result<bool> {
        let changed = self
            .conn
            .prepare_cached("UPDATE places SET title = ?2 WHERE url = ?1")?
            .execute(params![url, title])?;
        Ok(changed > 0)
    }

    /// Newest first. `query` filters by URL or title (full-text, prefix match on the last word).
    ///
    /// With a `before` cursor (see [`HistoryEntry::cursor`]), only the visits after it in this
    /// order are listed, so paging never skips or repeats visits. An empty or blank `query`
    /// lists everything; one with no searchable words matches nothing. Characters with a meaning
    /// in FTS5 query syntax are never interpreted.
    pub fn history(
        &self,
        query: Option<&str>,
        before: Option<HistoryCursor>,
        limit: u32,
    ) -> Result<Vec<HistoryEntry>> {
        let (before, before_id) =
            before.map_or((i64::MAX, i64::MAX), |c| (c.visited_at_ms, c.visit_id.0));
        let query = query.filter(|q| !q.trim().is_empty());
        let mut stmt;
        let rows = match query {
            None => {
                stmt = self.conn.prepare_cached(HISTORY_SQL)?;
                stmt.query(params![before, before_id, limit])?
            }
            Some(query) => {
                let Some(fts) = fts_query(query) else {
                    return Ok(Vec::new());
                };
                stmt = self.conn.prepare_cached(HISTORY_SEARCH_SQL)?;
                stmt.query(params![before, before_id, limit, fts])?
            }
        };
        rows.mapped(|row| {
            Ok(HistoryEntry {
                visit_id: VisitId(row.get(0)?),
                url: row.get(1)?,
                title: row.get(2)?,
                visited_at_ms: row.get(3)?,
                transition: transition_column(row, 4)?,
            })
        })
        .collect::<rusqlite::Result<_>>()
        .map_err(Into::into)
    }

    /// Deletes one visit. Returns `false` if it did not exist.
    pub fn delete_visit(&mut self, id: VisitId, now_ms: i64) -> Result<bool> {
        let tx = self.conn.transaction()?;
        let place_id: Option<i64> = tx
            .prepare_cached("SELECT place_id FROM visits WHERE id = ?1")?
            .query_row([id.0], |row| row.get(0))
            .optional()?;
        let Some(place_id) = place_id else {
            return Ok(false);
        };
        tx.prepare_cached("DELETE FROM visits WHERE id = ?1")?
            .execute([id.0])?;
        refresh_place(&tx, place_id, now_ms)?;
        tx.commit()?;
        Ok(true)
    }

    /// Deletes all visits of one URL. Returns `false` if there were none. A bookmarked page is
    /// kept, with no visits.
    pub fn delete_page(&mut self, url: &str, now_ms: i64) -> Result<bool> {
        let tx = self.conn.transaction()?;
        let place_id: Option<i64> = tx
            .prepare_cached("SELECT id FROM places WHERE url = ?1")?
            .query_row([url], |row| row.get(0))
            .optional()?;
        let Some(place_id) = place_id else {
            return Ok(false);
        };
        let removed = tx
            .prepare_cached("DELETE FROM visits WHERE place_id = ?1")?
            .execute([place_id])?;
        refresh_place(&tx, place_id, now_ms)?;
        tx.commit()?;
        Ok(removed > 0)
    }

    /// Deletes the visits in `range` and returns how many were removed. Pages left with no
    /// visits are removed unless bookmarked.
    pub fn clear_history(&mut self, range: ClearRange, now_ms: i64) -> Result<u64> {
        let from = match range {
            ClearRange::All => i64::MIN,
            ClearRange::Since { from_ms } => from_ms,
        };
        let tx = self.conn.transaction()?;
        let places: Vec<i64> = tx
            .prepare_cached("SELECT DISTINCT place_id FROM visits WHERE visited_at >= ?1")?
            .query_map([from], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let removed = tx
            .prepare_cached("DELETE FROM visits WHERE visited_at >= ?1")?
            .execute([from])?;
        for place_id in places {
            refresh_place(&tx, place_id, now_ms)?;
        }
        tx.commit()?;
        Ok(removed as u64)
    }
}

/// Reads a visit transition column.
fn transition_column(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<VisitTransition> {
    let value: i64 = row.get(index)?;
    VisitTransition::from_i64(value).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Integer,
            format!("unknown visit transition {value}").into(),
        )
    })
}

/// Brings a page's counters, last-visit time and frecency in line with its remaining visits and
/// bookmarks, and removes it if it has neither.
pub(crate) fn refresh_place(conn: &Connection, place_id: i64, now_ms: i64) -> Result<()> {
    let (visits, typed, last_visit_at): (i64, i64, Option<i64>) = conn
        .prepare_cached(
            "SELECT count(*), coalesce(sum(transition = 2), 0), max(visited_at)
             FROM visits WHERE place_id = ?1",
        )?
        .query_row([place_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
    let bookmarks: i64 = conn
        .prepare_cached("SELECT count(*) FROM bookmarks WHERE place_id = ?1")?
        .query_row([place_id], |row| row.get(0))?;
    if visits == 0 && bookmarks == 0 {
        conn.prepare_cached("DELETE FROM places WHERE id = ?1")?
            .execute([place_id])?;
        return Ok(());
    }
    conn.prepare_cached(
        "UPDATE places SET visit_count = ?2, typed_count = ?3, last_visit_at = ?4 WHERE id = ?1",
    )?
    .execute(params![place_id, visits, typed, last_visit_at])?;
    frecency::update_frecency(conn, place_id, now_ms)
}
