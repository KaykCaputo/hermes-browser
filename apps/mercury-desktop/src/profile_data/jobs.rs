//! Stepwise background jobs: long work (bulk import, frecency upkeep) that the service runs one
//! chunk at a time. Each chunk is its own transaction; between chunks every waiting request
//! runs, and jobs take turns. A job reports progress to a UI-thread callback after each chunk
//! and a summary when it ends: finished, cancelled (its [`JobHandle`], or shutdown, checked
//! between chunks) or failed. Work already committed by earlier chunks stays.

use std::any::Any;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::{Area, Change, Database, Message, Payload, ProfileData, ProfileDataError};

/// What one chunk did.
pub(super) enum Step {
    /// More to do; `progress` goes to the progress callback.
    Continue {
        progress: Option<Payload>,
        wrote: bool,
    },
    /// Done; `summary` goes to the completion callback.
    Finished { summary: Payload, wrote: bool },
}

/// A job's chunks.
pub(super) trait Steps: Send {
    /// Runs the next chunk, in its own transaction.
    fn step(&mut self, database: &mut Database, now: i64) -> Step;
    /// The summary when the job is cancelled before its next chunk.
    fn cancelled(&mut self) -> Payload;
    /// The summary when it cannot run (no database, a chunk panicked, the service stopped).
    fn failed(&mut self, error: ProfileDataError) -> Payload;
    /// What the job has done so far, for the log.
    fn describe(&self) -> String;
}

/// A job as the service holds it.
pub(super) struct Background {
    /// The id its summary and progress are delivered under; `None` for internal jobs.
    pub(super) id: Option<u64>,
    /// The area a chunk that wrote changes; notified once, when the job ends.
    pub(super) area: Option<Area>,
    pub(super) cancel: Arc<AtomicBool>,
    pub(super) steps: Box<dyn Steps>,
    pub(super) wrote: bool,
    /// What its chunks changed, merged, for its one notification at the end.
    pub(super) change: Option<Change>,
}

impl Background {
    /// The summary to deliver for a job the service will never run.
    pub(super) fn stopped(mut self, error: ProfileDataError) -> Option<(u64, Payload)> {
        let summary = self.steps.failed(error);
        self.id.map(|id| (id, summary))
    }
}

/// Cancels a stepwise job: it stops before its next chunk and reports what it finished.
#[derive(Debug, Clone)]
pub(crate) struct JobHandle {
    cancel: Arc<AtomicBool>,
}

impl JobHandle {
    #[cfg_attr(not(test), expect(dead_code, reason = "called by the import work"))]
    pub(crate) fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }
}

impl ProfileData {
    /// Starts a stepwise job. `on_progress` runs after each chunk that reports progress and
    /// `on_done` once at the end, both on the UI thread.
    pub(super) fn start_job<P: Any + Send, S: Any + Send>(
        &self,
        area: Option<Area>,
        steps: impl Steps + 'static,
        on_progress: impl Fn(P) + 'static,
        on_done: impl FnOnce(S) + 'static,
    ) -> JobHandle {
        let id = self.next_id();
        self.shared.pending.borrow_mut().insert(
            id,
            Box::new(move |payload: Payload| match payload.downcast::<S>() {
                Ok(summary) => on_done(*summary),
                Err(_) => log::error!("a profile data job summary had the wrong type"),
            }),
        );
        self.shared.progress.borrow_mut().insert(
            id,
            Rc::new(move |payload: Payload| match payload.downcast::<P>() {
                Ok(progress) => on_progress(*progress),
                Err(_) => log::error!("a profile data job progress had the wrong type"),
            }),
        );
        let cancel = Arc::new(AtomicBool::new(false));
        self.send(Message::Background(Background {
            id: Some(id),
            area,
            cancel: Arc::clone(&cancel),
            steps: Box::new(steps),
            wrote: false,
            change: None,
        }));
        JobHandle { cancel }
    }
}

/// The frecency upkeep pass: `refresh_frecencies` with one `now` until a chunk comes back
/// short. Internal: no callbacks, no change notification (scores are not shown).
pub(super) struct Upkeep {
    pub(super) now: i64,
    pub(super) batch: u32,
    pub(super) refreshed: u64,
}

impl Steps for Upkeep {
    fn step(&mut self, database: &mut Database, _now: i64) -> Step {
        let finished = Step::Finished {
            summary: Box::new(()),
            wrote: false,
        };
        match database.store.refresh_frecencies(self.now, self.batch) {
            Ok(count) => {
                self.refreshed += count;
                if count >= u64::from(self.batch) {
                    Step::Continue {
                        progress: None,
                        wrote: false,
                    }
                } else {
                    finished
                }
            }
            Err(error) => {
                log::warn!("could not refresh frecency scores: {error}");
                finished
            }
        }
    }

    fn cancelled(&mut self) -> Payload {
        Box::new(())
    }

    fn failed(&mut self, _error: ProfileDataError) -> Payload {
        Box::new(())
    }

    fn describe(&self) -> String {
        format!("frecency upkeep, {} pages re-scored", self.refreshed)
    }
}
