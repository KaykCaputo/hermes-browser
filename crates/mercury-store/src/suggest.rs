//! Address-bar suggestions and the upkeep of stored frecency scores.

use std::collections::HashSet;

use rusqlite::{OptionalExtension, params};

use crate::Store;
use crate::error::Result;
use crate::frecency::{self, DEFAULT_FRECENCY_SETTINGS};
use crate::search::{fts_query, url_key};

/// How many candidates [`Store::suggest`] takes from each source per result asked for.
pub const CANDIDATES_PER_RESULT: u32 = 4;

/// How many of the best-scored pages [`Store::suggest`] searches for a URL substring when the
/// indexed matches do not fill the `limit`.
pub const SUBSTRING_SCAN_ROWS: u32 = 2_000;

/// A stored score computed at least this long before `now_ms` is stale for
/// [`Store::refresh_frecencies`]. The age buckets are whole days, so a score cannot change much
/// sooner.
pub const FRECENCY_REFRESH_AGE_MS: i64 = 86_400_000;

/// One address-bar suggestion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    /// The page's URL.
    pub url: String,
    /// The page's title or, if it has none, its bookmark's title.
    pub title: Option<String>,
    /// Whether the page is bookmarked.
    pub is_bookmark: bool,
    /// The page's frecency as of the `now_ms` given to [`Store::suggest`]; higher ranks first.
    pub frecency: i64,
}

/// Pages whose `url_key` starts with the input (`?1` to `?2`), best stored score first.
const BY_URL_KEY_SQL: &str = "
    SELECT id FROM places INDEXED BY places_by_url_key
    WHERE url_key >= ?1 AND url_key < ?2
    ORDER BY frecency DESC LIMIT ?3";

/// Pages whose URL or title contains every word, best stored score first.
const BY_PAGE_TEXT_SQL: &str = "
    SELECT p.id FROM places_fts JOIN places p ON p.id = places_fts.rowid
    WHERE places_fts MATCH ?1
    ORDER BY p.frecency DESC LIMIT ?2";

/// Pages bookmarked under a title that contains every word, best stored score first.
const BY_BOOKMARK_TITLE_SQL: &str = "
    SELECT p.id FROM bookmarks_fts
    JOIN bookmarks b ON b.id = bookmarks_fts.rowid
    JOIN places p ON p.id = b.place_id
    WHERE bookmarks_fts MATCH ?1
    ORDER BY p.frecency DESC LIMIT ?2";

/// Among the best-scored pages, those whose URL contains the input.
const BY_URL_SUBSTRING_SQL: &str = "
    SELECT id FROM (
        SELECT id, url FROM places INDEXED BY places_by_frecency
        ORDER BY frecency DESC LIMIT ?2
    )
    WHERE instr(lower(url), ?1) > 0
    LIMIT ?3";

const CANDIDATE_SQL: &str = "
    SELECT p.url,
           COALESCE(p.title, (SELECT NULLIF(b.title, '') FROM bookmarks b
                              WHERE b.place_id = p.id ORDER BY b.id LIMIT 1)),
           EXISTS (SELECT 1 FROM bookmarks b WHERE b.place_id = p.id),
           p.last_visit_at
    FROM places p WHERE p.id = ?1 AND p.hidden = 0";

/// Page ids in the order found, without repeats.
#[derive(Default)]
struct Candidates {
    ids: Vec<i64>,
    seen: HashSet<i64>,
}

impl Candidates {
    fn add(&mut self, found: Vec<i64>) {
        for id in found {
            if self.seen.insert(id) {
                self.ids.push(id);
            }
        }
    }
}

