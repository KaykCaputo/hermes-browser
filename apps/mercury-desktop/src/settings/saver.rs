//! When the settings are written: after a quiet period following the last change, never more
//! than one write at a time, and never after the file turned out to be from a newer version.
//! The clock is passed in, so the rules are tested without waiting.

use std::time::{Duration, Instant};

/// How long the settings must stay unchanged before they are written. Typing in a field or
/// clicking through choices then gives one write, not one per keystroke.
pub(crate) const SAVE_QUIET_PERIOD: Duration = Duration::from_secs(1);

/// How a write ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WriteOutcome {
    Saved,
    /// The write failed; the next change, or quitting, tries again.
    Failed,
    /// The file on disk is from a newer version: never write again in this run.
    Refused,
}

#[derive(Debug)]
pub(crate) struct SaveScheduler {
    quiet: Duration,
    /// The time of the last change not yet handed to a write.
    changed_at: Option<Instant>,
    in_flight: bool,
    /// A write failed and nothing has been written since.
    unsaved: bool,
    refused: bool,
}

impl SaveScheduler {
    pub(crate) fn new(quiet: Duration, may_save: bool) -> Self {
        Self {
            quiet,
            changed_at: None,
            in_flight: false,
            unsaved: false,
            refused: !may_save,
        }
    }

    /// Records a change made at `now`; the quiet period starts again.
    pub(crate) fn changed(&mut self, now: Instant) {
        if !self.refused {
            self.changed_at = Some(now);
        }
    }

    /// How long from `now` until a write may start, or `None` if nothing is waiting or a write
    /// is in flight (its end calls for the next check).
    pub(crate) fn next_check(&self, now: Instant) -> Option<Duration> {
        if self.in_flight || self.refused {
            return None;
        }
        let due = self.changed_at? + self.quiet;
        Some(due.saturating_duration_since(now))
    }

    /// Whether to start a write now. When it returns true a write is in flight until
    /// `finished` is called.
    pub(crate) fn start_write(&mut self, now: Instant) -> bool {
        if self.next_check(now) != Some(Duration::ZERO) {
            return false;
        }
        self.changed_at = None;
        self.in_flight = true;
        true
    }

    pub(crate) fn finished(&mut self, outcome: WriteOutcome) {
        self.in_flight = false;
        match outcome {
            WriteOutcome::Saved => self.unsaved = false,
            WriteOutcome::Failed => self.unsaved = true,
            WriteOutcome::Refused => {
                self.refused = true;
                self.changed_at = None;
                self.unsaved = false;
            }
        }
    }

    /// Whether a write is in flight.
    pub(crate) fn is_writing(&self) -> bool {
        self.in_flight
    }

    /// Whether a last write is needed at quit, once the write in flight (if any) has ended.
    pub(crate) fn needs_final_write(&self) -> bool {
        !self.refused && (self.changed_at.is_some() || self.unsaved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const QUIET: Duration = Duration::from_secs(1);

    fn ms(milliseconds: u64) -> Duration {
        Duration::from_millis(milliseconds)
    }

    #[test]
    fn nothing_is_written_without_a_change() {
        let start = Instant::now();
        let mut scheduler = SaveScheduler::new(QUIET, true);

        assert_eq!(scheduler.next_check(start), None);
        assert!(!scheduler.start_write(start + ms(5000)));
        assert!(!scheduler.needs_final_write());
    }

    #[test]
    fn a_change_is_written_after_the_quiet_period() {
        let start = Instant::now();
        let mut scheduler = SaveScheduler::new(QUIET, true);

        scheduler.changed(start);

        assert_eq!(scheduler.next_check(start), Some(QUIET));
        assert!(!scheduler.start_write(start + ms(999)));
        assert_eq!(scheduler.next_check(start + ms(400)), Some(ms(600)));
        assert!(scheduler.start_write(start + ms(1000)));
    }

    #[test]
    fn each_change_restarts_the_quiet_period() {
        let start = Instant::now();
        let mut scheduler = SaveScheduler::new(QUIET, true);

        scheduler.changed(start);
        scheduler.changed(start + ms(800));

        assert!(!scheduler.start_write(start + ms(1500)));
        assert!(scheduler.start_write(start + ms(1800)));
    }

    #[test]
    fn only_one_write_is_in_flight() {
        let start = Instant::now();
        let mut scheduler = SaveScheduler::new(QUIET, true);
        scheduler.changed(start);
        assert!(scheduler.start_write(start + QUIET));

        scheduler.changed(start + ms(1100));

        assert_eq!(scheduler.next_check(start + ms(5000)), None);
        assert!(!scheduler.start_write(start + ms(5000)));

        scheduler.finished(WriteOutcome::Saved);

        assert_eq!(scheduler.next_check(start + ms(5000)), Some(Duration::ZERO));
        assert!(scheduler.start_write(start + ms(5000)));
    }

    #[test]
    fn a_written_change_needs_no_final_write() {
        let start = Instant::now();
        let mut scheduler = SaveScheduler::new(QUIET, true);
        scheduler.changed(start);
        assert!(scheduler.start_write(start + QUIET));

        scheduler.finished(WriteOutcome::Saved);

        assert_eq!(scheduler.next_check(start + ms(9000)), None);
        assert!(!scheduler.needs_final_write());
    }

    #[test]
    fn a_change_inside_the_quiet_period_is_written_at_quit() {
        let start = Instant::now();
        let mut scheduler = SaveScheduler::new(QUIET, true);

        scheduler.changed(start);

        assert!(scheduler.needs_final_write());
    }

    #[test]
    fn a_failed_write_is_retried_at_the_next_change_or_at_quit_not_in_a_loop() {
        let start = Instant::now();
        let mut scheduler = SaveScheduler::new(QUIET, true);
        scheduler.changed(start);
        assert!(scheduler.start_write(start + QUIET));

        scheduler.finished(WriteOutcome::Failed);

        assert_eq!(scheduler.next_check(start + ms(9000)), None);
        assert!(scheduler.needs_final_write());
        scheduler.changed(start + ms(9000));
        assert!(scheduler.start_write(start + ms(10_000)));
        scheduler.finished(WriteOutcome::Saved);
        assert!(!scheduler.needs_final_write());
    }

    #[test]
    fn a_refused_write_stops_every_later_write() {
        let start = Instant::now();
        let mut scheduler = SaveScheduler::new(QUIET, true);
        scheduler.changed(start);
        assert!(scheduler.start_write(start + QUIET));

        scheduler.finished(WriteOutcome::Refused);
        scheduler.changed(start + ms(2000));

        assert_eq!(scheduler.next_check(start + ms(2000)), None);
        assert!(!scheduler.start_write(start + ms(9000)));
        assert!(!scheduler.needs_final_write());
    }

    #[test]
    fn a_run_that_may_not_save_never_writes() {
        let start = Instant::now();
        let mut scheduler = SaveScheduler::new(QUIET, false);

        scheduler.changed(start);

        assert_eq!(scheduler.next_check(start), None);
        assert!(!scheduler.start_write(start + ms(9000)));
        assert!(!scheduler.needs_final_write());
    }
}
