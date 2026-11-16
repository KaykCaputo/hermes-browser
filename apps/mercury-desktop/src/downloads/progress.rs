//! How often a download's progress is written to the profile database.
//!
//! `mercury-downloads` reports progress up to four times a second; the list shows every report
//! from memory, and the database only needs enough to show a sensible size after a crash. One
//! write per download per `PROGRESS_WRITE_INTERVAL` (1 s), plus the final count, which the
//! finish path writes regardless.

use std::collections::HashMap;
use std::time::{Duration, Instant};

pub(crate) const PROGRESS_WRITE_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Default)]
pub(crate) struct ProgressWrites {
    last: HashMap<String, Instant>,
}

impl ProgressWrites {
    /// Whether a progress report for `guid` at `now` should be written. The first report of a
    /// download always is.
    pub(crate) fn due(&mut self, guid: &str, now: Instant) -> bool {
        match self.last.get_mut(guid) {
            Some(last) if now.saturating_duration_since(*last) < PROGRESS_WRITE_INTERVAL => false,
            Some(last) => {
                *last = now;
                true
            }
            None => {
                self.last.insert(guid.to_owned(), now);
                true
            }
        }
    }

    /// The download is over; its next report (after a resume) is written at once.
    pub(crate) fn forget(&mut self, guid: &str) {
        self.last.remove(guid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_is_written_at_most_once_a_second_per_download() {
        let start = Instant::now();
        let at = |millis| start + Duration::from_millis(millis);
        let mut writes = ProgressWrites::default();
        assert!(writes.due("a", at(0)));
        assert!(!writes.due("a", at(250)));
        assert!(!writes.due("a", at(999)));
        // Another download has its own clock.
        assert!(writes.due("b", at(500)));
        assert!(writes.due("a", at(1000)));
        assert!(!writes.due("a", at(1500)));
        assert!(!writes.due("b", at(1499)));
        assert!(writes.due("b", at(1500)));
    }

    #[test]
    fn a_finished_download_starts_afresh() {
        let start = Instant::now();
        let mut writes = ProgressWrites::default();
        assert!(writes.due("a", start));
        writes.forget("a");
        assert!(writes.due("a", start + Duration::from_millis(10)));
    }
}
