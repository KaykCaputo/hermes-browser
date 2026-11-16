//! Bulk import from another browser, as stepwise jobs (see `jobs`): chunks of
//! [`IMPORT_CHUNK`] visits, or of whole top-level bookmark nodes up to that many nodes, each
//! chunk one transaction.
//!
//! **Refused entries are skipped.** An entry the store refuses (a URL longer than
//! `MAX_URL_BYTES`, a bookmark nested too deeply, a GUID already in use) is left out (a
//! refused bookmark folder with everything in it) and its chunk is tried again; the summary
//! and the progress count what was skipped. Any other failure stops the import.
//!
//! **Not atomic as a whole; resumable.** A cancel, a shutdown or a failure stops the import
//! after the chunks already committed, which are always a prefix of the input. To resume, call
//! again with the input minus its first [`ImportSummary::resume_from`] entries: visits, or
//! top-level bookmark nodes counted across all the sets in order.
//!
//! **Importers should create the target folder first** (`add_bookmark_folder`, for example
//! "Imported from Firefox") and import the source's top-level children into it: chunks are
//! made of whole top-level nodes, so a single top-level folder holding everything would be one
//! chunk, one long transaction holding up every other request.
//!
//! Chunking does not change what the store keeps: visit counts, typed counts and last-visit
//! times are recomputed from the visit rows, and a page's title is that of its newest titled
//! visit whichever chunk brings it. The one difference is a tie at the same millisecond in
//! two chunks: the later chunk's title wins, where a single call would keep the greater.

#![cfg_attr(not(test), expect(dead_code, reason = "called by the import work"))]

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use mercury_store::{BookmarkId, BookmarkImport, StoreError, VisitImport, VisitTransition};

use super::jobs::{JobHandle, Step, Steps};
use super::{Area, Change, Database, Payload, ProfileData, ProfileDataError};

/// Visits per chunk, and the most bookmark nodes per chunk unless one top-level node is
/// larger. Sized by `import_benchmark_100k_visits` (see the lane report for the figures).
pub(crate) const IMPORT_CHUNK: usize = 500;

/// One visit to import (an owned `mercury_store::VisitImport`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ImportVisit {
    pub(crate) url: String,
    pub(crate) title: Option<String>,
    pub(crate) visited_at_ms: i64,
    pub(crate) transition: VisitTransition,
}

/// After each chunk: entries (visits or bookmark nodes) dealt with so far, imported or
/// skipped, of all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ImportProgress {
    pub(crate) done: u64,
    pub(crate) skipped: u64,
    pub(crate) total: u64,
}

/// How an import ended.
#[derive(Debug)]
pub(crate) enum ImportOutcome {
    Completed,
    Cancelled,
    /// A chunk failed for another reason than a refused entry; nothing of it was imported.
    Failed(ProfileDataError),
}

/// The end of an import.
#[derive(Debug)]
pub(crate) struct ImportSummary {
    /// Entries (visits or bookmark nodes) stored.
    pub(crate) imported: u64,
    /// Entries refused and left out (a refused folder counts with its contents).
    pub(crate) skipped: u64,
    /// All entries given.
    pub(crate) total: u64,
    /// Entries of the input dealt with, as a prefix: visits, or top-level bookmark nodes across
    /// all the sets in order. Resume by leaving out this many.
    pub(crate) resume_from: u64,
    pub(crate) outcome: ImportOutcome,
    /// The longest chunk (with its retries), which is the longest any request waited behind
    /// this import.
    pub(crate) longest_chunk: Duration,
}

/// Counts and timing shared by both kinds of import.
#[derive(Default)]
struct Tally {
    imported: u64,
    skipped: u64,
    total: u64,
    resume_from: u64,
    longest_chunk: Duration,
}

impl Tally {
    fn summary(&self, outcome: ImportOutcome) -> Payload {
        Box::new(ImportSummary {
            imported: self.imported,
            skipped: self.skipped,
            total: self.total,
            resume_from: self.resume_from,
            outcome,
            longest_chunk: self.longest_chunk,
        })
    }

    /// After a chunk that started at `started`: the next step.
    fn after_chunk(&mut self, started: Instant, imported: u64, finished: bool) -> Step {
        self.longest_chunk = self.longest_chunk.max(started.elapsed());
        if finished {
            Step::Finished {
                summary: self.summary(ImportOutcome::Completed),
                wrote: imported > 0,
            }
        } else {
            Step::Continue {
                progress: Some(Box::new(ImportProgress {
                    done: self.imported + self.skipped,
                    skipped: self.skipped,
                    total: self.total,
                })),
                wrote: imported > 0,
            }
        }
    }

    fn failed(&mut self, started: Instant, error: StoreError) -> Step {
        self.longest_chunk = self.longest_chunk.max(started.elapsed());
        Step::Finished {
            summary: self.summary(ImportOutcome::Failed(ProfileDataError::Store(error))),
            wrote: false,
        }
    }

