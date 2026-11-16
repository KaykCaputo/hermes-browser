//! The one owner of the profile database (`places.sqlite`: history, bookmarks, download
//! records) for the whole application.
//!
//! # Threading
//!
//! A service thread, "profile data", opens the [`Store`] and is the only code that touches
//! it; the UI thread never runs SQLite. Callers on the UI thread use a [`ProfileData`] handle
//! ([`handle`]). Each request method sends a job over a channel; the jobs run one at a time in
//! the order they were sent. Results come back on the UI thread: the service queues them and
//! wakes the UI with `slint::invoke_from_event_loop` (at most one wake-up outstanding), which
//! runs the reply callbacks in request order ([`ProfileData::deliver_replies`]). Slint's wake-up
//! is used rather than the engine's waker because replies have nothing to do with Servo: they
//! must arrive whether or not the engine is running, and must not pump it. Reply callbacks are
//! not `Send`: they never leave the UI thread.
//!
//! # Requests
//!
//! The request methods are spread over the submodules by area: `history` (visits and titles,
//! fed by [`record_visits`]), `bookmarks`, `downloads` and `import`. Each is typed: owned
//! arguments, and a reply callback receiving `Result<T, ProfileDataError>`. Writes that happen
//! too often to answer (visits, titles, download progress) have no reply; their failures are
//! logged. Only this module and its children can run work on the database thread (`request`,
//! `submit` and `Database` are private to it).
//!
//! **Change notifications** mean "the list changed": [`ProfileData::on_change`] registers a
//! UI-thread callback for an [`Area`], called with a [`Change`] after the replies of a delivery
//! in which a write to that area succeeded (once per delivery, the hints of its writes merged).
//! A writer gets both its reply and the notification. `History` is coalesced further: at most
//! one notification per [`HISTORY_NOTIFY_INTERVAL`]. Download progress notifies nothing (live
//! progress is the download manager's in-memory state). A listener that needs the service
//! should call [`handle`] inside the callback rather than capture a [`ProfileData`], which
//! would make a reference cycle through the listener list.
//!
//! **Stepwise jobs** (bulk import, frecency upkeep) run one chunk at a time, each chunk its own
//! transaction, with every waiting request served between chunks; see `jobs`.
//!
//! **Titles** after the first one for a page are debounced per URL for [`TITLE_DEBOUNCE`] (the
//! last one wins); held titles are written before any request with a reply runs, and at
//! shutdown.
//!
//! # Errors
//!
//! If the database cannot be opened (no usable profile, a damaged file or one failing
//! `PRAGMA quick_check`, a file from a newer Mercury, a lock held elsewhere for longer than the
//! store's busy timeout), the browser runs without it: the service says so once on standard
//! error, [`ProfileData::unavailable_reason`] gives the reason, every reply is
//! [`ProfileDataError::Unavailable`], and writes without a reply are dropped. A failed operation
//! replies [`ProfileDataError::Store`]. A request whose job panicked, or that was sent after the
//! service stopped, is answered `Unavailable` too.
//!
//! # Start-up, upkeep and shutdown
//!
//! [`start`] spawns the service. The service creates the database file private to the user
//! (0600) if it is new, runs `PRAGMA quick_check` if the last run did not close the database
//! (a `-wal` file is left) or the last check is more than [`CHECK_INTERVAL`] old (the time is
//! kept in `places.sqlite.checked`), opens the store, marks downloads left in
//! progress by the last run as interrupted (before any request runs), and starts the frecency
//! upkeep job: `refresh_frecencies` in chunks of [`REFRESH_BATCH`] until a chunk comes back
//! short. It starts the job again when it has been idle (no request) for
//! [`IDLE_BEFORE_UPKEEP`] and the last pass started at least [`UPKEEP_INTERVAL`] ago.
//!
//! [`shut_down`] runs every request sent before it, writes held titles, cancels stepwise jobs
//! between chunks (each reports what it finished), closes the database and returns, waiting no
//! longer than its deadline.
//!
//! # Panics in callbacks
//!
//! Reply, progress and change callbacks run inside `slint::invoke_from_event_loop`, that is
//! inside an AppKit callback on macOS, where a panic cannot unwind: a panicking callback aborts
//! the process. Callbacks must not panic (no `unwrap` on a reply).

