//! The one sequence that ends a run once its event loop is over: the exit code, the final
//! saves, the engine's ordered shutdown, the window, the media buffers, the temporary profile
//! and the log. `main` runs it after the event loop returns; the macOS terminate handler runs
//! it inside AppKit's callback when the session is ending (`macos_terminate.rs`). It runs at
//! most once per process, whichever comes first.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use mercury_ui::MainWindow;
use slint::Timer;

use crate::engine::shut_down_engine;
use crate::profile::ActiveProfile;
use crate::screenshot::ScreenshotResult;

/// What the end of the run needs, handed over just before the event loop starts.
pub(crate) struct RunEnd {
    pub(crate) window: MainWindow,
    /// Timers that may capture, quit or pump; stopped before the engine shuts down.
    pub(crate) timers: Vec<Timer>,
    /// `Some` in a screenshot run.
    pub(crate) screenshot_result: Option<Rc<Cell<ScreenshotResult>>>,
    /// Dropped after the engine has saved its data: a temporary profile is deleted then.
    pub(crate) profile: ActiveProfile,
    #[cfg(target_os = "macos")]
    pub(crate) media_buffers: Option<crate::media_buffers::MediaBuffers>,
}

thread_local! {
    static RUN_END: OnceSlot<RunEnd> = const { OnceSlot::new() };
}

/// Hands the run's end over. Call once, on the main thread, before the event loop starts.
pub(crate) fn prepare(run_end: RunEnd) {
    RUN_END.with(|slot| slot.put(run_end));
}

/// Ends the run and returns its exit code; `None` if it has already been ended (or was never
/// prepared). `event_loop_failed`: whether the event loop returned an error.
pub(crate) fn finish(event_loop_failed: bool) -> Option<u8> {
    let run_end = RUN_END.with(OnceSlot::take)?;
    Some(run_end.finish(event_loop_failed))
}

impl RunEnd {
    fn finish(self, event_loop_failed: bool) -> u8 {
        let RunEnd {
            window,
            timers,
            screenshot_result,
            profile,
            #[cfg(target_os = "macos")]
            media_buffers,
        } = self;

        let screenshot = screenshot_result.map(|result| result.get());
        if screenshot == Some(ScreenshotResult::Pending) && !event_loop_failed {
            eprintln!("mercury-browser: the browser closed before the screenshot was taken");
        }
        let exit_code = exit_code(event_loop_failed, screenshot);

        final_saves(Instant::now() + FINAL_SAVES_BUDGET);

        // Nothing may capture, pump or render while the engine shuts down, and the engine must
        // be gone before the window (and the renderer whose device Servo uses) is destroyed.
        drop(timers);
        shut_down_engine(exit_code);
        drop(window);
        // Copies of the media that was played.
        #[cfg(target_os = "macos")]
        drop(media_buffers);
        // A temporary profile holds this run's cookies.
        drop(profile);
        log::logger().flush();
        exit_code
    }
}

/// One deadline for all final saves together, after which the engine shuts down under its own
/// watchdog (`ENGINE_SHUTDOWN_TIMEOUT`, 5 s). The worst case, about 3 s + 0.5 s settling + 5 s,
/// stays far below the 45 s loginwindow gives a foreground application to quit at log-out
/// before it aborts the log-out (Apple, "The Life Cycle of a Daemon", Terminating Processes).
/// A save that misses the deadline leaves its data as after a crash, which is the safe state.
pub(crate) const FINAL_SAVES_BUDGET: Duration = Duration::from_secs(3);

/// The saves other parts of the browser make when a run ends, while the engine still runs.
/// Each goes in its slot below, one line each, and must finish by `deadline`.
fn final_saves(deadline: Instant) {
    // ---- Slot: session, start. Starts the final session write; it runs on the session
    // worker while the next slot works.
    // let session_write = crate::session::start_final_write();

    // ---- Slot: profile_data. Closes the history database (waits for its queued work).
    // crate::profile_data::shut_down(deadline);

    // ---- Slot: session, collect. Waits for the final session write, by the same deadline.
    // session_write.collect(deadline);

    let _ = deadline;
}

/// The exit code of a run whose event loop has ended.
pub(crate) fn exit_code(event_loop_failed: bool, screenshot: Option<ScreenshotResult>) -> u8 {
    match (event_loop_failed, screenshot) {
        (true, _) => 1,
        (false, None) => 0,
        (false, Some(result)) => result.exit_code(),
    }
}

/// Holds a value that can be taken out once.
pub(crate) struct OnceSlot<T>(RefCell<Option<T>>);

impl<T> OnceSlot<T> {
    pub(crate) const fn new() -> Self {
        Self(RefCell::new(None))
    }

    pub(crate) fn put(&self, value: T) {
        *self.0.borrow_mut() = Some(value);
    }

    /// The value, the first time only. The slot is not borrowed while the caller uses it.
    pub(crate) fn take(&self) -> Option<T> {
        self.0.borrow_mut().take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_event_loop_exits_with_a_failure() {
        assert_eq!(exit_code(true, None), 1);
        assert_eq!(exit_code(true, Some(ScreenshotResult::Saved)), 1);
    }

    #[test]
    fn a_normal_run_exits_successfully() {
        assert_eq!(exit_code(false, None), 0);
    }

    #[test]
    fn a_screenshot_run_exits_with_its_result() {
        assert_eq!(exit_code(false, Some(ScreenshotResult::Saved)), 0);
        assert_eq!(exit_code(false, Some(ScreenshotResult::Pending)), 1);
        assert_eq!(exit_code(false, Some(ScreenshotResult::CaptureFailed)), 1);
        assert_eq!(exit_code(false, Some(ScreenshotResult::TimedOut)), 3);
    }

    #[test]
    fn a_slot_gives_its_value_once() {
        let slot = OnceSlot::new();
        slot.put("run end");

        assert_eq!(slot.take(), Some("run end"));
        assert_eq!(slot.take(), None);
    }

    #[test]
    fn an_empty_slot_gives_nothing() {
        assert_eq!(OnceSlot::<u8>::new().take(), None);
    }

    #[test]
    fn the_slot_is_free_while_its_value_is_used() {
        let slot = OnceSlot::new();
        slot.put(1);
        let value = slot.take();

        // A second caller (the terminate handler while `main` is finishing) finds it empty
        // instead of panicking on a borrow.
        assert_eq!(slot.take(), None);
        assert_eq!(value, Some(1));
    }
}
