//! Bulk import from another browser, as stepwise jobs (see `jobs`): chunks of
//! [`IMPORT_CHUNK`] visits, or of whole top-level bookmark nodes up to that many nodes, each
//! chunk one transaction. An import is therefore not atomic as a whole: a cancel, a shutdown
//! or a refused entry stops it after the chunks already committed, and the summary says how
//! many entries were imported.
//!
//! Chunking does not change what the store keeps: visit counts, typed counts and last-visit
//! times are recomputed from the visit rows, and a page's title is that of its newest titled
//! visit whichever chunk brings it. The one difference is a tie at the same millisecond in
//! two chunks: the later chunk's title wins, where a single call would keep the greater.

#![cfg_attr(not(test), expect(dead_code, reason = "called by the import work"))]

use std::time::{Duration, Instant};

use mercury_store::{BookmarkId, BookmarkImport, StoreError, VisitImport, VisitTransition};

use super::jobs::{JobHandle, Step, Steps};
use super::{Area, Database, Payload, ProfileData, ProfileDataError};

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

/// After each chunk: entries imported so far, of all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ImportProgress {
    pub(crate) done: u64,
    pub(crate) total: u64,
}

/// How an import ended.
#[derive(Debug)]
pub(crate) enum ImportOutcome {
    Completed,
    Cancelled,
    /// A chunk failed; nothing of it was imported. A refused entry's index counts from the
    /// start of the whole import.
    Failed(ProfileDataError),
}

/// The end of an import: how many entries were imported of all, and how it ended.
#[derive(Debug)]
pub(crate) struct ImportSummary {
    pub(crate) imported: u64,
    pub(crate) total: u64,
    pub(crate) outcome: ImportOutcome,
    /// The longest chunk, which is the longest any request waited behind this import.
    pub(crate) longest_chunk: Duration,
}

/// Counts and timing shared by both kinds of import.
struct Tally {
    imported: u64,
    total: u64,
    longest_chunk: Duration,
}

impl Tally {
    fn summary(&self, outcome: ImportOutcome) -> Payload {
        Box::new(ImportSummary {
            imported: self.imported,
            total: self.total,
            outcome,
            longest_chunk: self.longest_chunk,
        })
    }

    /// Records a chunk that took since `started` and imported `count` entries (or failed).
    fn chunk(&mut self, started: Instant, result: mercury_store::Result<u64>) -> Step {
        self.longest_chunk = self.longest_chunk.max(started.elapsed());
        match result {
            Ok(count) => {
                self.imported += count;
                if self.imported >= self.total {
                    Step::Finished {
                        summary: self.summary(ImportOutcome::Completed),
                        wrote: count > 0,
                    }
                } else {
                    Step::Continue {
                        progress: Some(Box::new(ImportProgress {
                            done: self.imported,
                            total: self.total,
                        })),
                        wrote: count > 0,
                    }
                }
            }
            Err(error) => {
                let error = match error {
                    StoreError::ImportEntryRefused { index, reason } => {
                        StoreError::ImportEntryRefused {
                            index: index + self.imported as usize,
                            reason,
                        }
                    }
                    error => error,
                };
                Step::Finished {
                    summary: self.summary(ImportOutcome::Failed(ProfileDataError::Store(error))),
                    wrote: false,
                }
            }
        }
    }

    fn describe(&self, what: &str) -> String {
        format!(
            "import of {what}, {} of {} imported",
            self.imported, self.total
        )
    }
}

struct VisitSteps {
    visits: Vec<ImportVisit>,
    tally: Tally,
}

impl Steps for VisitSteps {
    fn step(&mut self, database: &mut Database, now: i64) -> Step {
        let start = self.tally.imported as usize;
        let end = (start + IMPORT_CHUNK).min(self.visits.len());
        let chunk = self.visits[start..end]
            .iter()
            .map(|visit| VisitImport {
                url: &visit.url,
                title: visit.title.as_deref(),
                visited_at_ms: visit.visited_at_ms,
                transition: visit.transition,
            })
            .collect::<Vec<_>>();
        let started = Instant::now();
        let result = database.store.import_visits(&chunk, now);
        self.tally.chunk(started, result)
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

struct BookmarkSteps {
    /// Top-level nodes with their parents, in order, consumed from the front.
    nodes: std::collections::VecDeque<(BookmarkId, BookmarkImport, usize)>,
    tally: Tally,
}

impl Steps for BookmarkSteps {
    fn step(&mut self, database: &mut Database, now: i64) -> Step {
        let mut chunk: Vec<(BookmarkId, Vec<BookmarkImport>)> = Vec::new();
        let mut size = 0;
        while let Some((_, _, count)) = self.nodes.front() {
            if size > 0 && size + count > IMPORT_CHUNK {
                break;
            }
            let Some((parent, node, count)) = self.nodes.pop_front() else {
                break;
            };
            size += count;
            match chunk.last_mut() {
                Some((last, nodes)) if *last == parent => nodes.push(node),
                _ => chunk.push((parent, vec![node])),
            }
        }
        let sets = chunk
            .iter()
            .map(|(parent, nodes)| (*parent, nodes.as_slice()))
            .collect::<Vec<_>>();
        let started = Instant::now();
        let result = database.store.import_bookmark_sets(&sets, now);
        self.tally.chunk(started, result)
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
    /// once at the end. Notifies [`Area::History`] when it ends, if anything was imported.
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
                    imported: 0,
                    total,
                    longest_chunk: Duration::ZERO,
                },
            },
            on_progress,
            on_done,
        )
    }

    /// Appends each tree under its parent folder, in order, in chunks of whole top-level nodes.
    /// The trees carry their own GUIDs (make them with `new_guid`). Progress and the summary
    /// count nodes; a refused node's index is its place in a pre-order walk of all the trees.
    /// Notifies [`Area::Bookmarks`] when it ends, if anything was imported.
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
            .collect::<std::collections::VecDeque<_>>();
        let total = nodes.iter().map(|(_, _, count)| *count as u64).sum();
        self.start_job(
            Some(Area::Bookmarks),
            BookmarkSteps {
                nodes,
                tally: Tally {
                    imported: 0,
                    total,
                    longest_chunk: Duration::ZERO,
                },
            },
            on_progress,
            on_done,
        )
    }
}