mod bookmarks;
pub(crate) mod downloads;
mod history;
pub(crate) mod import;
pub(crate) mod jobs;
#[cfg(test)]
mod tests;
mod worker;

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mercury_store::{BookmarkId, DownloadRecordId, Store, StoreError, VisitId};
use talaria_core::{BrowserController, VisitKey};

pub(crate) use history::record_visits;
use worker::Worker;

use crate::profile::ActiveProfile;

/// Pages re-scored per chunk of the frecency upkeep job.
pub(crate) const REFRESH_BATCH: u32 = 1_000;
/// How long the service must have had no request before it considers an upkeep pass.
pub(crate) const IDLE_BEFORE_UPKEEP: Duration = Duration::from_secs(60);
/// The least time between the starts of two upkeep passes.
pub(crate) const UPKEEP_INTERVAL: Duration = Duration::from_secs(60 * 60);
/// How long `shut_down` waits by default; the quit sequence passes its own deadline.
pub(crate) const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);
/// Later titles of the same page within this time are held, and only the last is written.
pub(crate) const TITLE_DEBOUNCE: Duration = Duration::from_secs(3);
/// History change notifications come at most this often.
pub(crate) const HISTORY_NOTIFY_INTERVAL: Duration = Duration::from_secs(1);
/// `PRAGMA quick_check` runs at least this often (and after any run that did not close the
/// database).
pub(crate) const CHECK_INTERVAL: Duration = Duration::from_secs(7 * 86_400);
/// Visits that can still be taken back (an error page or a replaced entry is recognised
/// moments after its commit).
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

/// Runs every request sent so far, writes held titles, cancels stepwise jobs between chunks,
/// closes the database and stops the service. Returns by `deadline` at the latest (logging an
/// error if the service was still busy). Later requests are answered `Unavailable`.
pub(crate) fn shut_down(deadline: Instant) {
    if let Some(data) = PROFILE_DATA.with(|current| current.borrow_mut().take()) {
        data.shut_down(deadline);
    }
}

fn deliver_replies() {
    if let Some(data) = handle() {
        data.deliver_replies();
    }
}

/// Creates the file private to the user if it is new (and makes an existing one private),
/// checks it with `PRAGMA quick_check`, and opens the store.
fn open_database(path: Option<PathBuf>) -> Result<Store, String> {
    let Some(path) = path else {
        return Err("there is no usable profile directory".to_owned());
    };
    let failure =
        |error: &dyn std::fmt::Display| format!("{} could not be opened: {error}", path.display());
    make_private_file(&path).map_err(|error| failure(&error))?;
    let marker = check_marker(&path);
    if check_is_due(&path, &marker, SystemTime::now()) {
        quick_check(&path).map_err(|error| failure(&error))?;
        if let Err(error) = write_private(&marker, now_ms().to_string().as_bytes()) {
            log::warn!("could not record the database check: {error}");
        }
    }
    Store::open(&path).map_err(|error| failure(&error))
}

/// The file that records when the database was last checked.
fn check_marker(database: &Path) -> PathBuf {
    let mut name = database.as_os_str().to_owned();
    name.push(".checked");
    PathBuf::from(name)
}

/// A check is due when a write-ahead log was left behind (the last run did not close the
/// database) or the last recorded check is missing, unreadable or [`CHECK_INTERVAL`] old.
fn check_is_due(database: &Path, marker: &Path, now: SystemTime) -> bool {
    let mut wal = database.as_os_str().to_owned();
    wal.push("-wal");
    if Path::new(&wal).exists() {
        return true;
    }
    let checked_at = std::fs::read_to_string(marker)
        .ok()
        .and_then(|text| text.trim().parse::<u64>().ok())
        .map(|ms| UNIX_EPOCH + Duration::from_millis(ms));
    checked_at.is_none_or(|at| {
        now.duration_since(at)
            .map_or(true, |age| age >= CHECK_INTERVAL)
    })
}

