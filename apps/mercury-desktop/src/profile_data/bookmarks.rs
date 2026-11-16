//! Bookmark requests: reading and editing the tree. New nodes get a fresh GUID
//! ([`new_guid`](super::new_guid)). Writes notify [`Area::Bookmarks`] with the folders whose
//! listing changed (both folders for a move).
//! Each reply runs on the UI thread with `Result<T, ProfileDataError>`.

#![cfg_attr(
    not(test),
    expect(dead_code, reason = "called by the bookmarks and import work")
)]

use mercury_store::{BookmarkId, BookmarkNode, BookmarkRoot};

use super::{Area, Change, Database, ProfileData, ProfileDataError, guid_or_error};

/// The change hint for editing node `id`: the folder that lists it (the node itself for a
/// root).
fn listing_of(db: &mut Database, id: BookmarkId) -> mercury_store::Result<Change> {
    let node = db.store().bookmark(id)?;
    Ok(Change::folders([node.parent.unwrap_or(id)]))
}

impl ProfileData {
    /// The id of a root folder.
    pub(crate) fn bookmark_root(
        &self,
        root: BookmarkRoot,
        reply: impl FnOnce(Result<BookmarkId, ProfileDataError>) + 'static,
    ) {
        self.request(None, move |db, _| db.store().bookmark_root(root), reply);
    }

    /// The children of a folder, by position.
    pub(crate) fn bookmark_children(
        &self,
        folder: BookmarkId,
        reply: impl FnOnce(Result<Vec<BookmarkNode>, ProfileDataError>) + 'static,
    ) {
        self.request(
            None,
            move |db, _| db.store().bookmark_children(folder),
            reply,
        );
    }

    /// One node.
    pub(crate) fn bookmark(
        &self,
        id: BookmarkId,
        reply: impl FnOnce(Result<BookmarkNode, ProfileDataError>) + 'static,
    ) {
        self.request(None, move |db, _| db.store().bookmark(id), reply);
    }

    /// The first bookmark of exactly this URL, if any (the star button).
    pub(crate) fn bookmark_for_url(
        &self,
        url: String,
        reply: impl FnOnce(Result<Option<BookmarkId>, ProfileDataError>) + 'static,
    ) {
        self.request(None, move |db, _| db.store().bookmark_for_url(&url), reply);
    }

    /// Adds a bookmark in `parent` at `position` (`None` appends).
    pub(crate) fn add_bookmark(
        &self,
        parent: BookmarkId,
        position: Option<u32>,
        title: String,
        url: String,
        reply: impl FnOnce(Result<BookmarkId, ProfileDataError>) + 'static,
    ) {
        self.request(
            Some(Area::Bookmarks),
            move |db, now| {
                let guid = guid_or_error()?;
                db.changed(Change::folders([parent]));
                db.store()
                    .add_bookmark(parent, position, &title, &url, &guid, now)
            },
            reply,
        );
    }

    /// Adds a folder in `parent` at `position` (`None` appends).
    pub(crate) fn add_bookmark_folder(
        &self,
        parent: BookmarkId,
        position: Option<u32>,
        title: String,
        reply: impl FnOnce(Result<BookmarkId, ProfileDataError>) + 'static,
    ) {
        self.request(
            Some(Area::Bookmarks),
            move |db, now| {
                let guid = guid_or_error()?;
                db.changed(Change::folders([parent]));
                db.store()
                    .add_bookmark_folder(parent, position, &title, &guid, now)
            },
            reply,
        );
    }

    /// Adds a separator in `parent` at `position` (`None` appends).
    pub(crate) fn add_bookmark_separator(
        &self,
        parent: BookmarkId,
        position: Option<u32>,
        reply: impl FnOnce(Result<BookmarkId, ProfileDataError>) + 'static,
    ) {
        self.request(
            Some(Area::Bookmarks),
            move |db, now| {
                let guid = guid_or_error()?;
                db.changed(Change::folders([parent]));
                db.store()
                    .add_bookmark_separator(parent, position, &guid, now)
            },
            reply,
        );
    }

    /// Renames a node.
    pub(crate) fn rename_bookmark(
        &self,
        id: BookmarkId,
        title: String,
        reply: impl FnOnce(Result<(), ProfileDataError>) + 'static,
    ) {
        self.request(
            Some(Area::Bookmarks),
            move |db, now| {
                let change = listing_of(db, id)?;
                db.changed(change);
                db.store().rename_bookmark(id, &title, now)
            },
            reply,
        );
    }

    /// Points a bookmark at another URL.
    pub(crate) fn set_bookmark_url(
        &self,
        id: BookmarkId,
        url: String,
        reply: impl FnOnce(Result<(), ProfileDataError>) + 'static,
    ) {
        self.request(
            Some(Area::Bookmarks),
            move |db, now| {
                let change = listing_of(db, id)?;
                db.changed(change);
                db.store().set_bookmark_url(id, &url, now)
            },
            reply,
        );
    }

    /// Moves a node into `parent` at `position` (`None` appends).
    pub(crate) fn move_bookmark(
        &self,
        id: BookmarkId,
        parent: BookmarkId,
        position: Option<u32>,
        reply: impl FnOnce(Result<(), ProfileDataError>) + 'static,
    ) {
        self.request(
            Some(Area::Bookmarks),
            move |db, now| {
                let mut change = listing_of(db, id)?;
                change.merge(Change::folders([parent]));
                db.changed(change);
                db.store().move_bookmark(id, parent, position, now)
            },
            reply,
        );
    }

    /// Removes a node and everything in it.
    pub(crate) fn remove_bookmark(
        &self,
        id: BookmarkId,
        reply: impl FnOnce(Result<(), ProfileDataError>) + 'static,
    ) {
        self.request(
            Some(Area::Bookmarks),
            move |db, now| {
                let change = listing_of(db, id)?;
                db.changed(change);
                db.store().remove_bookmark(id, now)
            },
            reply,
        );
    }
}
