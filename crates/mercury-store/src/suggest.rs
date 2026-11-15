//! Address-bar suggestions and the upkeep of stored frecency scores.

use std::collections::BTreeSet;

use rusqlite::{OptionalExtension, params};

use crate::Store;
use crate::error::Result;
use crate::frecency::{self, DEFAULT_FRECENCY_SETTINGS};
use crate::search::{fts_query, url_key, words};

/// The most suggestions one call returns; a larger `limit` is clamped to it.
pub const MAX_SUGGESTIONS: u32 = 20;

/// How many candidates [`Store::suggest`] takes from each source per result asked for.
pub const CANDIDATES_PER_RESULT: u32 = 4;

/// How many pages with the best stored scores [`Store::suggest`] examines for every input.
pub const BEST_PAGES_EXAMINED: u32 = 2_000;

/// The most rows [`Store::suggest`] examines in each index source (URL-key prefix, page full
/// text, bookmark-title full text), whatever the input matches.
pub const INDEX_ROWS_EXAMINED: u32 = 1_000;

/// A stored score computed at least this long before `now_ms` is stale for
/// [`Store::refresh_frecencies`]. The age buckets are whole days, so a score cannot change much
/// sooner.
pub const FRECENCY_REFRESH_AGE_MS: i64 = 86_400_000;

/// One address-bar suggestion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    /// The page's URL.
    pub url: String,
    /// For a bookmarked page, its bookmark's title (the first bookmark's, if there are
    /// several); otherwise the page's title.
    pub title: Option<String>,
    /// Whether the page is bookmarked.
    pub is_bookmark: bool,
    /// The page's frecency as of the `now_ms` given to [`Store::suggest`]; higher ranks first.
    pub frecency: i64,
}

/// The best stored scores, with what the in-memory match needs.
const BEST_PAGES_SQL: &str = "
    SELECT p.id, p.url, p.url_key, p.title,
           (SELECT b.title FROM bookmarks b WHERE b.place_id = p.id ORDER BY b.id LIMIT 1)
    FROM places p INDEXED BY places_by_frecency
    ORDER BY p.frecency DESC, p.id DESC
    LIMIT ?1";

/// Pages whose `url_key` starts with the input (`?1` to `?2`): at most `?3` rows in key
/// order, then the best `?4` of those by stored score. `?5`, if not null, must also occur in
/// the lower-case URL (used for a typed `www.`).
const BY_URL_KEY_SQL: &str = "
    SELECT id FROM (
        SELECT id, frecency, url FROM places INDEXED BY places_by_url_key
        WHERE url_key >= ?1 AND url_key < ?2
        LIMIT ?3
    )
    WHERE ?5 IS NULL OR instr(lower(url), ?5) > 0
    ORDER BY frecency DESC, id DESC LIMIT ?4";

/// Pages whose URL key and title contain every word: at most `?2` matches, newest pages
/// first, then the best `?3` of those by stored score.
const BY_PAGE_TEXT_SQL: &str = "
    SELECT p.id FROM (
        SELECT rowid AS id FROM places_fts WHERE places_fts MATCH ?1
        ORDER BY rowid DESC LIMIT ?2
    ) m
    JOIN places p INDEXED BY places_frecency_by_id ON p.id = m.id
    ORDER BY p.frecency DESC, p.id DESC LIMIT ?3";

/// Pages bookmarked under a title that contains every word: at most `?2` matching bookmarks,
/// newest first, then the best `?3` pages of those by stored score.
const BY_BOOKMARK_TITLE_SQL: &str = "
    SELECT DISTINCT p.id, p.frecency FROM (
        SELECT rowid AS id FROM bookmarks_fts WHERE bookmarks_fts MATCH ?1
        ORDER BY rowid DESC LIMIT ?2
    ) m
    JOIN bookmarks b ON b.id = m.id
    JOIN places p ON p.id = b.place_id
    ORDER BY p.frecency DESC, p.id DESC LIMIT ?3";