impl Store {
    /// Address-bar suggestions from history and bookmarks, best first.
    ///
    /// Candidates come from up to four sources, each walked in order of stored frecency and cut
    /// off after `limit × CANDIDATES_PER_RESULT` pages:
    /// 1. pages whose URL, without its scheme and a leading `www.`, starts with the input
    ///    (`exam` finds `https://www.example.com/`), ASCII case-insensitively;
    /// 2. pages whose URL and title contain every word of the input, the last as a prefix;
    /// 3. pages bookmarked under a title that contains every word of the input, likewise;
    /// 4. only if 1-3 found fewer than `limit` pages: the [`SUBSTRING_SCAN_ROWS`] pages with the
    ///    best stored scores whose URL contains the input anywhere.
    ///
    /// The candidates are then ranked by their frecency as of `now_ms`, then by latest visit,
    /// then by URL, and the first `limit` are returned. Sources 1-3 are served by indexes; the
    /// cost of a call depends on `limit`, not on the size of the history.
    ///
    /// The stored score of a page is computed at its last change and at
    /// [`refresh_frecencies`](Store::refresh_frecencies); as time passes it is higher than the
    /// score as of now. A page whose stored score is too low to enter a source's window cannot
    /// appear even if it would rank higher now; erring that way only hides pages that have
    /// recently been little used. A visit dated after `now_ms` counts as a visit made now (its
    /// age falls in the newest bucket, as in the donor).
    pub fn suggest(&self, input: &str, limit: u32, now_ms: i64) -> Result<Vec<Suggestion>> {
        let input = input.trim();
        if input.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let window = limit.saturating_mul(CANDIDATES_PER_RESULT);
        let mut candidates = Candidates::default();

        let key = url_key(input, false);
        if !key.is_empty() {
            let upper = format!("{key}\u{10FFFF}");
            candidates.add(
                self.conn
                    .prepare_cached(BY_URL_KEY_SQL)?
                    .query_map(params![key, upper, window], |row| row.get(0))?
                    .collect::<rusqlite::Result<_>>()?,
            );
        }
        if let Some(fts) = fts_query(input) {
            for sql in [BY_PAGE_TEXT_SQL, BY_BOOKMARK_TITLE_SQL] {
                candidates.add(
                    self.conn
                        .prepare_cached(sql)?
                        .query_map(params![fts, window], |row| row.get(0))?
                        .collect::<rusqlite::Result<_>>()?,
                );
            }
        }
        if candidates.ids.len() < limit as usize {
            candidates.add(
                self.conn
                    .prepare_cached(BY_URL_SUBSTRING_SQL)?
                    .query_map(
                        params![input.to_ascii_lowercase(), SUBSTRING_SCAN_ROWS, window],
                        |row| row.get(0),
                    )?
                    .collect::<rusqlite::Result<_>>()?,
            );
        }

        let mut ranked: Vec<(Suggestion, Option<i64>)> = Vec::with_capacity(candidates.ids.len());
        let mut candidate = self.conn.prepare_cached(CANDIDATE_SQL)?;
        for id in candidates.ids {
            let row = candidate
                .query_row([id], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })
                .optional()?;
            let Some((url, title, is_bookmark, last_visit_at)) = row else {
                continue;
            };
            let frecency =
                frecency::calculate_frecency(&self.conn, &DEFAULT_FRECENCY_SETTINGS, id, now_ms)?;
            ranked.push((
                Suggestion {
                    url,
                    title,
                    is_bookmark,
                    frecency,
                },
                last_visit_at,
            ));
        }
        ranked.sort_by(|(a, a_last), (b, b_last)| {
            (b.frecency, b_last, &a.url).cmp(&(a.frecency, a_last, &b.url))
        });
        ranked.truncate(limit as usize);
        Ok(ranked.into_iter().map(|(s, _)| s).collect())
    }

    /// Re-scores up to `max_places` pages whose stored frecency was computed at least
    /// [`FRECENCY_REFRESH_AGE_MS`] before `now_ms` (or never), highest stored score first, in one
    /// transaction. Returns how many were re-scored.
    ///
    /// Stored scores only fall with time, and the highest stale ones are those that distort
    /// [`suggest`](Store::suggest) most. Call it at start-up and when idle; repeated calls work
    /// through the history. A score computed after `now_ms` (from a later clock) is not stale.
    pub fn refresh_frecencies(&mut self, now_ms: i64, max_places: u32) -> Result<u64> {
        let tx = self.conn.transaction()?;
        let stale: Vec<i64> = tx
            .prepare_cached(
                "SELECT id FROM places INDEXED BY places_by_frecency
                 WHERE frecency_at IS NULL OR frecency_at <= ?1
                 ORDER BY frecency DESC LIMIT ?2",
            )?
            .query_map(
                params![now_ms.saturating_sub(FRECENCY_REFRESH_AGE_MS), max_places],
                |row| row.get(0),
            )?
            .collect::<rusqlite::Result<_>>()?;
        for &id in &stale {
            frecency::update_frecency(&tx, id, now_ms)?;
        }
        tx.commit()?;
        Ok(stale.len() as u64)
    }
}
