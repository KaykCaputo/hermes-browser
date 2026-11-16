use std::cell::{Cell, RefCell};
use std::error::Error;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

use image::{ExtendedColorType, ImageFormat};
use mercury_ui::MainWindow;
use slint::{ComponentHandle, Timer, TimerMode};
use talaria_core::{BrowserController, LoadState};

use crate::cli::ScreenshotOptions;

pub(crate) const LOAD_TIMEOUT: Duration = Duration::from_secs(30);
const EXIT_SUCCESS: u8 = 0;
const EXIT_FAILURE: u8 = 1;
const EXIT_LOAD_TIMEOUT: u8 = 3;
const POLL_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScreenshotStep {
    Wait,
    Capture,
    TimedOut,
    /// A capture or timeout was already reported; there is nothing left to do.
    Finished,
}

/// What a screenshot run achieved. Only a written PNG counts as success, so a run that ends
/// for any other reason (the window was closed, the event loop stopped) exits non-zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScreenshotResult {
    Pending,
    Saved,
    CaptureFailed,
    TimedOut,
}

impl ScreenshotResult {
    pub(crate) const fn exit_code(self) -> u8 {
        match self {
            Self::Saved => EXIT_SUCCESS,
            Self::Pending | Self::CaptureFailed => EXIT_FAILURE,
            Self::TimedOut => EXIT_LOAD_TIMEOUT,
        }
    }
}

/// Decides when the screenshot is taken: `delay` after the active tab has finished loading,
/// or never if it has not loaded within `LOAD_TIMEOUT`.
#[derive(Debug)]
pub(crate) struct ScreenshotSchedule {
    started_at: Instant,
    delay: Duration,
    loaded_at: Option<Instant>,
    finished: bool,
}

impl ScreenshotSchedule {
    pub(crate) const fn new(started_at: Instant, delay: Duration) -> Self {
        Self {
            started_at,
            delay,
            loaded_at: None,
            finished: false,
        }
    }

    pub(crate) fn poll(&mut self, now: Instant, page_loaded: bool) -> ScreenshotStep {
        if self.finished {
            return ScreenshotStep::Finished;
        }

        let step = self.next_step(now, page_loaded);
        self.finished = matches!(step, ScreenshotStep::Capture | ScreenshotStep::TimedOut);
        step
    }

    fn next_step(&mut self, now: Instant, page_loaded: bool) -> ScreenshotStep {
        if !page_loaded {
            self.loaded_at = None;
            return if now.saturating_duration_since(self.started_at) >= LOAD_TIMEOUT {
                ScreenshotStep::TimedOut
            } else {
                ScreenshotStep::Wait
            };
        }

        let loaded_at = *self.loaded_at.get_or_insert(now);
        if now.saturating_duration_since(loaded_at) >= self.delay {
            ScreenshotStep::Capture
        } else {
            ScreenshotStep::Wait
        }
    }
}

/// Polls the active tab and, once the schedule says so, saves the window as a PNG, records
/// the outcome in `result` and quits the event loop. The returned timer must be kept alive
/// while the event loop runs.
pub(crate) fn start_screenshot(
    window: &MainWindow,
    controller: Rc<RefCell<BrowserController>>,
    options: ScreenshotOptions,
    result: Rc<Cell<ScreenshotResult>>,
) -> Timer {
    let weak_window = window.as_weak();
    let mut schedule = ScreenshotSchedule::new(Instant::now(), options.delay);
    let timer = Timer::default();

    timer.start(TimerMode::Repeated, POLL_INTERVAL, move || {
        let _slint = crate::callback_scope::InSlintCallback::enter();
        let Some(window) = weak_window.upgrade() else {
            return;
        };
        let page_loaded = matches!(
            controller.borrow().active_state().load_state(),
            LoadState::Loaded
        );

        let outcome = match schedule.poll(Instant::now(), page_loaded) {
            ScreenshotStep::Wait | ScreenshotStep::Finished => return,
            ScreenshotStep::Capture => match save_window_png(&window, &options.path) {
                Ok((width, height)) => {
                    log::info!(
                        "saved a {width}x{height} screenshot to {}",
                        options.path.display()
                    );
                    ScreenshotResult::Saved
                }
                Err(error) => {
                    eprintln!(
                        "mercury-browser: failed to save the screenshot to {}: {error}",
                        options.path.display()
                    );
                    ScreenshotResult::CaptureFailed
                }
            },
            ScreenshotStep::TimedOut => {
                eprintln!(
                    "mercury-browser: the page did not finish loading within {} seconds; no screenshot was taken",
                    LOAD_TIMEOUT.as_secs()
                );
                ScreenshotResult::TimedOut
            }
        };

        result.set(outcome);
        if let Err(error) = slint::quit_event_loop() {
            eprintln!(
                "mercury-browser: failed to quit the event loop after the screenshot ({error}); exiting"
            );
            crate::profile::remove_temporary_profile();
            std::process::exit(i32::from(outcome.exit_code()));
        }
    });

    timer
}

