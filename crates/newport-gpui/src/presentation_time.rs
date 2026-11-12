//! Local display time is independent of cached remote data. Only labels used by
//! the last rendered view are compared; virtualized rows outside it add no work.
use newport_domain::services::{self, TimerUnit};
use std::cell::RefCell;

#[derive(Clone, PartialEq, Eq)]
enum Label {
    Elapsed(i64),
    Until(i64),
    Timer(TimerUnit),
}
impl Label {
    fn text(&self, now: i64) -> String {
        match self {
            Self::Elapsed(at) => services::duration(now.saturating_sub(*at)),
            Self::Until(at) if *at > now => {
                format!("in {}", services::duration(at.saturating_sub(now)))
            }
            Self::Until(_) => "Due now".into(),
            Self::Timer(timer) => services::timer_note(timer, now),
        }
    }
}

#[derive(Default)]
struct Frame {
    now: i64,
    checked: i64,
    labels: Vec<(Label, String)>,
}

#[derive(Default)]
pub(super) struct Clock(RefCell<Frame>);
impl Clock {
    pub fn begin_frame(&self, now: i64) {
        let mut frame = self.0.borrow_mut();
        frame.now = now;
        frame.checked = now;
        frame.labels.clear();
    }

    pub fn now(&self) -> i64 {
        self.0.borrow().now
    }

    fn label(&self, label: Label) -> String {
        let mut frame = self.0.borrow_mut();
        let text = label.text(frame.now);
        if !frame.labels.iter().any(|(previous, _)| previous == &label) {
            frame.labels.push((label, text.clone()));
        }
        text
    }

    pub fn elapsed(&self, at: i64) -> String {
        self.label(Label::Elapsed(at))
    }

    pub fn until(&self, at: i64) -> String {
        self.label(Label::Until(at))
    }

    pub fn timer(&self, timer: &TimerUnit) -> String {
        // Names, descriptions and unit links don't participate in the clock.
        self.label(Label::Timer(TimerUnit {
            active: timer.active.clone(),
            next: timer.next,
            last: timer.last,
            ..Default::default()
        }))
    }

    /// The caller supplies wall time or its last successful observation time.
    /// Check at most once per second, including after a backwards clock change.
    pub fn changed(&self, now: i64) -> bool {
        let mut frame = self.0.borrow_mut();
        if now.div_euclid(1000) == frame.checked.div_euclid(1000) {
            return false;
        }
        frame.checked = now;
        let mut changed = false;
        for (label, previous) in &mut frame.labels {
            let next = label.text(now);
            if *previous != next {
                *previous = next;
                changed = true;
            }
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_formatted_changes_in_the_current_view_invalidate() {
        let clock = Clock::default();
        clock.begin_frame(70_000);
        assert_eq!(clock.elapsed(0), "1m");
        assert!(!clock.changed(71_000));
        assert!(!clock.changed(119_000));
        assert!(clock.changed(120_000));
        assert!(!clock.changed(120_000));
        // Rendering another pane discards clocks that are no longer shown.
        clock.begin_frame(120_000);
        assert!(!clock.changed(180_000));
    }

    #[test]
    fn deadline_transitions_and_backwards_time_are_published_once() {
        let clock = Clock::default();
        clock.begin_frame(0);
        assert_eq!(clock.until(5_000), "in 5s");
        assert!(!clock.changed(200));
        assert!(clock.changed(1_000));
        assert!(clock.changed(5_000));
        assert!(!clock.changed(6_000));
        assert!(clock.changed(2_000));
        assert!(!clock.changed(2_000));
    }

    #[test]
    fn timer_labels_reuse_domain_semantics_and_duplicate_sources() {
        let clock = Clock::default();
        let timer = TimerUnit {
            active: "active".into(),
            next: Some(3_000),
            ..Default::default()
        };
        clock.begin_frame(0);
        assert_eq!(clock.timer(&timer), services::timer_note(&timer, 0));
        assert_eq!(clock.timer(&timer), "in 3s");
        assert_eq!(clock.0.borrow().labels.len(), 1);
        assert!(clock.changed(3_000));
        assert!(!clock.changed(4_000));
        clock.begin_frame(4_000);
        assert_eq!(clock.timer(&timer), "waiting");
        assert!(!clock.changed(60_000));
    }
}
