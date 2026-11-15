//! Bookmarks: a tree of folders, bookmarks and separators under two fixed roots.

use rusqlite::{Connection, OptionalExtension, params};

use crate::Store;
use crate::error::{Result, StoreError};
use crate::frecency;
use crate::history::refresh_place;

const KIND_BOOKMARK: i64 = 1;
const KIND_FOLDER: i64 = 2;
const KIND_SEPARATOR: i64 = 3;

/// The node query, as a literal so it can be extended with `concat!` (SQL is never formatted).
macro_rules! node_sql {
    () => {
        "SELECT b.id, b.guid, b.parent_id, b.position, b.title, b.kind, p.url, b.added_at,
                b.modified_at
         FROM bookmarks b LEFT JOIN places p ON p.id = b.place_id"
    };
}

/// Identifies one bookmark, folder or separator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BookmarkId(pub i64);

/// What a bookmark node is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BookmarkKind {
    /// A link to a page.
    Bookmark {
        /// The page's URL.
        url: String,
    },
    /// A folder that holds other nodes.
    Folder,
    /// A separator line.
    Separator,
}

/// One node of the bookmark tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookmarkNode {
    /// The node.
    pub id: BookmarkId,
    /// Its globally unique identifier.
    pub guid: String,
    /// The folder that holds it; `None` for the roots.
    pub parent: Option<BookmarkId>,
    /// Its index among its siblings (dense, from 0).
    pub position: u32,
    /// Its title; empty for separators.
    pub title: String,
    /// What it is.
    pub kind: BookmarkKind,
    /// When it was added, in milliseconds since the Unix epoch.
    pub added_at_ms: i64,
    /// When it was last renamed, re-pointed or moved, in milliseconds since the Unix epoch.
    pub modified_at_ms: i64,
}

/// The two fixed top-level folders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BookmarkRoot {
    /// "Bookmarks Bar", GUID `toolbar_____`.
    Toolbar,
    /// "Other Bookmarks", GUID `menu________`.
    Other,
}

impl Store {
    /// The id of a root folder.
    pub fn bookmark_root(&self, root: BookmarkRoot) -> Result<BookmarkId> {
        let guid = match root {
            BookmarkRoot::Toolbar => "toolbar_____",
            BookmarkRoot::Other => "menu________",
        };
        self.conn
            .prepare_cached("SELECT id FROM bookmarks WHERE guid = ?1 AND parent_id IS NULL")?
            .query_row([guid], |row| row.get(0))
            .optional()?
            .map(BookmarkId)
            .ok_or(StoreError::NotFound)
    }

    /// The children of a folder, ordered by position.
    pub fn bookmark_children(&self, folder: BookmarkId) -> Result<Vec<BookmarkNode>> {
        require_folder(&self.conn, folder)?;
        let mut stmt = self.conn.prepare_cached(concat!(
            node_sql!(),
            " WHERE b.parent_id = ?1 ORDER BY b.position"
        ))?;
        let nodes = stmt
            .query_map([folder.0], node_from_row)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(nodes)
    }

    /// One node.
    pub fn bookmark(&self, id: BookmarkId) -> Result<BookmarkNode> {
        self.conn
            .prepare_cached(concat!(node_sql!(), " WHERE b.id = ?1"))?
            .query_row([id.0], node_from_row)
            .optional()?
            .ok_or(StoreError::NotFound)
    }

    /// Adds a bookmark. `position: None` appends. `guid` must be unique.
    pub fn add_bookmark(
        &mut self,
        parent: BookmarkId,
        position: Option<u32>,
        title: &str,
        url: &str,
        guid: &str,
        now_ms: i64,
    ) -> Result<BookmarkId> {
        self.insert_node(
            parent,
            position,
            KIND_BOOKMARK,
            Some(title),
            Some(url),
            guid,
            now_ms,
        )
    }

    /// Adds a folder. `position: None` appends. `guid` must be unique.
    pub fn add_bookmark_folder(
        &mut self,
        parent: BookmarkId,
        position: Option<u32>,
        title: &str,
        guid: &str,
        now_ms: i64,
    ) -> Result<BookmarkId> {
        self.insert_node(
            parent,
            position,
            KIND_FOLDER,
            Some(title),
            None,
            guid,
            now_ms,
        )
    }

    /// Adds a separator. `position: None` appends. `guid` must be unique.
    pub fn add_bookmark_separator(
        &mut self,
        parent: BookmarkId,
        position: Option<u32>,
        guid: &str,
        now_ms: i64,
    ) -> Result<BookmarkId> {
        self.insert_node(parent, position, KIND_SEPARATOR, None, None, guid, now_ms)
    }