#[cfg(unix)]
fn make_private_file(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)?;
    let metadata = file.metadata()?;
    if metadata.permissions().mode() & 0o077 != 0 {
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn make_private_file(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Replaces the contents of `path`, creating it readable by this user only.
fn write_private(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)?.write_all(contents)
}

/// `PRAGMA quick_check` on its own connection: a damaged file is refused before the store
/// touches it. Its cost is a read of every page of the file.
fn quick_check(path: &Path) -> Result<(), String> {
    let conn = rusqlite::Connection::open(path).map_err(|error| error.to_string())?;
    conn.busy_timeout(Duration::from_secs(5))
        .map_err(|error| error.to_string())?;
    let result: String = conn
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if result == "ok" {
        Ok(())
    } else {
        Err(format!("the integrity check failed: {result}"))
    }
}

/// Milliseconds since the Unix epoch, by the system clock.
pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

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
fn guid_or_error() -> mercury_store::Result<String> {
    new_guid().ok_or(StoreError::InvalidArgument(
        "the system's random source is unavailable",
    ))
}

/// Why a request failed.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ProfileDataError {
    /// The database is not available: it could not be opened in this run, the service has
    /// stopped, or the request failed unexpectedly.
    #[error("the profile database is not available: {0}")]
    Unavailable(Arc<str>),
    /// The store refused or failed the operation.
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// What a write changed, for [`ProfileData::on_change`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Area {
    History,
    Bookmarks,
    Downloads,
}

/// What a write changed, as told to [`ProfileData::on_change`] listeners. More hints may be
/// added; a listener reads the ones it knows.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Change {
    pub(crate) area: Area,
    /// `Bookmarks`: the folders whose children changed; `None` if not known (any may have).
    pub(crate) folders: Option<Vec<BookmarkId>>,
    /// `Downloads`: the GUIDs of the records that changed; `None` if not known (any may have).
    pub(crate) downloads: Option<Vec<String>>,
}

impl Change {
    /// A change to `area` with no hints: anything in it may have changed.
    pub(crate) fn any(area: Area) -> Self {
        Self {
            area,
            folders: None,
            downloads: None,
        }
    }

    fn folders(folders: impl IntoIterator<Item = BookmarkId>) -> Self {
        Self {
            folders: Some(folders.into_iter().collect()),
            ..Self::any(Area::Bookmarks)
        }
    }

    fn download(guid: &str) -> Self {
        Self {
            downloads: Some(vec![guid.to_owned()]),
            ..Self::any(Area::Downloads)
        }
    }

    /// Adds `other`'s hints (same area); an unknown hint makes the merged one unknown.
    fn merge(&mut self, other: Change) {
        fn union<T: PartialEq>(into: &mut Option<Vec<T>>, from: Option<Vec<T>>) {
            match (into.as_mut(), from) {
                (Some(into), Some(from)) => {
                    for item in from {
                        if !into.contains(&item) {
                            into.push(item);
                        }
                    }
                }
                _ => *into = None,
            }
        }
        union(&mut self.folders, other.folders);
        union(&mut self.downloads, other.downloads);
    }
}

/// A registered change callback; pass it to [`ProfileData::remove_change_listener`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChangeListener(u64);

/// The open database, as the service's jobs see it.
struct Database {
    store: Store,
    /// The most recent visits recorded through `history`, newest last.
    recent_visits: VecDeque<(VisitKey, VisitId)>,
    /// Download records by GUID: those inserted or listed in this run.
    download_ids: HashMap<String, DownloadRecordId>,
    /// GUIDs looked up and not found, so later requests do not scan again; a GUID leaves the
    /// set when it is inserted.
    download_misses: HashSet<String>,
    /// How often the downloads table was listed to find a GUID (for the log).
    download_scans: u64,
    /// What the running job changed, with hints; collected by the worker after it.
    changes: Vec<Change>,
}

