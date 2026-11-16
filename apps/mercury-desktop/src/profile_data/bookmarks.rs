//! Bookmark requests: reading and editing the tree. New nodes get a fresh GUID ([`new_guid`]).
//! Each reply runs on the UI thread with `Result<T, ProfileDataError>`.

#![cfg_attr(
    not(test),
    allow(dead_code, reason = "called by the bookmarks and import work")
)]

use mercury_store::{BookmarkId, BookmarkNode, BookmarkRoot};

use super::{ProfileData, ProfileDataError};

/// A new globally unique identifier for a bookmark or download: 12 URL-safe Base64 characters
/// from 9 random bytes, the form Firefox uses. `None` if the system's random source failed.
pub(crate) fn new_guid() -> Option<String> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut bytes = [0_u8; 9];
    getrandom::fill(&mut bytes).ok()?;
    let mut guid = String::with_capacity(12);
    for chunk in bytes.chunks(3) {
        let bits = u32::from(chunk[0]) << 16 | u32::from(chunk[1]) << 8 | u32::from(chunk[2]);
        for shift in [18, 12, 6, 0] {
            guid.push(char::from(ALPHABET[(bits >> shift & 63) as usize]));
        }
    }
    Some(guid)
}

/// `new_guid`, or the store's "invalid argument" error that a reply can carry.
pub(super) fn guid_or_error() -> mercury_store::Result<String> {
    new_guid().ok_or(mercury_store::StoreError::InvalidArgument(
        "the system's random source is unavailable",
    ))
}

impl ProfileData {
    /// The id of a root folder.
    pub(crate) fn bookmark_root(
        &self,
        root: BookmarkRoot,
        reply: impl FnOnce(Result<BookmarkId, ProfileDataError>) + 'static,
    ) {
        self.request(move |db, _| db.store().bookmark_root(root), reply);
    }

    /// The children of a folder, by position.
    pub(crate) fn bookmark_children(
        &self,
        folder: BookmarkId,
        reply: impl FnOnce(Result<Vec<BookmarkNode>, ProfileDataError>) + 'static,
    ) {
        self.request(move |db, _| db.store().bookmark_children(folder), reply);
    }

    /// One node.
    pub(crate) fn bookmark(
        &self,
        id: BookmarkId,
        reply: impl FnOnce(Result<BookmarkNode, ProfileDataError>) + 'static,
    ) {
        self.request(move |db, _| db.store().bookmark(id), reply);
    }

    /// The first bookmark of exactly this URL, if any (the star button).
    pub(crate) fn bookmark_for_url(
        &self,
        url: String,
        reply: impl FnOnce(Result<Option<BookmarkId>, ProfileDataError>) + 'static,
    ) {
        self.request(move |db, _| db.store().bookmark_for_url(&url), reply);
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
            move |db, now| {
                let guid = guid_or_error()?;
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
            move |db, now| {
                let guid = guid_or_error()?;
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
            move |db, now| {
                let guid = guid_or_error()?;
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
            move |db, now| db.store().rename_bookmark(id, &title, now),
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
            move |db, now| db.store().set_bookmark_url(id, &url, now),
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
            move |db, now| db.store().move_bookmark(id, parent, position, now),
            reply,
        );
    }

    /// Removes a node and everything in it.
    pub(crate) fn remove_bookmark(
        &self,
        id: BookmarkId,
        reply: impl FnOnce(Result<(), ProfileDataError>) + 'static,
    ) {
        self.request(move |db, now| db.store().remove_bookmark(id, now), reply);
    }
}
