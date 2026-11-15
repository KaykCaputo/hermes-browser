//! The public face of the crate: requests, events, and the manager that runs transfers.

use std::collections::HashMap;
use std::fmt;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use futures_util::FutureExt;
use tokio::sync::watch;

use crate::transfer::{Outcome, Transfer};

/// How long dropping the manager waits for cancelled transfers to clean up.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);

/// Identifies one download for the lifetime of its `DownloadManager`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DownloadId(pub u64);

/// What to download, where, and with which credentials.
#[derive(Clone)]
pub struct DownloadRequest {
    /// The URL to fetch. Only `http` and `https` are accepted.
    pub url: url::Url,
    /// Sent as `Referer`, without its fragment or credentials, and not from `https` to `http`.
    pub referrer: Option<url::Url>,
    /// Sent as `User-Agent` on every request; use the engine's so servers see one browser.
    pub user_agent: String,
    /// Sent as `Cookie` only while requests stay on the first request's host (and never from
    /// `https` down to `http`).
    pub cookie_header: Option<String>,
    /// The folder the file is saved in. Created if missing; nothing is written outside it.
    pub destination_dir: PathBuf,
    /// A name the page proposed, for example from `<a download="…">`. Still sanitised.
    pub suggested_name: Option<String>,
    /// Continue an interrupted download instead of starting a new one.
    pub resume: Option<ResumeFrom>,
}

impl fmt::Debug for DownloadRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DownloadRequest")
            .field("url", &self.url.as_str())
            .field("referrer", &self.referrer.as_ref().map(url::Url::as_str))
            .field("user_agent", &self.user_agent)
            // Cookies are credentials: never print them.
            .field(
                "cookie_header",
                &self.cookie_header.as_ref().map(|_| "<redacted>"),
            )
            .field("destination_dir", &self.destination_dir)
            .field("suggested_name", &self.suggested_name)
            .field("resume", &self.resume)
            .finish()
    }
}

/// Where an interrupted download left off, from its `Started` event.
///
/// `partial_path` must be `target_path` plus `.part`, and both must be directly inside the
/// request's `destination_dir`; otherwise the request fails without touching either file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResumeFrom {
    pub partial_path: PathBuf,
    pub target_path: PathBuf,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

/// What happens to a download, in order: `Started` **at most once**, any number of `Progress`,
/// then exactly one of `Finished`, `Failed` or `Cancelled`.
///
/// A download that fails or is cancelled before the server's response has been accepted and the
/// partial file created (bad URL, HTTP error, refused redirect, invalid resume, cancel while
/// connecting) produces **only** its terminal event, with no `Started` before it. A panic inside
/// the transfer, including one raised by `on_event` while delivering `Started` or `Progress`,
/// ends it with `Failed { resumable: false }` saying it failed internally, after its partial
/// file is removed. If `on_event` panics while delivering the terminal event, the panic is
/// logged and nothing more is sent for that download: it is already over and stays over.
#[derive(Clone, Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "the event shape is the crate's published contract; events are few and short-lived"
)]
pub enum DownloadEvent {
    /// The server answered and the partial file exists. Persist the validators to resume later.
    Started {
        id: DownloadId,
        file_name: String,
        target_path: PathBuf,
        partial_path: PathBuf,
        total_bytes: Option<u64>,
        mime_type: Option<String>,
        final_url: url::Url,
        etag: Option<String>,
        last_modified: Option<String>,
    },
    /// At most about four a second, and once more when the body is complete.
    Progress {
        id: DownloadId,
        received_bytes: u64,
        total_bytes: Option<u64>,
        bytes_per_second: u64,
    },
    /// The file is complete at `target_path`, which may differ from the `Started` one if that
    /// name was taken in the meantime.
    Finished {
        id: DownloadId,
        target_path: PathBuf,
        received_bytes: u64,
    },
    /// `resumable` is `true` exactly when the partial file was kept and the server supports
    /// resuming; otherwise nothing is left on disk.
    Failed {
        id: DownloadId,
        error: String,
        resumable: bool,
    },
    /// Stopped by `cancel` or by dropping the manager; the partial file is removed.
    Cancelled { id: DownloadId },
}

/// Network time limits. The defaults are 30 seconds to connect and 60 seconds without data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timeouts {
    /// Limit on establishing a connection.
    pub connect: Duration,
    /// Limit on waiting for a response or for the next piece of the body. Exceeding it fails the
    /// transfer like a dropped connection.
    pub idle: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(30),
            idle: Duration::from_secs(60),
        }
    }
}

type EventSink = Arc<dyn Fn(DownloadEvent) + Send + Sync>;

