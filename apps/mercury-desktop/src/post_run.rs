//! The one sequence that ends a run once its event loop is over: the exit code, the final
//! saves, the engine's ordered shutdown, the window, the media buffers, the temporary profile
//! and the log. `main` runs it after the event loop returns; the macOS terminate handler runs
//! it inside AppKit's callback when the session is ending (`macos_terminate.rs`). It runs at
//! most once per process, whichever comes first.
//!
//! **Every step also runs inside AppKit's `applicationShouldTerminate:` callback on log-out.**
//! So no step may run Slint's or winit's event loop, call `invoke_from_event_loop` and wait for
//! it, or borrow Slint state; each must finish by its deadline without the event loop.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use mercury_ui::MainWindow;
use slint::Timer;

use crate::profile::ActiveProfile;
use crate::screenshot::ScreenshotResult;
use crate::shutdown::{Watchdog, exit_because_stuck};

/// One deadline for all final saves together. A save that misses it leaves its data as after a
/// crash, which is the safe state.
pub(crate) const FINAL_SAVES_BUDGET: Duration = Duration::from_secs(3);
/// The saves' deadline is cooperative; this watchdog ends the process if they overrun it.
const SAVES_WATCHDOG: Duration = Duration::from_secs(4);
/// What follows the engine's shutdown (the window, the media buffers, the profile).
const TAIL_WATCHDOG: Duration = Duration::from_secs(2);

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
    /// Saved, and its clear-history-on-exit hook run, by `finish_settings`.
    pub(crate) settings: Rc<crate::settings::SettingsHandle>,
}

/// Whether the post-run sequence has started.
#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PostRun {
    NotStarted,
    Running,
    Done,
}

thread_local! {
    static RUN_END: RunEndSlot<RunEnd> = const { RunEndSlot::new() };
}

/// Hands the run's end over. Call once, on the main thread, before the event loop starts.
pub(crate) fn prepare(run_end: RunEnd) {
    RUN_END.with(|slot| slot.prepare(run_end));
}

/// Ends the run and returns its exit code; `None` if it has already started (or was never
/// prepared). `event_loop_failed`: whether the event loop returned an error.
pub(crate) fn finish(event_loop_failed: bool) -> Option<u8> {
    RUN_END.with(|slot| {
        slot.finish(|run_end| {
            let mut steps = Steps::new(run_end, event_loop_failed);
            run_the_end(&mut steps, Instant::now(), |phase, exit_code| {
                Watchdog::start(phase.timeout(), move || {
                    exit_because_stuck(phase.description(), exit_code)
                })
            })
        })
    })
}

/// Whether the post-run sequence has started, is running or is done.
#[cfg(target_os = "macos")]
pub(crate) fn state() -> PostRun {
    RUN_END.with(RunEndSlot::state)
}

/// The steps of ending a run. Each lane's final save goes in its method on `Steps`, one line.
pub(crate) trait EndOfRun {
    /// The run's exit code; also reports a screenshot that was never taken.
    fn exit_code(&mut self) -> u8;
    /// Starts asynchronous saves; they run while the next steps work.
    fn begin_saves(&mut self);
    fn shut_down_profile_data(&mut self, deadline: Instant);
    fn collect_saves(&mut self, deadline: Instant);
    fn finish_settings(&mut self, deadline: Instant);
    /// Nothing may capture, pump or render while the engine shuts down.
    fn stop_timers(&mut self);
    /// Under its own watchdog (`ENGINE_SHUTDOWN_TIMEOUT`).
    fn shut_down_engine(&mut self, exit_code: u8);
    /// After the engine, which renders through the window's device.
    fn release_window(&mut self);
    fn remove_media_buffers(&mut self);
    fn remove_profile(&mut self);
    fn flush_log(&mut self);
}

/// Which part of the sequence a watchdog covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    Saves,
    Tail,
}

impl Phase {
    fn timeout(self) -> Duration {
        match self {
            Self::Saves => SAVES_WATCHDOG,
            Self::Tail => TAIL_WATCHDOG,
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Saves => "the final saves did not finish",
            Self::Tail => "the window, media buffers and profile were not released",
        }
    }
}