    fn describe(&self, what: &str) -> String {
        format!(
            "import of {what}, {} of {} imported, {} skipped",
            self.imported, self.total, self.skipped
        )
    }
}

struct VisitSteps {
    visits: Vec<ImportVisit>,
    tally: Tally,
}

impl Steps for VisitSteps {
    fn step(&mut self, database: &mut Database, now: i64) -> Step {
        let started = Instant::now();
        let start = self.tally.resume_from as usize;
        let end = (start + IMPORT_CHUNK).min(self.visits.len());
        // The store refuses a visit only for its URL's length: those are left out at once, and
        // the retry below is the fallback.
        let mut left_out: Vec<usize> = (start..end)
            .filter(|&index| self.visits[index].url.len() > mercury_store::MAX_URL_BYTES)
            .collect();
        loop {
            let kept = (start..end)
                .filter(|index| !left_out.contains(index))
                .collect::<Vec<_>>();
            let chunk = kept
                .iter()
                .map(|&index| {
                    let visit = &self.visits[index];
                    VisitImport {
                        url: &visit.url,
                        title: visit.title.as_deref(),
                        visited_at_ms: visit.visited_at_ms,
                        transition: visit.transition,
                    }
                })
                .collect::<Vec<_>>();
            match database.store.import_visits(&chunk, now) {
                Ok(imported) => {
                    self.tally.imported += imported;
                    self.tally.skipped += left_out.len() as u64;
                    self.tally.resume_from = end as u64;
                    return self
                        .tally
                        .after_chunk(started, imported, end == self.visits.len());
                }
                Err(StoreError::ImportEntryRefused { index, reason }) => {
                    log::debug!("import: visit {} left out: {reason}", kept[index]);
                    left_out.push(kept[index]);
                }
                Err(error) => return self.tally.failed(started, error),
            }
        }
    }

    fn cancelled(&mut self) -> Payload {
        self.tally.summary(ImportOutcome::Cancelled)
    }

    fn failed(&mut self, error: ProfileDataError) -> Payload {
        self.tally.summary(ImportOutcome::Failed(error))
    }

    fn describe(&self) -> String {
        self.tally.describe("visits")
    }
}

/// The nodes of one tree, counting its descendants.
fn node_count(node: &BookmarkImport) -> usize {
    let mut count = 0;
    let mut stack = vec![node];
    while let Some(node) = stack.pop() {
        count += 1;
        if let BookmarkImport::Folder { children, .. } = node {
            stack.extend(children);
        }
    }
    count
}

/// Removes the node at `index` in a pre-order walk of all the trees of `sets`, in order, with
/// everything in it; returns how many nodes went.
fn remove_in_pre_order(sets: &mut [(BookmarkId, Vec<BookmarkImport>)], index: usize) -> usize {
    fn walk(nodes: &mut Vec<BookmarkImport>, index: &mut usize) -> Option<usize> {
        let mut position = 0;
        while position < nodes.len() {
            if *index == 0 {
                return Some(node_count(&nodes.remove(position)));
            }
            *index -= 1;
            if let BookmarkImport::Folder { children, .. } = &mut nodes[position]
                && let Some(removed) = walk(children, index)
            {
                return Some(removed);
            }
            position += 1;
        }
        None
    }
    let mut index = index;
    sets.iter_mut()
        .find_map(|(_, nodes)| walk(nodes, &mut index))
        .unwrap_or(0)
}

struct BookmarkSteps {
    /// Top-level nodes with their parents and sizes, in order, consumed from the front.
    nodes: VecDeque<(BookmarkId, BookmarkImport, usize)>,
    tally: Tally,
}

impl Steps for BookmarkSteps {
    fn step(&mut self, database: &mut Database, now: i64) -> Step {
        let started = Instant::now();
        let mut chunk: Vec<(BookmarkId, Vec<BookmarkImport>)> = Vec::new();
        let mut size = 0;
        let mut top_level = 0;
        while let Some((_, _, count)) = self.nodes.front() {
            if size > 0 && size + count > IMPORT_CHUNK {
                break;
            }
            let Some((parent, node, count)) = self.nodes.pop_front() else {
                break;
            };
            size += count;
            top_level += 1;
            match chunk.last_mut() {
                Some((last, nodes)) if *last == parent => nodes.push(node),
                _ => chunk.push((parent, vec![node])),
            }
        }
        let mut skipped = 0;
        loop {
            let sets = chunk
                .iter()
                .map(|(parent, nodes)| (*parent, nodes.as_slice()))
                .collect::<Vec<_>>();
            match database.store.import_bookmark_sets(&sets, now) {
                Ok(imported) => {
                    for (parent, _) in &chunk {
                        database.changed(Change::folders([*parent]));
                    }
                    self.tally.imported += imported;
                    self.tally.skipped += skipped as u64;
                    self.tally.resume_from += top_level;
                    return self
                        .tally
                        .after_chunk(started, imported, self.nodes.is_empty());
                }
                Err(StoreError::ImportEntryRefused { index, reason }) => {
                    let removed = remove_in_pre_order(&mut chunk, index);
                    log::debug!("import: bookmark node left out ({removed} with it): {reason}");
                    if removed == 0 {
                        return self.tally.failed(
                            started,
                            StoreError::InvalidArgument("a refused bookmark could not be found"),
                        );
                    }
                    skipped += removed;
                }
                Err(error) => return self.tally.failed(started, error),
            }
        }
    }

