//! The one owner of the profile database (`places.sqlite`: history, bookmarks, download
//! records) for the whole application.
//!
//! # Threading
//!
//! A service thread, "profile data", opens the [`Store`] and is the only code that touches
//! it; the UI thread never runs SQLite. Callers on the UI thread use a [`ProfileData`] handle
//! ([`handle`]); each request method sends a job over a channel, and the jobs run one at a
//! time in the order they were sent. A request with a reply gets it on the UI thread: the
//! service queues the result and wakes the UI with `slint::invoke_from_event_loop`, which then
//! runs the reply callbacks in request order ([`ProfileData::deliver_replies`]). Slint's
//! wake-up is used rather than the engine's waker because replies have nothing to do with
//! Servo: they must arrive whether or not the engine is running, and must not pump it.
//!
//! # Requests
//!
//! The request methods are spread over the submodules by area: `history` (round 1: visits and
//! titles, fed by [`record_visits`]), `bookmarks`, `downloads` and `import`. Each is typed: its
//! arguments are owned values and its reply callback receives `Result<T, ProfileDataError>`.
//! Writes that happen too often to answer (visits, titles, download progress) have no reply;
//! their failures are logged.
//!
//! # Errors
//!
//! If the database cannot be opened (no usable profile, a damaged file, a file from a newer
//! Mercury, a lock held elsewhere for longer than the store's busy timeout), the browser runs
//! without it: the service says so once on standard error, every reply is
//! [`ProfileDataError::Unavailable`] with the reason, and writes without a reply are dropped.
//! A failed operation replies [`ProfileDataError::Store`].
//!
//! # Start-up, upkeep and shutdown
//!
//! [`start`] spawns the service. Once the database is open the service marks downloads left in
//! progress by the last run as interrupted (before any request runs), then refreshes stale
//! frecency scores in batches of [`REFRESH_BATCH`] until a batch comes back short, running every
//! queued request between batches, so requests go first. It repeats the pass when it has been
//! idle (no request) for [`IDLE_BEFORE_UPKEEP`] and the last pass started at least
//! [`UPKEEP_INTERVAL`] ago; scores only go stale after a day, so that keeps them fresh.
//!
//! [`shut_down`] runs every request sent before it, then closes the database and returns; call
//! it once, after the event loop has ended and before the engine is shut down.

mod bookmarks;
mod downloads;
mod history;
mod import;
#[cfg(test)]
mod tests;

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mercury_store::{Store, StoreError, VisitId};
use talaria_core::{BrowserController, VisitKey};

pub(crate) use history::record_visits;

use crate::profile::ActiveProfile;

/// Pages re-scored per `refresh_frecencies` call; requests waiting run between calls.
pub(crate) const REFRESH_BATCH: u32 = 256;
/// How long the service must have had no request before it considers an upkeep pass.
pub(crate) const IDLE_BEFORE_UPKEEP: Duration = Duration::from_secs(60);
/// The least time between the starts of two upkeep passes.
pub(crate) const UPKEEP_INTERVAL: Duration = Duration::from_secs(60 * 60);
/// How long `shut_down` waits for queued work before giving up on the service.
pub(crate) const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);
/// Visits that can still be taken back (an error page is recognised moments after its commit).
const RECENT_VISITS: usize = 64;

thread_local! {
    static PROFILE_DATA: RefCell<Option<ProfileData>> = const { RefCell::new(None) };
}

/// Starts the service for this run's profile and teaches the controller the engine's error
/// pages, so that their visits are taken back. Call once, on the UI thread, after the Slint
/// backend exists.
pub(crate) fn start(profile: &ActiveProfile, controller: &mut BrowserController) {
    controller.set_error_page_titles(talaria_servo::error_page_titles());
    let database = profile
        .paths
        .as_ref()
        .map(|paths| paths.places_database.clone());
    let data = ProfileData::spawn(
        move || open_database(database),
        Arc::new(now_ms),
        Arc::new(|| {
            let _queued = slint::invoke_from_event_loop(deliver_replies);
        }),
        Timing::default(),
    );
    PROFILE_DATA.with(|current| *current.borrow_mut() = Some(data));
}

/// The service's handle, if it has been started and not shut down.
pub(crate) fn handle() -> Option<ProfileData> {
    PROFILE_DATA.with(|current| current.borrow().clone())
}

/// Runs every request sent so far, closes the database and stops the service. Blocks for at
/// most [`SHUTDOWN_TIMEOUT`]. Later requests are dropped unanswered.
pub(crate) fn shut_down() {
    if let Some(data) = PROFILE_DATA.with(|current| current.borrow_mut().take()) {
        data.shut_down(SHUTDOWN_TIMEOUT);
    }
}