struct Shared {
    on_event: EventSink,
    /// Running transfers and the switch that cancels each.
    running: Mutex<HashMap<DownloadId, watch::Sender<bool>>>,
    /// Tasks that have not yet returned, including ones emitting their terminal event.
    alive: Mutex<usize>,
    all_done: Condvar,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A running task's bookkeeping, undone in `Drop` so that it holds even if the task unwinds:
/// the id leaves the running set and the task stops counting as alive.
struct TaskGuard {
    shared: Arc<Shared>,
    id: DownloadId,
}

impl TaskGuard {
    fn leave_running(&self) {
        lock(&self.shared.running).remove(&self.id);
    }
}

impl Drop for TaskGuard {
    fn drop(&mut self) {
        self.leave_running();
        let mut alive = lock(&self.shared.alive);
        *alive = alive.saturating_sub(1);
        self.shared.all_done.notify_all();
    }
}

/// Runs downloads on a small runtime of its own (two worker threads).
///
/// Each download produces `DownloadEvent`s through the `on_event` callback: `Started` at most
/// once (never for a download that fails or is cancelled before the response), `Progress`, and
/// exactly one terminal event.
///
/// Dropping the manager cancels every running download, waits up to three seconds for their
/// partial files to be removed, and returns. Dropping it from **inside** `on_event` means the
/// task delivering that event is among those waited for, so the drop takes the full three
/// seconds before it returns; drop it from another thread instead.
pub struct DownloadManager {
    runtime: Option<tokio::runtime::Runtime>,
    client: reqwest::Client,
    timeouts: Timeouts,
    shared: Arc<Shared>,
    next_id: AtomicU64,
}

impl DownloadManager {
    /// `on_event` is called on a runtime thread; it must be cheap and must not block. It may
    /// panic without harming the manager (see `DownloadEvent`), but should not.
    pub fn new(on_event: impl Fn(DownloadEvent) + Send + Sync + 'static) -> std::io::Result<Self> {
        Self::with_timeouts(Timeouts::default(), on_event)
    }

    /// Like `new`, with other time limits (tests use milliseconds).
    pub fn with_timeouts(
        timeouts: Timeouts,
        on_event: impl Fn(DownloadEvent) + Send + Sync + 'static,
    ) -> std::io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("mercury-downloads")
            .enable_all()
            .build()?;
        let client = {
            let _context = runtime.enter();
            reqwest::Client::builder()
                .connect_timeout(timeouts.connect)
                // Redirects are followed by hand so each hop's scheme and host can be checked.
                .redirect(reqwest::redirect::Policy::none())
                .referer(false)
                // Servo makes its own connections without a proxy; downloads behave the same.
                .no_proxy()
                .build()
                .map_err(std::io::Error::other)?
        };
        Ok(Self {
            runtime: Some(runtime),
            client,
            timeouts,
            shared: Arc::new(Shared {
                on_event: Arc::new(on_event),
                running: Mutex::new(HashMap::new()),
                alive: Mutex::new(0),
                all_done: Condvar::new(),
            }),
            next_id: AtomicU64::new(1),
        })
    }

    /// Starts a download and returns at once; progress arrives through `on_event`.
    pub fn start(&self, request: DownloadRequest) -> DownloadId {
        let id = DownloadId(self.next_id.fetch_add(1, Ordering::Relaxed));
        let (cancel, cancelled) = watch::channel(false);
        lock(&self.shared.running).insert(id, cancel);
        *lock(&self.shared.alive) += 1;

        let transfer = Transfer {
            id,
            request,
            client: self.client.clone(),
            timeouts: self.timeouts,
            emit: Arc::clone(&self.shared.on_event),
        };
        let guard = TaskGuard {
            shared: Arc::clone(&self.shared),
            id,
        };
        if let Some(runtime) = &self.runtime {
            runtime.spawn(async move {
                // `Transfer::run` turns its own panics into an internal failure after removing
                // the partial file; this is the last resort if that handling itself panics.
                let outcome = AssertUnwindSafe(transfer.run(cancelled))
                    .catch_unwind()
                    .await
                    .unwrap_or_else(|_| Outcome::internal_failure());
                // Leave the running set before reporting, so `active()` is already right when
                // the terminal event arrives.
                guard.leave_running();
                let event = outcome.into_event(id);
                let on_event = &guard.shared.on_event;
                if std::panic::catch_unwind(AssertUnwindSafe(|| on_event(event))).is_err() {
                    // The download is over either way; a second terminal event would break the
                    // "exactly one" promise, so the panic is only logged.
                    log::error!("on_event panicked while delivering the terminal event of {id:?}");
                }
                drop(guard);
            });
        }
        id
    }

    /// Stops the transfer and deletes its partial file. Idempotent; unknown ids are ignored.
    pub fn cancel(&self, id: DownloadId) {
        if let Some(cancel) = lock(&self.shared.running).get(&id) {
            let _ = cancel.send(true);
        }
    }

    /// Number of transfers still running.
    pub fn active(&self) -> usize {
        lock(&self.shared.running).len()
    }
}

impl Drop for DownloadManager {
    fn drop(&mut self) {
        for cancel in lock(&self.shared.running).values() {
            let _ = cancel.send(true);
        }
        let deadline = Instant::now() + SHUTDOWN_GRACE;
        let mut alive = lock(&self.shared.alive);
        while *alive > 0 {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                log::warn!("{} download task(s) did not stop in time", *alive);
                break;
            }
            alive = self
                .shared
                .all_done
                .wait_timeout(alive, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        drop(alive);
        if let Some(runtime) = self.runtime.take() {
            // Never blocks, and is allowed even when dropped from inside an async context.
            runtime.shutdown_background();
        }
    }
}

impl Outcome {
    fn into_event(self, id: DownloadId) -> DownloadEvent {
        match self {
            Outcome::Finished {
                target_path,
                received_bytes,
            } => DownloadEvent::Finished {
                id,
                target_path,
                received_bytes,
            },
            Outcome::Failed { error, resumable } => DownloadEvent::Failed {
                id,
                error,
                resumable,
            },
            Outcome::Cancelled => DownloadEvent::Cancelled { id },
        }
    }
}
