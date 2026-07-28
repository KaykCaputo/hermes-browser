use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoadMilestone {
    HeadParsed,
    FirstFrame,
    Complete,
}

impl LoadMilestone {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::HeadParsed => "head parsed",
            Self::FirstFrame => "first frame ready",
            Self::Complete => "load complete",
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct PageLoadMetrics {
    started_at: Option<Instant>,
    head_parsed: bool,
    first_frame: bool,
    complete: bool,
}

impl PageLoadMetrics {
    pub(crate) fn start(&mut self, now: Instant) {
        self.started_at = Some(now);
        self.head_parsed = false;
        self.first_frame = false;
        self.complete = false;
    }

    pub(crate) fn record(&mut self, milestone: LoadMilestone, now: Instant) -> Option<Duration> {
        let started_at = self.started_at?;
        let already_recorded = match milestone {
            LoadMilestone::HeadParsed => &mut self.head_parsed,
            LoadMilestone::FirstFrame => &mut self.first_frame,
            LoadMilestone::Complete => &mut self.complete,
        };

        if *already_recorded {
            return None;
        }

        *already_recorded = true;
        Some(now.saturating_duration_since(started_at))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn milestones_are_measured_from_the_latest_start() {
        let first_start = Instant::now();
        let second_start = first_start + Duration::from_secs(10);
        let mut metrics = PageLoadMetrics::default();

        metrics.start(first_start);
        metrics.start(second_start);

        assert_eq!(
            metrics.record(
                LoadMilestone::HeadParsed,
                second_start + Duration::from_millis(125),
            ),
            Some(Duration::from_millis(125))
        );
    }

    #[test]
    fn each_milestone_is_recorded_only_once_per_load() {
        let start = Instant::now();
        let mut metrics = PageLoadMetrics::default();
        metrics.start(start);

        assert_eq!(
            metrics.record(LoadMilestone::FirstFrame, start + Duration::from_millis(50),),
            Some(Duration::from_millis(50))
        );
        assert_eq!(
            metrics.record(LoadMilestone::FirstFrame, start + Duration::from_millis(75),),
            None
        );
    }

    #[test]
    fn milestones_before_a_start_are_ignored() {
        let mut metrics = PageLoadMetrics::default();

        assert_eq!(
            metrics.record(LoadMilestone::Complete, Instant::now()),
            None
        );
    }

    #[test]
    fn a_new_load_resets_all_milestones() {
        let first_start = Instant::now();
        let second_start = first_start + Duration::from_secs(1);
        let mut metrics = PageLoadMetrics::default();
        metrics.start(first_start);
        let _ = metrics.record(
            LoadMilestone::Complete,
            first_start + Duration::from_millis(10),
        );

        metrics.start(second_start);

        assert_eq!(
            metrics.record(
                LoadMilestone::Complete,
                second_start + Duration::from_millis(20),
            ),
            Some(Duration::from_millis(20))
        );
    }
}