fn deliver_replies() {
    if let Some(data) = handle() {
        data.deliver_replies();
    }
}

fn open_database(path: Option<PathBuf>) -> Result<Store, String> {
    let Some(path) = path else {
        return Err("there is no usable profile directory".to_owned());
    };
    Store::open(&path).map_err(|error| format!("{} could not be opened: {error}", path.display()))
}

/// Milliseconds since the Unix epoch, by the system clock.
pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

/// Why a request failed.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ProfileDataError {
    /// The database could not be opened in this run; the reason is the one given at start-up.
    #[error("the profile database is not available: {0}")]
    Unavailable(Arc<str>),
    /// The store refused or failed the operation.
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// The open database, as the service's jobs see it.
pub(crate) struct Database {
    store: Store,
    /// The most recent visits recorded through `history::record_visits`, newest last.
    recent_visits: VecDeque<(VisitKey, VisitId)>,
}

impl Database {
    pub(crate) fn store(&mut self) -> &mut Store {
        &mut self.store
    }
}

type Job = Box<dyn FnOnce(&mut Worker) + Send>;
type Pending = Box<dyn FnOnce(Box<dyn Any + Send>)>;
type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;
type Wake = Arc<dyn Fn() + Send + Sync>;

enum Message {
    Job(Job),
    Shutdown,
}

struct Delivery {
    id: u64,
    payload: Box<dyn Any + Send>,
}

/// The service's timing policy; tests shorten it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Timing {
    pub(crate) idle_before_upkeep: Duration,
    pub(crate) upkeep_interval: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            idle_before_upkeep: IDLE_BEFORE_UPKEEP,
            upkeep_interval: UPKEEP_INTERVAL,
        }
    }
}

/// A handle to the service, for the UI thread. Clones share the service.
#[derive(Clone)]
pub(crate) struct ProfileData {
    shared: Rc<Shared>,
}

struct Shared {
    requests: Sender<Message>,
    replies: Receiver<Delivery>,
    pending: RefCell<HashMap<u64, Pending>>,
    next_id: Cell<u64>,
    finished: Receiver<()>,
}

impl ProfileData {
    /// Starts the service thread. `open` runs on it; `wake` is called on it after each reply
    /// and must make the UI thread call `deliver_replies` soon.
    pub(crate) fn spawn(
        open: impl FnOnce() -> Result<Store, String> + Send + 'static,
        clock: Clock,
        wake: Wake,
        timing: Timing,
    ) -> Self {
        let (requests, messages) = mpsc::channel();
        let (reply_sender, replies) = mpsc::channel();
        let (finished_sender, finished) = mpsc::channel();
        let spawned = thread::Builder::new()
            .name("profile data".to_owned())
            .spawn(move || {
                let database = open()
                    .map(|store| Database {
                        store,
                        recent_visits: VecDeque::new(),
                    })
                    .map_err(|reason| {
                        eprintln!(
                            "mercury-browser: history, bookmarks and downloads are not kept in \
                             this run: {reason}"
                        );
                        Arc::<str>::from(reason)
                    });
                let worker = Worker {
                    database,
                    clock,
                    replies: reply_sender,
                    wake,
                };
                worker.run(&messages, timing);
                let _sent = finished_sender.send(());
            });
        if let Err(error) = spawned {
            eprintln!(
                "mercury-browser: history, bookmarks and downloads are not kept in this run: \
                 the profile data thread could not start: {error}"
            );
        }
        Self {
            shared: Rc::new(Shared {
                requests,
                replies,
                pending: RefCell::new(HashMap::new()),
                next_id: Cell::new(0),
                finished,
            }),
        }
    }

