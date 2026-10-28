//! An opt-in, coalesced indication that a process needs a real GUI display.
//! Clipboard clients also connect to X11/Wayland; connecting alone isn't intent.
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

#[derive(Default)]
pub(crate) struct Notices {
    enabled: AtomicBool,
    pending: AtomicBool,
    last: Mutex<Option<Instant>>,
}
impl Notices {
    pub fn enable(&self) {
        self.enabled.store(true, Ordering::Release);
    }
    pub fn signal(&self) {
        if !self.enabled.load(Ordering::Acquire) {
            return;
        }
        let mut last = self.last.lock().unwrap();
        if last.is_some_and(|at| at.elapsed() < Duration::from_secs(3)) {
            return;
        }
        *last = Some(Instant::now());
        self.pending.store(true, Ordering::Release);
    }
    pub fn take(&self) -> bool {
        self.pending.swap(false, Ordering::AcqRel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn opt_in_and_coalescing() {
        let notices = Notices::default();
        notices.signal();
        assert!(!notices.take());
        notices.enable();
        notices.signal();
        assert!(notices.take());
        for _ in 0..20 {
            notices.signal();
        }
        assert!(!notices.take());
        *notices.last.lock().unwrap() = Some(Instant::now() - Duration::from_secs(4));
        notices.signal();
        assert!(notices.take());
    }
}