const CANDIDATE_SQL: &str = "
    SELECT p.url,
           COALESCE((SELECT NULLIF(b.title, '') FROM bookmarks b
                     WHERE b.place_id = p.id ORDER BY b.id LIMIT 1), p.title),
           EXISTS (SELECT 1 FROM bookmarks b WHERE b.place_id = p.id),
           p.last_visit_at
    FROM places p WHERE p.id = ?1 AND p.hidden = 0";

/// Page ids in the order found, without repeats.
#[derive(Default)]
struct Candidates {
    ids: Vec<i64>,
    seen: BTreeSet<i64>,
}

impl Candidates {
    fn add(&mut self, found: impl IntoIterator<Item = i64>) {
        for id in found {
            if self.seen.insert(id) {
                self.ids.push(id);
            }
        }
    }
}

/// Whether every input word occurs among `text_words`, the last input word as a prefix and the
/// others whole — the full-text rule, without phrase adjacency.
fn words_match(input_words: &[String], text_words: &[String]) -> bool {
    let Some((last, others)) = input_words.split_last() else {
        return false;
    };
    others.iter().all(|w| text_words.contains(w))
        && text_words.iter().any(|t| t.starts_with(last.as_str()))
}

impl Store {
    /// Address-bar suggestions from history and bookmarks, best first. At most
    /// `min(limit, MAX_SUGGESTIONS)` are returned.
    ///
    /// Every call does a bounded amount of work, whatever the input matches. Candidates come
    /// from four sources, each examining a fixed number of rows and contributing at most
    /// `window = min(limit, MAX_SUGGESTIONS) × CANDIDATES_PER_RESULT` pages (best stored score
    /// first, ties to the newest row):
    /// 1. **The best pages**: the [`BEST_PAGES_EXAMINED`] pages with the highest stored scores,
    ///    matched in memory: the URL contains the input (ASCII case-insensitive), or every
    ///    input word occurs among the words of the URL key, the page title or its bookmark's
    ///    title (the last word as a prefix).
    /// 2. **URL-key prefix**: pages whose URL, without scheme, user info and a leading `www.`,
    ///    with its host in Unicode, starts with the input (`exam` finds
    ///    `https://www.example.com/`, `bü` finds `https://xn--bcher-kva.example/`); at most
    ///    [`INDEX_ROWS_EXAMINED`] pages in key order. A typed `www.` must then also occur in
    ///    the URL.
    /// 3. **Page full text**: pages whose URL key and title contain every input word (the
    ///    last as a prefix); at most [`INDEX_ROWS_EXAMINED`] matches, newest pages first.
    /// 4. **Bookmark-title full text**: likewise over bookmark titles.
    ///
    /// What can be found: any page among the best [`BEST_PAGES_EXAMINED`]; any page whose
    /// URL-key prefix or words match at most [`INDEX_ROWS_EXAMINED`] pages, however low its
    /// score — so a rare word, or a host typed far enough to single it out, always finds its
    /// pages. What cannot: a page outside the best pages that shares the typed prefix or words
    /// with more than [`INDEX_ROWS_EXAMINED`] pages and is not among those examined (key order
    /// for source 2, the newest pages for 3 and 4), or that is outside a source's window.
    /// A URL substring that is neither a key prefix nor a word prefix is found only among the
    /// best pages.
    ///
    /// The candidates are ranked by their frecency as of `now_ms`, then by latest visit, then
    /// by URL. Each source's window is chosen by *stored* score, so rankings are only right if
    /// stored scores are fresh: after time away, a page used today can be kept out of a window
    /// by pages whose stale stored scores are higher, even though it would rank above them.
    /// Run [`refresh_frecencies`](Store::refresh_frecencies) until it returns less than its
    /// `max_places`, at start-up and when idle. A visit dated after `now_ms` counts as a visit
    /// made now (its age falls in the newest bucket, as in the donor).
    pub fn suggest(&self, input: &str, limit: u32, now_ms: i64) -> Result<Vec<Suggestion>> {
        let input = input.trim();
        let limit = limit.min(MAX_SUGGESTIONS);
        if input.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let window = limit * CANDIDATES_PER_RESULT;
        let mut candidates = Candidates::default();

        // 1. The best pages, matched in memory.
        let needle = input.to_ascii_lowercase();
        let input_words: Vec<String> = words(input).collect();
        let mut best = Vec::new();
        {
            let mut stmt = self.conn.prepare_cached(BEST_PAGES_SQL)?;
            let mut rows = stmt.query([BEST_PAGES_EXAMINED])?;
            while let Some(row) = rows.next()? {
                if best.len() >= window as usize {
                    break;
                }
                let url: String = row.get(1)?;
                let matched = url.to_ascii_lowercase().contains(&needle) || {
                    let key: String = row.get(2)?;
                    let title: Option<String> = row.get(3)?;
                    let bookmark_title: Option<String> = row.get(4)?;
                    let text_words: Vec<String> = words(&key)
                        .chain(title.as_deref().map(words).into_iter().flatten())
                        .chain(bookmark_title.as_deref().map(words).into_iter().flatten())
                        .collect();
                    words_match(&input_words, &text_words)
                };
                if matched {
                    best.push(row.get(0)?);
                }
            }
        }
        candidates.add(best);

        // 2. URL-key prefix.
        let mut key = url_key(input, false);
        let mut www = None;
        if let Some(rest) = key.strip_prefix("www.") {
            www = Some(format!("www.{rest}"));
            key = rest.to_owned();
        }
        if !key.is_empty() {
            let upper = format!("{key}\u{10FFFF}");
            candidates.add(
                self.conn
                    .prepare_cached(BY_URL_KEY_SQL)?
                    .query_map(
                        params![key, upper, INDEX_ROWS_EXAMINED, window, www],
                        |row| row.get(0),
                    )?
                    .collect::<rusqlite::Result<Vec<i64>>>()?,
            );
        }

        // 3 and 4. Full text.
        if let Some(fts) = fts_query(input) {
            candidates.add(
                self.conn
                    .prepare_cached(BY_PAGE_TEXT_SQL)?
                    .query_map(params![fts, INDEX_ROWS_EXAMINED, window], |row| row.get(0))?
                    .collect::<rusqlite::Result<Vec<i64>>>()?,
            );
            candidates.add(
                self.conn
                    .prepare_cached(BY_BOOKMARK_TITLE_SQL)?
                    .query_map(params![fts, INDEX_ROWS_EXAMINED, window], |row| row.get(0))?
                    .collect::<rusqlite::Result<Vec<i64>>>()?,
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

    /// Re-scores, in one transaction, up to `max_places` pages whose stored frecency was
    /// computed at least [`FRECENCY_REFRESH_AGE_MS`] before `now_ms`, highest stored score
    /// first, and returns how many it re-scored.
    ///
    /// **What a caller must do.** Call it with the same `now_ms` until it returns less than
    /// `max_places`; then every stored score was computed less than a day before `now_ms`, and
    /// [`suggest`](Store::suggest) ranks as if scores were exact, give or take one day of
    /// decay. Scores go stale again as days pass, so the application runs that loop at
    /// start-up and when idle.
    ///
    /// After a single call that returned `max_places`, the `max_places` highest stale scores
    /// are fresh, but lower stale scores may still keep pages out of suggestion windows.
    /// Finding the stale pages uses an index on the time of each score, so a call costs
    /// little when nothing (or little) is stale. A score computed after `now_ms` (from a later
    /// clock) is not stale.
    pub fn refresh_frecencies(&mut self, now_ms: i64, max_places: u32) -> Result<u64> {
        let tx = self.conn.transaction()?;
        let stale: Vec<i64> = tx
            .prepare_cached(
                "SELECT id FROM places INDEXED BY places_by_frecency_at
                 WHERE frecency_at <= ?1
                 ORDER BY frecency DESC, id DESC LIMIT ?2",
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

#[cfg(test)]
mod tests {
    use super::words_match;

    fn w(s: &[&str]) -> Vec<String> {
        s.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn words_match_like_the_full_text_rule() {
        let text = w(&["daily", "news", "example", "org"]);
        assert!(words_match(&w(&["daily", "ne"]), &text));
        assert!(words_match(&w(&["exa"]), &text));
        assert!(!words_match(&w(&["dai", "news"]), &text));
        assert!(!words_match(&w(&[]), &text));
    }
}
