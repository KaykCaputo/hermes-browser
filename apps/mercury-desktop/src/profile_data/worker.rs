//! The service thread: requests in order, debounced titles, stepwise jobs between requests, and
//! the frecency upkeep.

use std::collections::{HashMap, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::time::{Duration, Instant};

use super::jobs::{Background, Step, Upkeep};
use super::{
    Area, Change, Clock, Database, Delivery, Job, Message, Payload, ProfileDataError,
    REFRESH_BATCH, Timing, Wake,
};

/// How long the thread sleeps when it has nothing scheduled (it still wakes for any message).
const NOTHING_SCHEDULED: Duration = Duration::from_secs(3600);

/// A page's title writes within the debounce window.
struct TitleSlot {
    written: Instant,
    held: Option<String>,
}

pub(super) struct Worker {
    database: Result<Database, Arc<str>>,
    clock: Clock,
    deliveries: Sender<Delivery>,
    wake: Wake,
    wake_pending: Arc<AtomicBool>,
    timing: Timing,
    titles: HashMap<String, TitleSlot>,
    background: VecDeque<Background>,
    last_request: Instant,
    /// When the last upkeep pass started.
    last_upkeep: Instant,
    /// When the last `History` notification went out, and the one held back since.
    history_sent: Option<Instant>,
    history_held: Option<Change>,
}

impl Worker {
    pub(super) fn new(
        database: Result<Database, Arc<str>>,
        clock: Clock,
        deliveries: Sender<Delivery>,
        wake: Wake,
        wake_pending: Arc<AtomicBool>,
        timing: Timing,
    ) -> Self {
        Self {
            database,
            clock,
            deliveries,
            wake,
            wake_pending,
            timing,
            titles: HashMap::new(),
            background: VecDeque::new(),
            last_request: Instant::now(),
            last_upkeep: Instant::now(),
            history_sent: None,
            history_held: None,
        }
    }

    pub(super) fn run(mut self, messages: &Receiver<Message>) {
        if let Ok(database) = &mut self.database {
            let now = (self.clock)();
            match database.store.mark_unfinished_downloads_interrupted(now) {
                Ok(0) => {}
                Ok(count) => log::info!("{count} unfinished download(s) marked interrupted"),
                Err(error) => log::warn!("could not mark unfinished downloads: {error}"),
            }
            self.start_upkeep();
        }
        loop {
            let timeout = self.next_timeout();
            let message = if timeout.is_zero() {
                match messages.try_recv() {
                    Ok(message) => Some(message),
                    Err(TryRecvError::Empty) => None,
                    Err(TryRecvError::Disconnected) => break,
                }
            } else {
                match messages.recv_timeout(timeout) {
                    Ok(message) => Some(message),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            };
            match message {
                Some(Message::Job(job)) => {
                    self.last_request = Instant::now();
                    self.run_job(job);
                }
                Some(Message::Title { url, title }) => self.title(url, title),
                Some(Message::Background(job)) => {
                    self.last_request = Instant::now();
                    self.background.push_back(job);
                }
                Some(Message::Shutdown) => break,
                None => {
                    self.flush_due_titles();
                    self.flush_due_history();
                    if !self.background.is_empty() {
                        self.step_next_job();
                    } else if self.upkeep_is_due() {
                        self.start_upkeep();
                    }
                }
            }
        }
        self.finish(messages);
    }

    /// How long to wait for a message before doing scheduled work: nothing while a job has
    /// chunks left (requests still go first); otherwise until the next held title or upkeep.
    fn next_timeout(&self) -> Duration {
        if !self.background.is_empty() {
            return Duration::ZERO;
        }
        let now = Instant::now();
        let titles = self
            .titles
            .values()
            .map(|slot| slot.written + self.timing.title_debounce);
        let upkeep = self.database.is_ok().then(|| {
            (self.last_request + self.timing.idle_before_upkeep)
                .max(self.last_upkeep + self.timing.upkeep_interval)
        });
        let history = self
            .history_held
            .as_ref()
            .and(self.history_sent)
            .map(|sent| sent + self.timing.history_notify_interval);
        titles
            .chain(upkeep)
            .chain(history)
            .min()
            .map_or(NOTHING_SCHEDULED, |at| at.saturating_duration_since(now))
    }

    fn upkeep_is_due(&self) -> bool {
        self.database.is_ok()
            && self.last_request.elapsed() >= self.timing.idle_before_upkeep
            && self.last_upkeep.elapsed() >= self.timing.upkeep_interval
    }

    fn start_upkeep(&mut self) {
        self.last_upkeep = Instant::now();
        self.background.push_back(Background {
            id: None,
            area: None,
            cancel: Arc::new(AtomicBool::new(false)),
            steps: Box::new(Upkeep {
                now: (self.clock)(),
                batch: REFRESH_BATCH,
                refreshed: 0,
            }),
            wrote: false,
            change: None,
        });
    }

    fn deliver(&self, delivery: Delivery) {
        if self.deliveries.send(delivery).is_ok() && !self.wake_pending.swap(true, Ordering::AcqRel)
        {
            (self.wake)();
        }
    }

    /// Tells listeners about a change: at once, except that `History` changes go out at most
    /// once per interval (the ones in between are merged and held).
    fn notify(&mut self, change: Change) {
        if change.area != Area::History {
            self.deliver(Delivery::Changed(change));
            return;
        }
        match &mut self.history_held {
            Some(held) => held.merge(change),
            None => self.history_held = Some(change),
        }
        self.flush_due_history();
    }

    fn flush_due_history(&mut self) {
        let due = self
            .history_sent
            .is_none_or(|sent| sent.elapsed() >= self.timing.history_notify_interval);
        if due && let Some(change) = self.history_held.take() {
            self.history_sent = Some(Instant::now());
            self.deliver(Delivery::Changed(change));
        }
    }

    /// What the job that just ran changed: the hints it noted, or its area with none.
    fn take_changes(&mut self, area: Option<Area>, wrote: bool) -> Vec<Change> {
        let noted = match &mut self.database {
            Ok(database) => std::mem::take(&mut database.changes),
            Err(_) => Vec::new(),
        };
        match (wrote, area) {
            (false, _) | (true, None) => Vec::new(),
            (true, Some(area)) if noted.is_empty() => vec![Change::any(area)],
            (true, Some(_)) => noted,
        }
    }

    fn run_job(&mut self, job: Job) {
        if job.reply_to.is_some() {
            self.flush_titles();
        }
        let Job {
            reply_to,
            area,
            work,
            fail,
        } = job;
        let now = (self.clock)();
        let database = self.database.as_mut().map_err(|reason| &*reason);
        match catch_unwind(AssertUnwindSafe(|| work(database, now))) {
            Ok(outcome) => {
                if let (Some(id), Some(payload)) = (reply_to, outcome.reply) {
                    self.deliver(Delivery::Reply { id, payload });
                }
                for change in self.take_changes(area, outcome.wrote) {
                    self.notify(change);
                }
            }
            Err(_) => {
                self.take_changes(None, false);
                log::error!("a profile data request panicked; it is answered as unavailable");
                if let (Some(id), Some(fail)) = (reply_to, fail) {
                    let payload = fail(unexpected());
                    self.deliver(Delivery::Reply { id, payload });
                }
            }
        }
    }

    /// Runs one chunk of the job at the front, then puts it at the back if it has more.
    fn step_next_job(&mut self) {
        let Some(mut job) = self.background.pop_front() else {
            return;
        };
        if job.cancel.load(Ordering::Acquire) {
            let summary = job.steps.cancelled();
            self.end_job(job, summary);
            return;
        }
        let database = match &mut self.database {
            Ok(database) => database,
            Err(reason) => {
                let summary = job
                    .steps
                    .failed(ProfileDataError::Unavailable(Arc::clone(reason)));
                self.end_job(job, summary);
                return;
            }
        };
        let now = (self.clock)();
        match catch_unwind(AssertUnwindSafe(|| job.steps.step(database, now))) {
            Ok(Step::Continue { progress, wrote }) => {
                job.wrote |= wrote;
                self.collect_job_changes(&mut job, wrote);
                if let (Some(id), Some(payload)) = (job.id, progress) {
                    self.deliver(Delivery::Progress { id, payload });
                }
                self.background.push_back(job);
            }
            Ok(Step::Finished { summary, wrote }) => {
                job.wrote |= wrote;
                self.collect_job_changes(&mut job, wrote);
                self.end_job(job, summary);
            }
            Err(_) => {
                self.take_changes(None, false);
                log::error!("a profile data job panicked: {}", job.steps.describe());
                let summary = job.steps.failed(unexpected());
                self.end_job(job, summary);
            }
        }
    }

    /// Adds what a job's chunk changed to the job's one notification.
    fn collect_job_changes(&mut self, job: &mut Background, wrote: bool) {
        for change in self.take_changes(job.area, wrote) {
            match &mut job.change {
                Some(merged) => merged.merge(change),
                None => job.change = Some(change),
            }
        }
    }

    /// Delivers a job's summary, then its change notification if a chunk wrote.
    fn end_job(&mut self, job: Background, summary: Payload) {
        if let Some(id) = job.id {
            self.deliver(Delivery::Reply {
                id,
                payload: summary,
            });
        }
        if let (true, Some(area)) = (job.wrote, job.area) {
            self.notify(job.change.unwrap_or_else(|| Change::any(area)));
        }
    }

    /// A page's title: written at once unless one was written for that URL within the
    /// debounce window, in which case it is held and the last one held wins.
    fn title(&mut self, url: String, title: String) {
        let debounce = self.timing.title_debounce;
        match self.titles.get_mut(&url) {
            Some(slot) if slot.written.elapsed() < debounce => slot.held = Some(title),
            _ => {
                self.write_title(&url, &title);
                self.titles.insert(
                    url,
                    TitleSlot {
                        written: Instant::now(),
                        held: None,
                    },
                );
            }
        }
    }

    fn write_title(&mut self, url: &str, title: &str) {
        let Ok(database) = &mut self.database else {
            return;
        };
        match database.store.set_page_title(url, title) {
            Ok(true) => self.notify(Change::any(Area::History)),
            Ok(false) => {}
            Err(error) => log::warn!("recording a page title failed: {error}"),
        }
    }

    /// Writes the held titles whose window has passed, and forgets idle windows.
    fn flush_due_titles(&mut self) {
        let debounce = self.timing.title_debounce;
        let due = self
            .titles
            .iter_mut()
            .filter(|(_, slot)| slot.written.elapsed() >= debounce)
            .filter_map(|(url, slot)| slot.held.take().map(|title| (url.clone(), title)))
            .collect::<Vec<_>>();
        self.titles
            .retain(|_, slot| slot.held.is_some() || slot.written.elapsed() < debounce);
        for (url, title) in due {
            self.write_title(&url, &title);
            self.titles.insert(
                url,
                TitleSlot {
                    written: Instant::now(),
                    held: None,
                },
            );
        }
    }

    /// Writes every held title now.
    fn flush_titles(&mut self) {
        let held = self
            .titles
            .iter_mut()
            .filter_map(|(url, slot)| slot.held.take().map(|title| (url.clone(), title)))
            .collect::<Vec<_>>();
        for (url, title) in held {
            self.write_title(&url, &title);
        }
    }

    /// At shutdown: held titles are written, jobs are cancelled between chunks and report what
    /// they finished, and anything sent after the shutdown is answered `Unavailable`. Dropping
    /// the worker then closes the database.
    fn finish(mut self, messages: &Receiver<Message>) {
        self.flush_titles();
        if let Some(change) = self.history_held.take() {
            self.deliver(Delivery::Changed(change));
        }
        for mut job in std::mem::take(&mut self.background) {
            if job.id.is_some() {
                log::info!("cancelled at shutdown: {}", job.steps.describe());
            }
            let summary = job.steps.cancelled();
            self.end_job(job, summary);
        }
        let stopped =
            || ProfileDataError::Unavailable("the profile data service has stopped".into());
        while let Ok(message) = messages.try_recv() {
            let answer = match message {
                Message::Job(Job {
                    reply_to: Some(id),
                    fail: Some(fail),
                    ..
                }) => Some((id, fail(stopped()))),
                Message::Background(job) => job.stopped(stopped()),
                _ => None,
            };
            if let Some((id, payload)) = answer {
                self.deliver(Delivery::Reply { id, payload });
            }
        }
    }
}

fn unexpected() -> ProfileDataError {
    ProfileDataError::Unavailable("the request failed unexpectedly".into())
}