impl Database {
    fn new(store: Store) -> Self {
        Self {
            store,
            recent_visits: VecDeque::new(),
            download_ids: HashMap::new(),
            download_misses: HashSet::new(),
            download_scans: 0,
            changes: Vec::new(),
        }
    }

    /// Notes what the current job changes, with hints; reported only if the job succeeds.
    fn changed(&mut self, change: Change) {
        self.changes.push(change);
    }

    fn store(&mut self) -> &mut Store {
        &mut self.store
    }
}

type Payload = Box<dyn Any + Send>;
type Work = Box<dyn FnOnce(Result<&mut Database, &Arc<str>>, i64) -> JobOutcome + Send>;
type ReplyCallback = Box<dyn FnOnce(Payload)>;
type ProgressCallback = Rc<dyn Fn(Payload)>;
type Listener = (ChangeListener, Area, Rc<dyn Fn(&Change)>);
type Failure = Box<dyn FnOnce(ProfileDataError) -> Payload + Send>;
type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;
type Wake = Arc<dyn Fn() + Send + Sync>;

/// A request: `work` runs on the service thread with the database (or why there is none) and
/// the time; it returns the reply (for a request with a reply) and whether it wrote to `area`.
struct Job {
    reply_to: Option<u64>,
    area: Option<Area>,
    work: Work,
    /// Builds the reply when `work` could not run (it panicked, or the service stopped).
    fail: Option<Failure>,
}

struct JobOutcome {
    reply: Option<Payload>,
    wrote: bool,
}

enum Message {
    Job(Job),
    Title { url: String, title: String },
    Background(jobs::Background),
    Shutdown,
}

enum Delivery {
    Reply { id: u64, payload: Payload },
    Progress { id: u64, payload: Payload },
    Changed(Change),
}

/// The service's timing policy; tests shorten it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Timing {
    pub(crate) idle_before_upkeep: Duration,
    pub(crate) upkeep_interval: Duration,
    pub(crate) title_debounce: Duration,
    pub(crate) history_notify_interval: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            idle_before_upkeep: IDLE_BEFORE_UPKEEP,
            upkeep_interval: UPKEEP_INTERVAL,
            title_debounce: TITLE_DEBOUNCE,
            history_notify_interval: HISTORY_NOTIFY_INTERVAL,
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
    deliveries: Receiver<Delivery>,
    /// Reply callbacks, by request id.
    pending: RefCell<HashMap<u64, ReplyCallback>>,
    /// Progress callbacks of stepwise jobs, by job id.
    progress: RefCell<HashMap<u64, ProgressCallback>>,
    listeners: RefCell<Vec<Listener>>,
    /// Replies made on the UI thread for requests the service can no longer take.
    undelivered: RefCell<Vec<(u64, Payload)>>,
    next_id: Cell<u64>,
    finished: Receiver<()>,
    wake_pending: Arc<AtomicBool>,
    wake: Wake,
    unavailable: Arc<OnceLock<Option<Arc<str>>>>,
}