    /// Runs `work` on the service thread and hands its result to `reply` on the UI thread.
    pub(crate) fn request<T: Send + 'static>(
        &self,
        work: impl FnOnce(&mut Database, i64) -> mercury_store::Result<T> + Send + 'static,
        reply: impl FnOnce(Result<T, ProfileDataError>) + 'static,
    ) {
        let id = self.shared.next_id.get();
        self.shared.next_id.set(id + 1);
        self.shared.pending.borrow_mut().insert(
            id,
            Box::new(
                move |payload| match payload.downcast::<Result<T, ProfileDataError>>() {
                    Ok(result) => reply(*result),
                    Err(_) => log::error!("a profile data reply had the wrong type"),
                },
            ),
        );
        let job: Job = Box::new(move |worker: &mut Worker| {
            let now = (worker.clock)();
            let result = match &mut worker.database {
                Ok(database) => work(database, now).map_err(ProfileDataError::Store),
                Err(reason) => Err(ProfileDataError::Unavailable(Arc::clone(reason))),
            };
            worker.reply(id, Box::new(result));
        });
        if self.shared.requests.send(Message::Job(job)).is_err() {
            log::debug!("profile data request dropped: the service has stopped");
            self.shared.pending.borrow_mut().remove(&id);
        }
    }

    /// Runs `work` on the service thread without a reply; a failure is logged as `what`.
    pub(crate) fn submit(
        &self,
        what: &'static str,
        work: impl FnOnce(&mut Database, i64) -> mercury_store::Result<()> + Send + 'static,
    ) {
        let job: Job = Box::new(move |worker: &mut Worker| {
            let now = (worker.clock)();
            if let Ok(database) = &mut worker.database
                && let Err(error) = work(database, now)
            {
                match error {
                    StoreError::InvalidArgument(_) | StoreError::InvalidState(_) => {
                        log::debug!("{what} refused: {error}")
                    }
                    _ => log::warn!("{what} failed: {error}"),
                }
            }
        });
        if self.shared.requests.send(Message::Job(job)).is_err() {
            log::debug!("{what} dropped: the service has stopped");
        }
    }

    /// Runs the reply callbacks of every request answered so far, in request order.
    pub(crate) fn deliver_replies(&self) {
        while let Ok(delivery) = self.shared.replies.try_recv() {
            let callback = self.shared.pending.borrow_mut().remove(&delivery.id);
            if let Some(callback) = callback {
                callback(delivery.payload);
            }
        }
    }

    fn shut_down(&self, timeout: Duration) {
        if self.shared.requests.send(Message::Shutdown).is_err() {
            return;
        }
        match self.shared.finished.recv_timeout(timeout) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => {}
            Err(RecvTimeoutError::Timeout) => log::error!(
                "the profile database was still busy after {} seconds; quitting without \
                 waiting for it",
                timeout.as_secs()
            ),
        }
    }
}

/// The service thread's state.
struct Worker {
    database: Result<Database, Arc<str>>,
    clock: Clock,
    replies: Sender<Delivery>,
    wake: Wake,
}

impl Worker {
    fn reply(&self, id: u64, payload: Box<dyn Any + Send>) {
        if self.replies.send(Delivery { id, payload }).is_ok() {
            (self.wake)();
        }
    }

    fn run(mut self, messages: &Receiver<Message>, timing: Timing) {
        if let Ok(database) = &mut self.database {
            let now = (self.clock)();
            match database.store.mark_unfinished_downloads_interrupted(now) {
                Ok(0) => {}
                Ok(count) => log::info!("{count} unfinished download(s) marked interrupted"),
                Err(error) => log::warn!("could not mark unfinished downloads: {error}"),
            }
        }
        // The `now_ms` of the upkeep pass under way, if one is, and of the last pass started.
        let mut upkeep = self.database.is_ok().then(|| (self.clock)());
        let mut last_upkeep = upkeep;
        loop {
            let message = match upkeep {
                Some(pass_now) => match messages.try_recv() {
                    Ok(message) => message,
                    Err(TryRecvError::Empty) => {
                        if !self.refresh_batch(pass_now) {
                            upkeep = None;
                        }
                        continue;
                    }
                    Err(TryRecvError::Disconnected) => break,
                },
                None => match messages.recv_timeout(timing.idle_before_upkeep) {
                    Ok(message) => message,
                    Err(RecvTimeoutError::Timeout) => {
                        let now = (self.clock)();
                        let interval =
                            i64::try_from(timing.upkeep_interval.as_millis()).unwrap_or(i64::MAX);
                        let due = last_upkeep
                            .is_none_or(|last: i64| now.saturating_sub(last) >= interval);
                        if self.database.is_ok() && due {
                            upkeep = Some(now);
                            last_upkeep = upkeep;
                        }
                        continue;
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                },
            };
            match message {
                Message::Job(job) => job(&mut self),
                Message::Shutdown => break,
            }
        }
        // Dropping the worker closes the database.
    }

    /// One `refresh_frecencies` call. Returns whether the pass should continue.
    fn refresh_batch(&mut self, pass_now: i64) -> bool {
        let Ok(database) = &mut self.database else {
            return false;
        };
        match database.store.refresh_frecencies(pass_now, REFRESH_BATCH) {
            Ok(count) => count >= u64::from(REFRESH_BATCH),
            Err(error) => {
                log::warn!("could not refresh frecency scores: {error}");
                false
            }
        }
    }
}
