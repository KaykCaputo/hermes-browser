//! Address-bar suggestions and the upkeep of stored frecency scores.
//!
//! # Cost of a call
//!
//! The input is first limited: more than [`MAX_SEARCH_INPUT_BYTES`](crate::MAX_SEARCH_INPUT_BYTES)
//! bytes finds nothing, and only the first [`MAX_SEARCH_WORDS`](crate::MAX_SEARCH_WORDS) words
//! are used. Then:
//! - **Constant-bounded:** the best-pages source reads exactly [`BEST_PAGES_EXAMINED`] rows; the
//!   URL-key source reads at most [`INDEX_ROWS_EXAMINED`] index entries; re-scoring reads at most
//!   `4 × window` candidates. None of these depends on how many pages match.
//! - **Linear in the matches of the typed prefix, with a small constant:** the two full-text
//!   sources. Their `LIMIT` caps the rows they return, not the work: for a last word longer than
//!   the prefix indexes (four characters or more) FTS5 merges the postings of every term with
//!   that prefix before producing a row, and words that rarely occur together walk both
//!   posting lists. Measured by the reviewer: about 1 ms per 50,000 matching pages, so 10–15 ms
//!   at a million pages.

use std::collections::BTreeSet;

use rusqlite::{Connection, OptionalExtension, params};

use crate::Store;
use crate::error::{Result, StoreError};
use crate::frecency::{self, DEFAULT_FRECENCY_SETTINGS};
use crate::search::{
    MAX_SEARCH_INPUT_BYTES, fts_query, is_token_char, search_terms, url_key, words,
};

/// The most suggestions one call returns; a larger `limit` is clamped to it.
pub const MAX_SUGGESTIONS: u32 = 20;

/// How many candidates [`Store::suggest`] takes from each source per result asked for.
pub const CANDIDATES_PER_RESULT: u32 = 4;

/// How many pages with the best stored scores [`Store::suggest`] examines for every input.
pub const BEST_PAGES_EXAMINED: u32 = 1_000;

/// The most rows the URL-key source examines, and the most matches each full-text source
/// returns (see the module documentation for what that does and does not bound).
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
/// order, then the best `?4` of those by stored score.
const BY_URL_KEY_SQL: &str = "
    SELECT id FROM (
        SELECT id, frecency FROM places INDEXED BY places_by_url_key
        WHERE url_key >= ?1 AND url_key < ?2
        LIMIT ?3
    )
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
           (SELECT count(*) FROM bookmarks b WHERE b.place_id = p.id),
           p.last_visit_at, p.visit_count
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

/// Whether every input word (in lower case) occurs among the words of `texts`, the last input
/// word as a prefix and the others whole. This approximates the full-text rule: it ignores
/// phrase adjacency, and it compares lower-cased text without folding diacritics, which the
/// tokenizer does (so `cafe` matches `Café` in the full-text sources but not here).
fn words_match(input_words: &[String], texts: &[&str]) -> bool {
    let Some((last, others)) = input_words.split_last() else {
        return false;
    };
    let mut found_last = false;
    let mut found_others = vec![false; others.len()];
    for text in texts {
        for token in text
            .split(|c: char| !is_token_char(c))
            .filter(|t| !t.is_empty())
        {
            let lower;
            let token = if token.is_ascii() {
                if !token.bytes().any(|b| b.is_ascii_uppercase()) {
                    token
                } else {
                    lower = token.to_ascii_lowercase();
                    lower.as_str()
                }
            } else {
                lower = token.to_lowercase();
                lower.as_str()
            };
            found_last |= token.starts_with(last.as_str());
            for (found, word) in found_others.iter_mut().zip(others) {
                *found |= token == word;
            }
        }
    }
    found_last && found_others.into_iter().all(|f| f)
}