    fn cancelled(&mut self) -> Payload {
        self.tally.summary(ImportOutcome::Cancelled)
    }

    fn failed(&mut self, error: ProfileDataError) -> Payload {
        self.tally.summary(ImportOutcome::Failed(error))
    }

    fn describe(&self) -> String {
        self.tally.describe("bookmarks")
    }
}

impl ProfileData {
    /// Imports visits in chunks. `on_progress` runs after each chunk but the last, `on_done`
    /// once at the end. Notifies [`Area::History`] when it ends, if anything was imported. See
    /// the module documentation for refused entries and resuming.
    pub(crate) fn import_visits(
        &self,
        visits: Vec<ImportVisit>,
        on_progress: impl Fn(ImportProgress) + 'static,
        on_done: impl FnOnce(ImportSummary) + 'static,
    ) -> JobHandle {
        let total = visits.len() as u64;
        self.start_job(
            Some(Area::History),
            VisitSteps {
                visits,
                tally: Tally {
                    total,
                    ..Tally::default()
                },
            },
            on_progress,
            on_done,
        )
    }

    /// Appends each tree under its parent folder, in order, in chunks of whole top-level nodes.
    /// The trees carry their own GUIDs (make them with `new_guid`). Progress and the summary
    /// count nodes; `resume_from` counts top-level nodes. Notifies [`Area::Bookmarks`] with the
    /// parent folders when it ends, if anything was imported. See the module documentation for
    /// refused entries, resuming, and why to import into a folder created first.
    pub(crate) fn import_bookmarks(
        &self,
        sets: Vec<(BookmarkId, Vec<BookmarkImport>)>,
        on_progress: impl Fn(ImportProgress) + 'static,
        on_done: impl FnOnce(ImportSummary) + 'static,
    ) -> JobHandle {
        let nodes = sets
            .into_iter()
            .flat_map(|(parent, nodes)| {
                nodes.into_iter().map(move |node| {
                    let count = node_count(&node);
                    (parent, node, count)
                })
            })
            .collect::<VecDeque<_>>();
        let total = nodes.iter().map(|(_, _, count)| *count as u64).sum();
        self.start_job(
            Some(Area::Bookmarks),
            BookmarkSteps {
                nodes,
                tally: Tally {
                    total,
                    ..Tally::default()
                },
            },
            on_progress,
            on_done,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mark(title: &str) -> BookmarkImport {
        BookmarkImport::Bookmark {
            title: title.to_owned(),
            url: format!("https://{title}.test/"),
            guid: title.to_owned(),
            added_at_ms: None,
        }
    }

    fn folder(title: &str, children: Vec<BookmarkImport>) -> BookmarkImport {
        BookmarkImport::Folder {
            title: title.to_owned(),
            guid: title.to_owned(),
            added_at_ms: None,
            children,
        }
    }

    fn titles(sets: &[(BookmarkId, Vec<BookmarkImport>)]) -> Vec<String> {
        fn walk(nodes: &[BookmarkImport], out: &mut Vec<String>) {
            for node in nodes {
                match node {
                    BookmarkImport::Folder {
                        title, children, ..
                    } => {
                        out.push(title.clone());
                        walk(children, out);
                    }
                    BookmarkImport::Bookmark { title, .. } => out.push(title.clone()),
                    BookmarkImport::Separator { guid } => out.push(guid.clone()),
                }
            }
        }
        let mut out = Vec::new();
        for (_, nodes) in sets {
            walk(nodes, &mut out);
        }
        out
    }

    #[test]
    fn a_node_is_removed_by_its_pre_order_index_with_its_contents() {
        let make = || {
            vec![
                (
                    BookmarkId(1),
                    vec![mark("a"), folder("f", vec![mark("b"), mark("c")])],
                ),
                (BookmarkId(2), vec![mark("d")]),
            ]
        };
        // Pre-order: a f b c d.
        let mut sets = make();
        assert_eq!(remove_in_pre_order(&mut sets, 1), 3);
        assert_eq!(titles(&sets), ["a", "d"]);

        let mut sets = make();
        assert_eq!(remove_in_pre_order(&mut sets, 3), 1);
        assert_eq!(titles(&sets), ["a", "f", "b", "d"]);

        let mut sets = make();
        assert_eq!(remove_in_pre_order(&mut sets, 4), 1);
        assert_eq!(titles(&sets), ["a", "f", "b", "c"]);

        let mut sets = make();
        assert_eq!(remove_in_pre_order(&mut sets, 5), 0);
        assert_eq!(titles(&sets), ["a", "f", "b", "c", "d"]);
    }
}
