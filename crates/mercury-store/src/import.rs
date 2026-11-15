//! Bulk import of visits and bookmarks from another browser.

use std::collections::BTreeMap;

use rusqlite::params;

use crate::Store;
use crate::bookmarks::{BookmarkId, KIND_BOOKMARK, KIND_FOLDER, KIND_SEPARATOR, require_folder};
use crate::error::{Result, StoreError};
use crate::frecency;
use crate::history::{VisitTransition, ensure_place, insert_visit, offer_title, refresh_place};
use crate::limits::{MAX_TITLE_CHARS, cut};

/// The deepest bookmark tree [`Store::import_bookmarks`] accepts; the nodes passed directly are
/// at depth 1.
pub const BOOKMARK_IMPORT_MAX_DEPTH: usize = 100;

/// One visit to import.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisitImport<'a> {
    /// The page's URL.
    pub url: &'a str,
    /// The page's title, if known.
    pub title: Option<&'a str>,
    /// When the visit happened, in milliseconds since the Unix epoch.
    pub visited_at_ms: i64,
    /// How the page was reached.
    pub transition: VisitTransition,
}

/// One node of a bookmark tree to import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BookmarkImport {
    /// A folder and its contents.
    Folder {
        /// Its title.
        title: String,
        /// Its globally unique identifier.
        guid: String,
        /// When it was added; `None` means the time of the import.
        added_at_ms: Option<i64>,
        /// Its contents, in order.
        children: Vec<BookmarkImport>,
    },
    /// A bookmark.
    Bookmark {
        /// Its title.
        title: String,
        /// The page's URL.
        url: String,
        /// Its globally unique identifier.
        guid: String,
        /// When it was added; `None` means the time of the import.
        added_at_ms: Option<i64>,
    },
    /// A separator.
    Separator {
        /// Its globally unique identifier.
        guid: String,
    },
}

impl Store {
    /// Adds many visits in one transaction; frecency is recomputed once per affected page, as of
    /// `now_ms`. All or nothing. Returns the number of visits added.
    ///
    /// A page's title is that of its most recent titled visit, across the visits already stored
    /// and those imported; the order of `visits` does not matter (for titled visits at the same
    /// millisecond, the greater title wins). Visit counts, typed counts and last-visit times are
    /// computed from the visit rows.
    pub fn import_visits(&mut self, visits: &[VisitImport<'_>], now_ms: i64) -> Result<u64> {
        let tx = self.conn.transaction()?;
        // Per page: its newest titled visit in the import (ties: the greater title, so that
        // the slice order never matters).
        let mut pages: BTreeMap<i64, Option<(i64, &str)>> = BTreeMap::new();
        for visit in visits {
            let place_id = ensure_place(&tx, visit.url)?;
            insert_visit(&tx, place_id, visit.visited_at_ms, visit.transition)?;
            let newest = pages.entry(place_id).or_default();
            if let Some(title) = visit.title {
                let offered = (visit.visited_at_ms, title);
                if newest.is_none_or(|current| offered > current) {
                    *newest = Some(offered);
                }
            }
        }
        for (place_id, newest) in pages {
            if let Some((seen_at, title)) = newest {
                offer_title(&tx, place_id, title, seen_at)?;
            }
            refresh_place(&tx, place_id, now_ms)?;
        }
        tx.commit()?;
        Ok(visits.len() as u64)
    }

    /// Appends a tree under `parent` in one transaction, keeping positions dense. All or
    /// nothing. Returns the number of nodes added.
    ///
    /// `added_at_ms: None` means `now_ms`; every node's `modified_at` is `now_ms`. The tree is
    /// walked without recursion; a node deeper than [`BOOKMARK_IMPORT_MAX_DEPTH`] (the nodes in
    /// `nodes` are at depth 1) fails the call with [`StoreError::InvalidArgument`]. A GUID that
    /// is already in use, or used twice in `nodes`, fails it with [`StoreError::DuplicateGuid`].
    pub fn import_bookmarks(
        &mut self,
        parent: BookmarkId,
        nodes: &[BookmarkImport],
        now_ms: i64,
    ) -> Result<u64> {
        struct Frame<'n> {
            parent: i64,
            nodes: std::slice::Iter<'n, BookmarkImport>,
            position: i64,
            depth: usize,
        }

        let tx = self.conn.transaction()?;
        require_folder(&tx, parent)?;
        let start: i64 = tx
            .prepare_cached("SELECT count(*) FROM bookmarks WHERE parent_id = ?1")?
            .query_row([parent.0], |row| row.get(0))?;
        let mut stack = vec![Frame {
            parent: parent.0,
            nodes: nodes.iter(),
            position: start,
            depth: 1,
        }];
        let mut added = 0u64;
        let mut places = Vec::new();
        while let Some(frame) = stack.last_mut() {
            let Some(node) = frame.nodes.next() else {
                stack.pop();
                continue;
            };
            let (parent_id, position, depth) = (frame.parent, frame.position, frame.depth);
            frame.position += 1;

            let (kind, title, url, guid, added_at, children) = match node {
                BookmarkImport::Folder {
                    title,
                    guid,
                    added_at_ms,
                    children,
                } => (
                    KIND_FOLDER,
                    Some(title),
                    None,
                    guid,
                    *added_at_ms,
                    Some(children),
                ),
                BookmarkImport::Bookmark {
                    title,
                    url,
                    guid,
                    added_at_ms,
                } => (
                    KIND_BOOKMARK,
                    Some(title),
                    Some(url),
                    guid,
                    *added_at_ms,
                    None,
                ),
                BookmarkImport::Separator { guid } => {
                    (KIND_SEPARATOR, None, None, guid, None, None)
                }
            };
            let taken: bool = tx
                .prepare_cached("SELECT EXISTS (SELECT 1 FROM bookmarks WHERE guid = ?1)")?
                .query_row([guid], |row| row.get(0))?;
            if taken {
                return Err(StoreError::DuplicateGuid(guid.clone()));
            }
            let place_id = url.map(|url| ensure_place(&tx, url)).transpose()?;
            tx.prepare_cached(
                "INSERT INTO bookmarks
                     (guid, kind, parent_id, position, title, place_id, added_at, modified_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?
            .execute(params![
                guid,
                kind,
                parent_id,
                position,
                title.map(|t| cut(t, MAX_TITLE_CHARS)),
                place_id,
                added_at.unwrap_or(now_ms),
                now_ms
            ])?;
            let id = tx.last_insert_rowid();
            added += 1;
            places.extend(place_id);

            if let Some(children) = children.filter(|c| !c.is_empty()) {
                if depth + 1 > BOOKMARK_IMPORT_MAX_DEPTH {
                    return Err(StoreError::InvalidArgument(
                        "the bookmark tree is nested too deeply",
                    ));
                }
                stack.push(Frame {
                    parent: id,
                    nodes: children.iter(),
                    position: 0,
                    depth: depth + 1,
                });
            }
        }
        places.sort_unstable();
        places.dedup();
        for place_id in places {
            frecency::update_frecency(&tx, place_id, now_ms)?;
        }
        tx.commit()?;
        Ok(added)
    }
}