/// The sequence. Every save gets the same deadline, `now + FINAL_SAVES_BUDGET`; the saves and
/// the steps after the engine each run under a watchdog from `arm` (held until the phase ends).
///
/// Worst case before the process has ended: the saves' watchdog ends the process 4 s after the
/// saves begin, the engine's 5 s after its shutdown begins (the 0.5 s settle period included),
/// the tail's 2 s after it begins; each watchdog's own clean-up (removing the temporary
/// profile) may add up to `STUCK_CLEAN_UP_GRACE` (5 s). Only one watchdog can fire, so the
/// worst case is 4 + 5 + 2 + 5 = 16 s, against the 45 s loginwindow gives a foreground
/// application at log-out before it aborts the log-out (Apple, "The Life Cycle of a Daemon",
/// Terminating Processes). The usual case is about 0.6 s.
pub(crate) fn run_the_end<S: EndOfRun, W>(
    steps: &mut S,
    now: Instant,
    mut arm: impl FnMut(Phase, u8) -> W,
) -> u8 {
    let exit_code = steps.exit_code();
    let deadline = now + FINAL_SAVES_BUDGET;

    let saves = arm(Phase::Saves, exit_code);
    steps.begin_saves();
    steps.shut_down_profile_data(deadline);
    steps.collect_saves(deadline);
    steps.finish_settings(deadline);
    drop(saves);

    steps.stop_timers();
    steps.shut_down_engine(exit_code);

    let tail = arm(Phase::Tail, exit_code);
    steps.release_window();
    steps.remove_media_buffers();
    steps.remove_profile();
    drop(tail);

    steps.flush_log();
    exit_code
}

/// The real steps.
struct Steps {
    event_loop_failed: bool,
    window: Option<MainWindow>,
    timers: Vec<Timer>,
    screenshot_result: Option<Rc<Cell<ScreenshotResult>>>,
    profile: Option<ActiveProfile>,
    #[cfg(target_os = "macos")]
    media_buffers: Option<crate::media_buffers::MediaBuffers>,
    settings: Rc<crate::settings::SettingsHandle>,
}

impl Steps {
    fn new(run_end: RunEnd, event_loop_failed: bool) -> Self {
        let RunEnd {
            window,
            timers,
            screenshot_result,
            profile,
            #[cfg(target_os = "macos")]
            media_buffers,
            settings,
        } = run_end;
        Self {
            event_loop_failed,
            window: Some(window),
            timers,
            screenshot_result,
            profile: Some(profile),
            #[cfg(target_os = "macos")]
            media_buffers,
            settings,
        }
    }
}

impl EndOfRun for Steps {
    fn exit_code(&mut self) -> u8 {
        let screenshot = self.screenshot_result.as_ref().map(|result| result.get());
        if screenshot == Some(ScreenshotResult::Pending) && !self.event_loop_failed {
            eprintln!("mercury-browser: the browser closed before the screenshot was taken");
        }
        exit_code(self.event_loop_failed, screenshot)
    }

    // ---- Slots for final saves: one line each. ------------------------------------------

    fn begin_saves(&mut self) {
        // Slot (session): starts the final session write on the session worker.
        // crate::session::begin_final_save();
    }

    fn shut_down_profile_data(&mut self, deadline: Instant) {
        // Slot (history): closes the history database, waiting for its queued work.
        // crate::profile_data::shut_down(deadline);
        let _ = deadline;
    }

    fn collect_saves(&mut self, deadline: Instant) {
        // Slot (session): waits for the final session write.
        // crate::session::collect_final_save(deadline);
        let _ = deadline;
    }

    fn finish_settings(&mut self, deadline: Instant) {
        // Closes the settings window (keeping typed text), waits for the write in flight,
        // writes once more if needed, and runs the clear-history-on-exit hook.
        self.settings.finish_run(deadline);
    }

    // ---- End of the slots. ----------------------------------------------------------------

    fn stop_timers(&mut self) {
        self.timers.clear();
    }