fn save_window_png(window: &MainWindow, path: &Path) -> Result<(u32, u32), Box<dyn Error>> {
    let pixels = window.window().take_snapshot()?;
    let (width, height) = (pixels.width(), pixels.height());
    image::save_buffer_with_format(
        path,
        pixels.as_bytes(),
        width,
        height,
        ExtendedColorType::Rgba8,
        ImageFormat::Png,
    )?;
    Ok((width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_that_never_captured_exits_with_a_failure() {
        assert_ne!(ScreenshotResult::Pending.exit_code(), 0);
        assert_eq!(ScreenshotResult::Pending.exit_code(), 1);
    }

    #[test]
    fn only_a_saved_screenshot_exits_successfully() {
        assert_eq!(ScreenshotResult::Saved.exit_code(), 0);
        assert_eq!(ScreenshotResult::CaptureFailed.exit_code(), 1);
        assert_eq!(ScreenshotResult::TimedOut.exit_code(), 3);
    }

    const DELAY: Duration = Duration::from_millis(250);

    #[test]
    fn waits_while_the_page_is_loading() {
        let start = Instant::now();
        let mut schedule = ScreenshotSchedule::new(start, DELAY);

        assert_eq!(
            schedule.poll(start + Duration::from_secs(5), false),
            ScreenshotStep::Wait
        );
    }

    #[test]
    fn captures_once_the_delay_after_loading_has_passed() {
        let start = Instant::now();
        let loaded = start + Duration::from_secs(2);
        let mut schedule = ScreenshotSchedule::new(start, DELAY);

        assert_eq!(schedule.poll(loaded, true), ScreenshotStep::Wait);
        assert_eq!(
            schedule.poll(loaded + DELAY - Duration::from_millis(1), true),
            ScreenshotStep::Wait
        );
        assert_eq!(schedule.poll(loaded + DELAY, true), ScreenshotStep::Capture);
    }

    #[test]
    fn a_new_load_restarts_the_delay() {
        let start = Instant::now();
        let mut schedule = ScreenshotSchedule::new(start, DELAY);

        assert_eq!(schedule.poll(start, true), ScreenshotStep::Wait);
        let reloading = start + Duration::from_millis(200);
        assert_eq!(schedule.poll(reloading, false), ScreenshotStep::Wait);
        let reloaded = start + Duration::from_millis(300);
        assert_eq!(schedule.poll(reloaded, true), ScreenshotStep::Wait);
        assert_eq!(
            schedule.poll(start + DELAY, true),
            ScreenshotStep::Wait,
            "the delay counts from the latest load"
        );
        assert_eq!(
            schedule.poll(reloaded + DELAY, true),
            ScreenshotStep::Capture
        );
    }

    #[test]
    fn times_out_when_the_page_never_loads() {
        let start = Instant::now();
        let mut schedule = ScreenshotSchedule::new(start, DELAY);

        assert_eq!(
            schedule.poll(start + LOAD_TIMEOUT - Duration::from_millis(1), false),
            ScreenshotStep::Wait
        );
        assert_eq!(
            schedule.poll(start + LOAD_TIMEOUT, false),
            ScreenshotStep::TimedOut
        );
    }

    #[test]
    fn the_schedule_finishes_after_a_capture() {
        let start = Instant::now();
        let mut schedule = ScreenshotSchedule::new(start, DELAY);

        assert_eq!(schedule.poll(start, true), ScreenshotStep::Wait);
        assert_eq!(schedule.poll(start + DELAY, true), ScreenshotStep::Capture);
        assert_eq!(
            schedule.poll(start + DELAY * 2, true),
            ScreenshotStep::Finished
        );
        assert_eq!(
            schedule.poll(start + LOAD_TIMEOUT * 2, false),
            ScreenshotStep::Finished
        );
    }

    #[test]
    fn the_schedule_finishes_after_a_timeout() {
        let start = Instant::now();
        let mut schedule = ScreenshotSchedule::new(start, DELAY);

        assert_eq!(
            schedule.poll(start + LOAD_TIMEOUT, false),
            ScreenshotStep::TimedOut
        );
        assert_eq!(
            schedule.poll(start + LOAD_TIMEOUT, true),
            ScreenshotStep::Finished
        );
        assert_eq!(
            schedule.poll(start + LOAD_TIMEOUT + DELAY, true),
            ScreenshotStep::Finished
        );
    }

    #[test]
    fn a_page_that_loaded_before_the_timeout_is_still_captured() {
        let start = Instant::now();
        let loaded = start + LOAD_TIMEOUT - Duration::from_millis(10);
        let mut schedule = ScreenshotSchedule::new(start, DELAY);

        assert_eq!(schedule.poll(loaded, true), ScreenshotStep::Wait);
        assert_eq!(schedule.poll(loaded + DELAY, true), ScreenshotStep::Capture);
    }
}