impl ProfileData {
    /// Starts the service thread. `open` runs on it; `wake` is called on it when deliveries
    /// are waiting and must make the UI thread call `deliver_replies` soon.
    pub(crate) fn spawn(
        open: impl FnOnce() -> Result<Store, String> + Send + 'static,
        clock: Clock,
        wake: Wake,
        timing: Timing,
    ) -> Self {
        let (requests, messages) = mpsc::channel();
        let (delivery_sender, deliveries) = mpsc::channel();
        let (finished_sender, finished) = mpsc::channel();
        let wake_pending = Arc::new(AtomicBool::new(false));
        let unavailable = Arc::new(OnceLock::new());
        let worker_wake = Arc::clone(&wake);
        let worker_wake_pending = Arc::clone(&wake_pending);
        let worker_unavailable = Arc::clone(&unavailable);
        let spawned = thread::Builder::new()
            .name("profile data".to_owned())
            .spawn(move || {
                let database = open().map(Database::new).map_err(|reason| {
                    eprintln!(
                        "mercury-browser: history, bookmarks and downloads are not kept in \
                         this run: {reason}"
                    );
                    Arc::<str>::from(reason)
                });
                let _set = worker_unavailable.set(database.as_ref().err().cloned());
                let worker = Worker::new(
                    database,
                    clock,
                    delivery_sender,
                    worker_wake,
                    worker_wake_pending,
                    timing,
                );
                worker.run(&messages);
                // Closed before `finished`: from then on a send fails and is answered at once.
                drop(messages);
                let _sent = finished_sender.send(());
            });
        if let Err(error) = spawned {
            let reason = format!("the profile data thread could not start: {error}");
            eprintln!(
                "mercury-browser: history, bookmarks and downloads are not kept in this run: \
                 {reason}"
            );
            let _set = unavailable.set(Some(reason.into()));
        }
        Self {
            shared: Rc::new(Shared {
                requests,
                deliveries,
                pending: RefCell::new(HashMap::new()),
                progress: RefCell::new(HashMap::new()),
                listeners: RefCell::new(Vec::new()),
                undelivered: RefCell::new(Vec::new()),
                next_id: Cell::new(0),
                finished,
                wake_pending,
                wake,
                unavailable,
            }),
        }
    }

    /// Why the database is not available in this run, once the service has tried to open it;
    /// `None` while it is opening and when it is available.
    #[cfg_attr(not(test), expect(dead_code, reason = "shown by the history window"))]
    pub(crate) fn unavailable_reason(&self) -> Option<Arc<str>> {
        self.shared.unavailable.get().cloned().flatten()
    }