    fn shut_down_engine(&mut self, exit_code: u8) {
        crate::engine::shut_down_engine(exit_code);
    }

    fn release_window(&mut self) {
        // Slot (menus): drops the About window and the menus' handle on the controller.
        // crate::menus::release();
        drop(self.window.take());
    }

    fn remove_media_buffers(&mut self) {
        // Copies of the media that was played.
        #[cfg(target_os = "macos")]
        drop(self.media_buffers.take());
    }

    fn remove_profile(&mut self) {
        // A temporary profile holds this run's cookies.
        drop(self.profile.take());
    }

    fn flush_log(&mut self) {
        log::logger().flush();
    }
}

/// The exit code of a run whose event loop has ended.
pub(crate) fn exit_code(event_loop_failed: bool, screenshot: Option<ScreenshotResult>) -> u8 {
    match (event_loop_failed, screenshot) {
        (true, _) => 1,
        (false, None) => 0,
        (false, Some(result)) => result.exit_code(),
    }
}

enum SlotState<T> {
    Empty,
    Ready(T),
    Running,
    Done,
}

/// Holds the run's end until the sequence takes it, once.
pub(crate) struct RunEndSlot<T>(RefCell<SlotState<T>>);

impl<T> RunEndSlot<T> {
    pub(crate) const fn new() -> Self {
        Self(RefCell::new(SlotState::Empty))
    }

    pub(crate) fn prepare(&self, value: T) {
        *self.0.borrow_mut() = SlotState::Ready(value);
    }

    /// Runs `end` with the value, the first time only; the slot is not borrowed meanwhile.
    pub(crate) fn finish(&self, end: impl FnOnce(T) -> u8) -> Option<u8> {
        let value = {
            let mut state = self.0.borrow_mut();
            match std::mem::replace(&mut *state, SlotState::Running) {
                SlotState::Ready(value) => value,
                other => {
                    *state = other;
                    return None;
                }
            }
        };
        let exit_code = end(value);
        *self.0.borrow_mut() = SlotState::Done;
        Some(exit_code)
    }