/// Source 1: of the [`BEST_PAGES_EXAMINED`] best stored scores, the pages whose URL contains
/// `needle` (ASCII lower case; ASCII case-insensitive) or whose words match `input_words`.
fn best_pages(
    conn: &Connection,
    needle: &str,
    input_words: &[String],
    window: u32,
) -> Result<Vec<i64>> {
    let mut found = Vec::new();
    let mut url_lower = String::new();
    let mut stmt = conn.prepare_cached(BEST_PAGES_SQL)?;
    let mut rows = stmt.query([BEST_PAGES_EXAMINED])?;
    while let Some(row) = rows.next()? {
        if found.len() >= window as usize {
            break;
        }
        let text = |index| -> rusqlite::Result<&str> {
            Ok(row.get_ref(index)?.as_str_or_null()?.unwrap_or(""))
        };
        url_lower.clear();
        url_lower.push_str(text(1)?);
        url_lower.make_ascii_lowercase();
        // `str::contains` is linear in the URL and the needle (two-way search).
        let matched = (!needle.is_empty() && url_lower.contains(needle))
            || words_match(input_words, &[text(2)?, text(3)?, text(4)?]);
        if matched {
            found.push(row.get(0)?);
        }
    }
    Ok(found)
}

/// Source 2: pages whose URL key starts with `key`.
fn by_url_key(conn: &Connection, key: &str, window: u32) -> Result<Vec<i64>> {
    if key.is_empty() {
        return Ok(Vec::new());
    }
    let upper = format!("{key}\u{10FFFF}");
    Ok(conn
        .prepare_cached(BY_URL_KEY_SQL)?
        .query_map(params![key, upper, INDEX_ROWS_EXAMINED, window], |row| {
            row.get(0)
        })?
        .collect::<rusqlite::Result<_>>()?)
}

