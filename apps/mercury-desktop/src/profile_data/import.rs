//! Bulk import from another browser: all or nothing, one transaction per request.

#![cfg_attr(not(test), allow(dead_code, reason = "called by the import work"))]

use mercury_store::{BookmarkId, BookmarkImport, VisitImport, VisitTransition};

use super::{ProfileData, ProfileDataError};

/// One visit to import (an owned `mercury_store::VisitImport`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ImportVisit {
    pub(crate) url: String,
    pub(crate) title: Option<String>,
    pub(crate) visited_at_ms: i64,
    pub(crate) transition: VisitTransition,
}

impl ProfileData {
    /// Adds the visits; replies with how many. A refused entry fails the whole request with
    /// `StoreError::ImportEntryRefused`, naming it.
    pub(crate) fn import_visits(
        &self,
        visits: Vec<ImportVisit>,
        reply: impl FnOnce(Result<u64, ProfileDataError>) + 'static,
    ) {
        self.request(
            move |db, now| {
                let visits = visits
                    .iter()
                    .map(|visit| VisitImport {
                        url: &visit.url,
                        title: visit.title.as_deref(),
                        visited_at_ms: visit.visited_at_ms,
                        transition: visit.transition,
                    })
                    .collect::<Vec<_>>();
                db.store().import_visits(&visits, now)
            },
            reply,
        );
    }

    /// Appends each tree under its parent folder, in one transaction; replies with how many
    /// nodes were added. The trees carry their own GUIDs (make them with `new_guid`).
    pub(crate) fn import_bookmarks(
        &self,
        sets: Vec<(BookmarkId, Vec<BookmarkImport>)>,
        reply: impl FnOnce(Result<u64, ProfileDataError>) + 'static,
    ) {
        self.request(
            move |db, now| {
                let sets = sets
                    .iter()
                    .map(|(parent, nodes)| (*parent, nodes.as_slice()))
                    .collect::<Vec<_>>();
                db.store().import_bookmark_sets(&sets, now)
            },
            reply,
        );
    }
}