    #[cfg(any(target_os = "macos", test))]
    pub(crate) fn state(&self) -> PostRun {
        match *self.0.borrow() {
            SlotState::Empty | SlotState::Ready(_) => PostRun::NotStarted,
            SlotState::Running => PostRun::Running,
            SlotState::Done => PostRun::Done,
        }
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

    #[derive(Default)]
    struct Recorder {
        calls: Vec<String>,
        deadlines: Vec<Instant>,
    }

    impl Recorder {
        fn record(&mut self, call: &str) {
            self.calls.push(call.to_owned());
        }
    }

    impl EndOfRun for Recorder {
        fn exit_code(&mut self) -> u8 {
            self.record("exit code");
            3
        }
        fn begin_saves(&mut self) {
            self.record("begin saves");
        }
        fn shut_down_profile_data(&mut self, deadline: Instant) {
            self.record("profile data");
            self.deadlines.push(deadline);
        }
        fn collect_saves(&mut self, deadline: Instant) {
            self.record("collect saves");
            self.deadlines.push(deadline);
        }
        fn finish_settings(&mut self, deadline: Instant) {
            self.record("settings");
            self.deadlines.push(deadline);
        }
        fn stop_timers(&mut self) {
            self.record("stop timers");
        }
        fn shut_down_engine(&mut self, exit_code: u8) {
            self.record(&format!("engine {exit_code}"));
        }
        fn release_window(&mut self) {
            self.record("window");
        }
        fn remove_media_buffers(&mut self) {
            self.record("media buffers");
        }
        fn remove_profile(&mut self) {
            self.record("profile");
        }
        fn flush_log(&mut self) {
            self.record("flush log");
        }
    }

    fn run_recorded(now: Instant) -> (u8, Vec<String>, Vec<Instant>) {
        let mut steps = Recorder::default();
        let code = run_the_end(&mut steps, now, |_, _| ());
        (code, steps.calls, steps.deadlines)
    }

    #[test]
    fn saves_come_before_the_engine_and_the_profile_after_it() {
        let (code, calls, _) = run_recorded(Instant::now());

        assert_eq!(code, 3);
        assert_eq!(
            calls,
            vec![
                "exit code",
                "begin saves",
                "profile data",
                "collect saves",
                "settings",
                "stop timers",
                "engine 3",
                "window",
                "media buffers",
                "profile",
                "flush log",
            ]
        );
    }

    #[test]
    fn every_save_gets_the_same_deadline() {
        let now = Instant::now();
        let (_, _, deadlines) = run_recorded(now);

        assert_eq!(deadlines, vec![now + FINAL_SAVES_BUDGET; 3]);
    }

    /// Steps and watchdog phases in one list.
    struct Interleaved(Rc<RefCell<Vec<String>>>);

    impl EndOfRun for Interleaved {
        fn exit_code(&mut self) -> u8 {
            0
        }
        fn begin_saves(&mut self) {
            self.0.borrow_mut().push("begin saves".to_owned());
        }
        fn shut_down_profile_data(&mut self, _: Instant) {}
        fn collect_saves(&mut self, _: Instant) {}
        fn finish_settings(&mut self, _: Instant) {
            self.0.borrow_mut().push("settings".to_owned());
        }
        fn stop_timers(&mut self) {}
        fn shut_down_engine(&mut self, _: u8) {
            self.0.borrow_mut().push("engine".to_owned());
        }
        fn release_window(&mut self) {
            self.0.borrow_mut().push("window".to_owned());
        }
        fn remove_media_buffers(&mut self) {}
        fn remove_profile(&mut self) {
            self.0.borrow_mut().push("profile".to_owned());
        }
        fn flush_log(&mut self) {}
    }

    struct Disarm(&'static str, Rc<RefCell<Vec<String>>>);

    impl Drop for Disarm {
        fn drop(&mut self) {
            self.1.borrow_mut().push(format!("disarm {}", self.0));
        }
    }

    #[test]
    fn the_saves_and_the_tail_each_run_under_a_watchdog() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut steps = Interleaved(Rc::clone(&log));
        let watchdogs = Rc::clone(&log);

        run_the_end(&mut steps, Instant::now(), |phase, _| {
            let name = match phase {
                Phase::Saves => "saves",
                Phase::Tail => "tail",
            };
            watchdogs.borrow_mut().push(format!("arm {name}"));
            Disarm(name, Rc::clone(&watchdogs))
        });

        assert_eq!(
            *log.borrow(),
            vec![
                "arm saves",
                "begin saves",
                "settings",
                "disarm saves",
                "engine",
                "arm tail",
                "window",
                "profile",
                "disarm tail",
            ]
        );
    }

    #[test]
    fn a_second_finish_runs_nothing() {
        let slot = RunEndSlot::new();
        slot.prepare(Recorder::default());
        let mut runs = 0;

        let first = slot.finish(|mut steps| {
            runs += 1;
            run_the_end(&mut steps, Instant::now(), |_, _| ())
        });
        let second = slot.finish(|mut steps| {
            runs += 1;
            run_the_end(&mut steps, Instant::now(), |_, _| ())
        });

        assert_eq!(first, Some(3));
        assert_eq!(second, None);
        assert_eq!(runs, 1);
        assert_eq!(slot.state(), PostRun::Done);
    }

    #[test]
    fn a_finish_requested_while_one_runs_runs_nothing_and_sees_it_running() {
        let slot = RunEndSlot::new();
        slot.prepare(1u8);
        assert_eq!(slot.state(), PostRun::NotStarted);

        let outer = slot.finish(|value| {
            assert_eq!(slot.state(), PostRun::Running);
            assert_eq!(slot.finish(|_| 9), None);
            value
        });

        assert_eq!(outer, Some(1));
        assert_eq!(slot.state(), PostRun::Done);
    }

    #[test]
    fn an_unprepared_slot_finishes_nothing() {
        let slot = RunEndSlot::<u8>::new();

        assert_eq!(slot.finish(|_| 9), None);
        assert_eq!(slot.state(), PostRun::NotStarted);
    }
}