/// Source 3: pages whose URL key and title match `fts`.
fn by_page_text(conn: &Connection, fts: &str, window: u32) -> Result<Vec<i64>> {
    Ok(conn
        .prepare_cached(BY_PAGE_TEXT_SQL)?
        .query_map(params![fts, INDEX_ROWS_EXAMINED, window], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}

/// Source 4: pages bookmarked under a title that matches `fts`.
fn by_bookmark_title(conn: &Connection, fts: &str, window: u32) -> Result<Vec<i64>> {
    Ok(conn
        .prepare_cached(BY_BOOKMARK_TITLE_SQL)?
        .query_map(params![fts, INDEX_ROWS_EXAMINED, window], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}

impl Store {
    /// Address-bar suggestions from history and bookmarks, best first. At most
    /// `min(limit, MAX_SUGGESTIONS)` are returned.
    ///
    /// **Input.** Input longer than [`MAX_SEARCH_INPUT_BYTES`](crate::MAX_SEARCH_INPUT_BYTES)
    /// returns nothing, before any source runs. Words beyond
    /// [`MAX_SEARCH_WORDS`](crate::MAX_SEARCH_WORDS) are ignored. Each word is normalised the
    /// way the index was built (scheme, user info and a leading `www.` removed), so a pasted
    /// URL finds its page, and a typed `www.` is ignored as the scheme is (`www.exam` finds
    /// `https://example.com/` too). The scheme alone (`https`) is no page's word; it still
    /// finds the best pages through their URLs (source 1).
    ///
    /// **Sources.** Each contributes at most
    /// `window = min(limit, MAX_SUGGESTIONS) × CANDIDATES_PER_RESULT` pages, best stored score
    /// first, ties to the newest row:
    /// 1. **The best pages**: the [`BEST_PAGES_EXAMINED`] pages with the highest stored scores,
    ///    matched in memory: the URL contains the input (ASCII case-insensitive), or every
    ///    input word occurs among the words of the URL key, the page title or its bookmark's
    ///    title (the last word as a prefix).
    /// 2. **URL-key prefix**: pages whose URL, without scheme, user info and a leading `www.`,
    ///    with its host in Unicode, starts with the input (`exam` finds
    ///    `https://www.example.com/`, `bü` finds `https://xn--bcher-kva.example/`); at most
    ///    [`INDEX_ROWS_EXAMINED`] pages in key order.
    /// 3. **Page full text**: pages whose URL key and title contain every input word (the
    ///    last as a prefix); at most [`INDEX_ROWS_EXAMINED`] matches, newest pages first.
    /// 4. **Bookmark-title full text**: likewise over bookmark titles.
    ///
    /// The cost of sources 1 and 2 and of re-scoring is constant; sources 3 and 4 are linear
    /// in the number of pages matching the typed words, with a small constant (see the module
    /// documentation).
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
        if input.is_empty() || limit == 0 || input.len() > MAX_SEARCH_INPUT_BYTES {
            return Ok(Vec::new());
        }
        let Some(terms) = search_terms(input) else {
            return Ok(Vec::new());
        };
        let window = limit * CANDIDATES_PER_RESULT;
        let mut candidates = Candidates::default();

        let input_words: Vec<String> = terms.iter().flat_map(|t| words(t)).collect();
        candidates.add(best_pages(
            &self.conn,
            &input.to_ascii_lowercase(),
            &input_words,
            window,
        )?);

        candidates.add(by_url_key(&self.conn, &url_key(input, true), window)?);

        if let Some(fts) = fts_query(input) {
            candidates.add(by_page_text(&self.conn, &fts, window)?);
            candidates.add(by_bookmark_title(&self.conn, &fts, window)?);
        }

        let mut ranked: Vec<(Suggestion, Option<i64>)> = Vec::with_capacity(candidates.ids.len());
        let mut candidate = self.conn.prepare_cached(CANDIDATE_SQL)?;
        for id in candidates.ids {
            let row = candidate
                .query_row([id], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get::<_, i64>(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                })
                .optional()?;
            let Some((url, title, bookmarks, last_visit_at, visit_count)) = row else {
                continue;
            };
            let frecency = frecency::calculate_frecency_from_counts(
                &self.conn,
                &DEFAULT_FRECENCY_SETTINGS,
                id,
                visit_count,
                bookmarks,
                now_ms,
            )?;
            ranked.push((
                Suggestion {
                    url,
                    title,
                    is_bookmark: bookmarks > 0,
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
    /// first, and returns how many it re-scored. `max_places` must be at least 1: 0 does
    /// nothing and fails with [`StoreError::InvalidArgument`], which also ends a caller's loop.
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
        if max_places == 0 {
            return Err(StoreError::InvalidArgument("max_places must be at least 1"));
        }
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
    use super::*;
    use crate::BookmarkRoot;

    fn w(s: &[&str]) -> Vec<String> {
        s.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn words_match_approximates_the_full_text_rule() {
        let texts = ["example.org/news", "Daily NEWS", ""];
        assert!(words_match(&w(&["daily", "ne"]), &texts));
        assert!(words_match(&w(&["exa"]), &texts));
        assert!(!words_match(&w(&["dai", "news"]), &texts));
        assert!(!words_match(&w(&[]), &texts));
        assert!(words_match(&w(&["bücher"]), &["Bücher und Mehr"]));
        // No diacritic folding here, unlike the tokenizer.
        assert!(!words_match(&w(&["cafe"]), &["Café"]));
    }

    /// Ten never-visited bookmarks, all scored 140, added in order 0…9; each source must pick
    /// the newest rows first among equal scores.
    fn ties() -> (Store, Vec<String>) {
        let mut store = Store::open_in_memory().unwrap();
        let toolbar = store.bookmark_root(BookmarkRoot::Toolbar).unwrap();
        let urls: Vec<String> = (0..10)
            .map(|i| format!("https://tie-{i}.example/"))
            .collect();
        for (i, url) in urls.iter().enumerate() {
            store
                .add_bookmark(
                    toolbar,
                    None,
                    &format!("Tie {i}"),
                    url,
                    &format!("g-{i}"),
                    0,
                )
                .unwrap();
        }
        (store, urls)
    }

    fn urls_of(store: &Store, ids: Vec<i64>) -> Vec<String> {
        ids.into_iter()
            .map(|id| {
                store
                    .conn
                    .query_row("SELECT url FROM places WHERE id = ?1", [id], |row| {
                        row.get(0)
                    })
                    .unwrap()
            })
            .collect()
    }

    #[test]
    fn every_source_breaks_ties_to_the_newest_row() {
        let (store, urls) = ties();
        let newest_four: Vec<String> = urls.iter().rev().take(4).cloned().collect();
        let fts = fts_query("tie").unwrap();
        let by_source = [
            (
                "best pages",
                best_pages(&store.conn, "tie-", &w(&["tie"]), 4).unwrap(),
            ),
            ("URL key", by_url_key(&store.conn, "tie-", 4).unwrap()),
            ("page text", by_page_text(&store.conn, &fts, 4).unwrap()),
            (
                "bookmark title",
                by_bookmark_title(&store.conn, &fts, 4).unwrap(),
            ),
        ];
        for (name, ids) in by_source {
            assert_eq!(urls_of(&store, ids), newest_four, "{name}");
        }
    }
}
