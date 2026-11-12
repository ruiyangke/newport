//! Cancellation and native mutation share one linearization point.
use std::sync::atomic::{AtomicU8, Ordering};
const WAITING: u8 = 0;
const COMMITTED: u8 = 1;
const CANCELED: u8 = 2;
#[derive(Default)]
pub(crate) struct Commit(AtomicU8);
impl Commit {
    pub(crate) fn cancel(&self) {
        let _ = self
            .0
            .compare_exchange(WAITING, CANCELED, Ordering::AcqRel, Ordering::Acquire);
    }
    pub(crate) fn pending(&self) -> bool {
        self.0.load(Ordering::Acquire) == WAITING
    }
    /// Call only after opening the native clipboard and checking its sequence.
    pub(crate) fn begin(&self) -> bool {
        self.0
            .compare_exchange(WAITING, COMMITTED, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_before_commit_blocks_native_mutation() {
        let commit = Commit::default();
        commit.cancel();
        assert!(!commit.pending());
        assert!(!commit.begin());
    }
    #[test]
    fn an_accepted_native_commit_cannot_be_misreported_as_canceled() {
        let commit = Commit::default();
        assert!(commit.begin());
        commit.cancel();
        assert_eq!(commit.0.load(Ordering::Acquire), COMMITTED);
        assert!(!commit.pending());
        assert!(!commit.begin());
    }
}