    /// Calls `callback` on the UI thread after each delivery in which a write to `area`
    /// succeeded, after that delivery's replies, with what changed. Call [`handle`] inside the
    /// callback rather than capturing a [`ProfileData`].
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "called by the bookmarks and downloads work")
    )]
    pub(crate) fn on_change(
        &self,
        area: Area,
        callback: impl Fn(&Change) + 'static,
    ) -> ChangeListener {
        let listener = ChangeListener(self.next_id());
        self.shared
            .listeners
            .borrow_mut()
            .push((listener, area, Rc::new(callback)));
        listener
    }

    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "called by the bookmarks and downloads work")
    )]
    pub(crate) fn remove_change_listener(&self, listener: ChangeListener) {
        self.shared
            .listeners
            .borrow_mut()
            .retain(|(registered, _, _)| *registered != listener);
    }

    fn next_id(&self) -> u64 {
        let id = self.shared.next_id.get();
        self.shared.next_id.set(id + 1);
        id
    }

    /// Runs `work` on the service thread and hands its result to `reply` on the UI thread. A
    /// successful `work` with an `area` is a write to it.
    fn request<T: Send + 'static>(
        &self,
        area: Option<Area>,
        work: impl FnOnce(&mut Database, i64) -> mercury_store::Result<T> + Send + 'static,
        reply: impl FnOnce(Result<T, ProfileDataError>) + 'static,
    ) {
        let id = self.next_id();
        self.shared.pending.borrow_mut().insert(
            id,
            Box::new(
                move |payload| match payload.downcast::<Result<T, ProfileDataError>>() {
                    Ok(result) => reply(*result),
                    Err(_) => log::error!("a profile data reply had the wrong type"),
                },
            ),
        );
        let fail: Failure = Box::new(|error| Box::new(Err::<T, _>(error)));
        let job = Job {
            reply_to: Some(id),
            area,
            work: Box::new(move |database, now| {
                let result = match database {
                    Ok(database) => work(database, now).map_err(ProfileDataError::Store),
                    Err(reason) => Err(ProfileDataError::Unavailable(Arc::clone(reason))),
                };
                JobOutcome {
                    wrote: result.is_ok(),
                    reply: Some(Box::new(result)),
                }
            }),
            fail: Some(fail),
        };
        self.send(Message::Job(job));
    }

    /// Runs `work` on the service thread without a reply; a failure is logged as `what`.
    fn submit(
        &self,
        area: Option<Area>,
        what: &'static str,
        work: impl FnOnce(&mut Database, i64) -> mercury_store::Result<()> + Send + 'static,
    ) {
        let job = Job {
            reply_to: None,
            area,
            work: Box::new(move |database, now| {
                let wrote = match database {
                    Ok(database) => match work(database, now) {
                        Ok(()) => true,
                        Err(
                            error @ (StoreError::InvalidArgument(_) | StoreError::InvalidState(_)),
                        ) => {
                            log::debug!("{what} refused: {error}");
                            false
                        }
                        Err(error) => {
                            log::warn!("{what} failed: {error}");
                            false
                        }
                    },
                    Err(_) => false,
                };
                JobOutcome { reply: None, wrote }
            }),
            fail: None,
        };
        self.send(Message::Job(job));
    }

    /// Sends a message; if the service has stopped, answers what it carried `Unavailable`.
    fn send(&self, message: Message) {
        let Err(mpsc::SendError(message)) = self.shared.requests.send(message) else {
            return;
        };
        let stopped =
            || ProfileDataError::Unavailable("the profile data service has stopped".into());
        let answer = match message {
            Message::Job(Job {
                reply_to: Some(id),
                fail: Some(fail),
                ..
            }) => Some((id, fail(stopped()))),
            Message::Background(job) => job.stopped(stopped()),
            _ => None,
        };
        if let Some(answer) = answer {
            self.shared.undelivered.borrow_mut().push(answer);
            if !self.shared.wake_pending.swap(true, Ordering::AcqRel) {
                (self.shared.wake)();
            }
        } else {
            log::debug!("profile data write dropped: the service has stopped");
        }
    }

    /// Runs the callbacks of every delivery so far: replies and progress in the order the
    /// service made them, then the change callbacks, once per area.
    pub(crate) fn deliver_replies(&self) {
        self.shared.wake_pending.store(false, Ordering::Release);
        let mut changed: BTreeMap<Area, Change> = BTreeMap::new();
        let undelivered = std::mem::take(&mut *self.shared.undelivered.borrow_mut());
        let deliveries = self
            .shared
            .deliveries
            .try_iter()
            .collect::<Vec<_>>()
            .into_iter()
            .chain(
                undelivered
                    .into_iter()
                    .map(|(id, payload)| Delivery::Reply { id, payload }),
            );
        for delivery in deliveries {
            match delivery {
                Delivery::Reply { id, payload } => {
                    self.shared.progress.borrow_mut().remove(&id);
                    let callback = self.shared.pending.borrow_mut().remove(&id);
                    if let Some(callback) = callback {
                        callback(payload);
                    }
                }
                Delivery::Progress { id, payload } => {
                    let callback = self.shared.progress.borrow().get(&id).cloned();
                    if let Some(callback) = callback {
                        callback(payload);
                    }
                }
                Delivery::Changed(change) => match changed.get_mut(&change.area) {
                    Some(merged) => merged.merge(change),
                    None => {
                        changed.insert(change.area, change);
                    }
                },
            }
        }
        for (area, change) in changed {
            let listeners = self
                .shared
                .listeners
                .borrow()
                .iter()
                .filter(|(_, listening, _)| *listening == area)
                .map(|(_, _, callback)| Rc::clone(callback))
                .collect::<Vec<_>>();
            for listener in listeners {
                listener(&change);
            }
        }
    }

    fn shut_down(&self, deadline: Instant) {
        if self.shared.requests.send(Message::Shutdown).is_err() {
            return;
        }
        let timeout = deadline.saturating_duration_since(Instant::now());
        match self.shared.finished.recv_timeout(timeout) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => {}
            Err(RecvTimeoutError::Timeout) => log::error!(
                "the profile database was still busy at the quit deadline; quitting without \
                 waiting for it"
            ),
        }
    }
}