    /// Renames a node.
    pub fn rename_bookmark(&mut self, id: BookmarkId, title: &str, now_ms: i64) -> Result<()> {
        let changed = self
            .conn
            .prepare_cached("UPDATE bookmarks SET title = ?2, modified_at = ?3 WHERE id = ?1")?
            .execute(params![id.0, title, now_ms])?;
        if changed == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    /// Points a bookmark at another URL. Folders and separators have no URL.
    pub fn set_bookmark_url(&mut self, id: BookmarkId, url: &str, now_ms: i64) -> Result<()> {
        let tx = self.conn.transaction()?;
        let node = node_info(&tx, id)?;
        let (KIND_BOOKMARK, Some(old_place)) = (node.kind, node.place_id) else {
            return Err(StoreError::InvalidArgument("only a bookmark has a URL"));
        };
        let new_place = ensure_place(&tx, url)?;
        tx.prepare_cached("UPDATE bookmarks SET place_id = ?2, modified_at = ?3 WHERE id = ?1")?
            .execute(params![id.0, new_place, now_ms])?;
        refresh_place(&tx, old_place, now_ms)?;
        if new_place != old_place {
            frecency::update_frecency(&tx, new_place, now_ms)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Moves within or between folders, keeping sibling positions dense (0..n). Roots cannot be
    /// moved; a folder cannot be moved into itself or its descendants.
    ///
    /// `position` is the node's index in the destination folder after the move; `None` or a
    /// position past the end appends.
    pub fn move_bookmark(
        &mut self,
        id: BookmarkId,
        new_parent: BookmarkId,
        position: Option<u32>,
        now_ms: i64,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        let node = node_info(&tx, id)?;
        let Some(old_parent) = node.parent_id else {
            return Err(StoreError::CannotModifyRoot);
        };
        require_folder(&tx, new_parent)?;
        let would_cycle: bool = tx
            .prepare_cached(
                "WITH RECURSIVE ancestors(id) AS (
                     SELECT ?1
                     UNION ALL
                     SELECT b.parent_id FROM bookmarks b JOIN ancestors a ON b.id = a.id
                     WHERE b.parent_id IS NOT NULL
                 )
                 SELECT EXISTS (SELECT 1 FROM ancestors WHERE id = ?2)",
            )?
            .query_row(params![new_parent.0, id.0], |row| row.get(0))?;
        if would_cycle {
            return Err(StoreError::InvalidMove);
        }

        close_gap(&tx, old_parent, node.position)?;
        let siblings: i64 = tx
            .prepare_cached("SELECT count(*) FROM bookmarks WHERE parent_id = ?1 AND id != ?2")?
            .query_row(params![new_parent.0, id.0], |row| row.get(0))?;
        let target = position.map_or(siblings, |p| i64::from(p).min(siblings));
        tx.prepare_cached(
            "UPDATE bookmarks SET position = position + 1
             WHERE parent_id = ?1 AND position >= ?2 AND id != ?3",
        )?
        .execute(params![new_parent.0, target, id.0])?;
        tx.prepare_cached(
            "UPDATE bookmarks SET parent_id = ?2, position = ?3, modified_at = ?4 WHERE id = ?1",
        )?
        .execute(params![id.0, new_parent.0, target, now_ms])?;
        tx.commit()?;
        Ok(())
    }

    /// Removes a node and, for a folder, everything in it. Roots cannot be removed.
    ///
    /// A page that is left with neither visits nor bookmarks is removed too.
    pub fn remove_bookmark(&mut self, id: BookmarkId, now_ms: i64) -> Result<()> {
        let tx = self.conn.transaction()?;
        let node = node_info(&tx, id)?;
        let Some(parent) = node.parent_id else {
            return Err(StoreError::CannotModifyRoot);
        };
        let places: Vec<i64> = tx
            .prepare_cached(
                "WITH RECURSIVE subtree(id) AS (
                     SELECT ?1
                     UNION ALL
                     SELECT b.id FROM bookmarks b JOIN subtree s ON b.parent_id = s.id
                 )
                 SELECT DISTINCT place_id FROM bookmarks
                 WHERE id IN subtree AND place_id IS NOT NULL",
            )?
            .query_map([id.0], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        tx.prepare_cached("DELETE FROM bookmarks WHERE id = ?1")?
            .execute([id.0])?;
        close_gap(&tx, parent, node.position)?;
        for place_id in places {
            refresh_place(&tx, place_id, now_ms)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The first bookmark for this exact URL, if any (drives the star button).
    pub fn bookmark_for_url(&self, url: &str) -> Result<Option<BookmarkId>> {
        Ok(self
            .conn
            .prepare_cached(
                "SELECT b.id FROM bookmarks b JOIN places p ON p.id = b.place_id
                 WHERE p.url = ?1 ORDER BY b.id LIMIT 1",
            )?
            .query_row([url], |row| row.get(0))
            .optional()?
            .map(BookmarkId))
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_node(
        &mut self,
        parent: BookmarkId,
        position: Option<u32>,
        kind: i64,
        title: Option<&str>,
        url: Option<&str>,
        guid: &str,
        now_ms: i64,
    ) -> Result<BookmarkId> {
        let tx = self.conn.transaction()?;
        require_folder(&tx, parent)?;
        let taken: bool = tx
            .prepare_cached("SELECT EXISTS (SELECT 1 FROM bookmarks WHERE guid = ?1)")?
            .query_row([guid], |row| row.get(0))?;
        if taken {
            return Err(StoreError::DuplicateGuid(guid.to_owned()));
        }
        let siblings: i64 = tx
            .prepare_cached("SELECT count(*) FROM bookmarks WHERE parent_id = ?1")?
            .query_row([parent.0], |row| row.get(0))?;
        let target = position.map_or(siblings, |p| i64::from(p).min(siblings));
        tx.prepare_cached(
            "UPDATE bookmarks SET position = position + 1 WHERE parent_id = ?1 AND position >= ?2",
        )?
        .execute(params![parent.0, target])?;
        let place_id = url.map(|url| ensure_place(&tx, url)).transpose()?;
        tx.prepare_cached(
            "INSERT INTO bookmarks
                 (guid, kind, parent_id, position, title, place_id, added_at, modified_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
        )?
        .execute(params![
            guid, kind, parent.0, target, title, place_id, now_ms
        ])?;
        let id = tx.last_insert_rowid();
        if let Some(place_id) = place_id {
            frecency::update_frecency(&tx, place_id, now_ms)?;
        }
        tx.commit()?;
        Ok(BookmarkId(id))
    }
}

/// The columns of a node needed to edit the tree.
struct NodeInfo {
    kind: i64,
    parent_id: Option<i64>,
    position: i64,
    place_id: Option<i64>,
}

fn node_info(conn: &Connection, id: BookmarkId) -> Result<NodeInfo> {
    conn.prepare_cached("SELECT kind, parent_id, position, place_id FROM bookmarks WHERE id = ?1")?
        .query_row([id.0], |row| {
            Ok(NodeInfo {
                kind: row.get(0)?,
                parent_id: row.get(1)?,
                position: row.get(2)?,
                place_id: row.get(3)?,
            })
        })
        .optional()?
        .ok_or(StoreError::NotFound)
}

fn require_folder(conn: &Connection, id: BookmarkId) -> Result<()> {
    let kind: Option<i64> = conn
        .prepare_cached("SELECT kind FROM bookmarks WHERE id = ?1")?
        .query_row([id.0], |row| row.get(0))
        .optional()?;
    match kind {
        None => Err(StoreError::NotFound),
        Some(KIND_FOLDER) => Ok(()),
        Some(_) => Err(StoreError::NotAFolder),
    }
}

/// Shifts the siblings after `position` in `parent` down by one.
fn close_gap(conn: &Connection, parent: i64, position: i64) -> Result<()> {
    conn.prepare_cached(
        "UPDATE bookmarks SET position = position - 1 WHERE parent_id = ?1 AND position > ?2",
    )?
    .execute(params![parent, position])?;
    Ok(())
}

/// The id of the page for `url`, creating an unvisited page if needed.
fn ensure_place(conn: &Connection, url: &str) -> Result<i64> {
    conn.prepare_cached("INSERT INTO places (url) VALUES (?1) ON CONFLICT (url) DO NOTHING")?
        .execute([url])?;
    Ok(conn
        .prepare_cached("SELECT id FROM places WHERE url = ?1")?
        .query_row([url], |row| row.get(0))?)
}

fn node_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<BookmarkNode> {
    let kind = match (row.get::<_, i64>(5)?, row.get::<_, Option<String>>(6)?) {
        (KIND_BOOKMARK, Some(url)) => BookmarkKind::Bookmark { url },
        (KIND_FOLDER, _) => BookmarkKind::Folder,
        (KIND_SEPARATOR, _) => BookmarkKind::Separator,
        (kind, _) => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                5,
                rusqlite::types::Type::Integer,
                format!("invalid bookmark kind {kind}").into(),
            ));
        }
    };
    Ok(BookmarkNode {
        id: BookmarkId(row.get(0)?),
        guid: row.get(1)?,
        parent: row.get::<_, Option<i64>>(2)?.map(BookmarkId),
        position: row.get(3)?,
        title: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
        kind,
        added_at_ms: row.get(7)?,
        modified_at_ms: row.get(8)?,
    })
}
