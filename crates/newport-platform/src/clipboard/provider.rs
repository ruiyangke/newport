//! Lazy native clipboard publication. The synchronous OS callback must run in
//! a dedicated helper process, never in the application's UI process. Sources
//! queue work to the existing integration owner; they must not do network I/O
//! in `request` or `is_current`.
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

pub const MAX_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_FORMATS: usize = 256;
pub const MAX_WAIT: Duration = Duration::from_secs(20);

/// Keep the transport's source/revision/connection fence with the stable bytes
/// until the native write. An invalidated representation must return `None`.
pub trait Payload: Send {
    fn bytes(&self) -> Option<&[u8]>;
}

/// One exact value with publication and read authority kept by its source.
/// Implementations must make revocation visible without dispatching to the
/// native provider's thread, which may currently be waiting for a read.
/// Source revocation is terminal. A confirmed copy may retain read authority
/// after focus moves, but must not thereby gain permission to publish again.
pub trait Source: Send + Sync + 'static {
    fn is_current(&self) -> bool;
    fn request(&self, format: &str, reply: Reply);
    /// Claim one publication attempt before enqueueing native work. False does
    /// not transfer ownership and must not revoke an existing publication.
    /// An accepted claim is followed by confirmation or revocation, including
    /// when the publication future is dropped before it is polled. No I/O here.
    fn begin_publication(&self) -> bool {
        self.is_current()
    }
    /// Called by the native owner at its publication acknowledgment, before
    /// returning the publication handle. A source may atomically accept its
    /// copy lifetime here; false rejects the publication. Do not do I/O here.
    fn publication_confirmed(&self) -> bool {
        self.is_current()
    }
    /// Publication failed, was abandoned, or lost native ownership. This may
    /// be called more than once and must immediately fence source authority.
    /// Existing focus-bound sources may rely on their separate lease instead.
    fn publication_revoked(&self) {}
}

struct State {
    accepting: bool,
    result: Option<Option<Box<dyn Payload>>>,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    deadline: Instant,
}

/// A single response. Dropping it reports failure; a late response is discarded
/// after cancellation/deadline rather than populating a later clipboard offer.
pub struct Reply(Arc<Shared>);

impl Reply {
    pub fn is_cancelled(&self) -> bool {
        Instant::now() >= self.0.deadline
            || !self
                .0
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .accepting
    }

    pub fn complete(self, payload: Box<dyn Payload>) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.accepting && Instant::now() < self.0.deadline {
            state.result = Some(Some(payload));
        }
        self.0.changed.notify_all();
    }
}

impl Drop for Reply {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.accepting && state.result.is_none() {
            state.result = Some(None);
        }
        self.0.changed.notify_all();
    }
}

pub(super) struct Pending(Arc<Shared>);

impl Pending {
    fn expired(&self) -> bool {
        Instant::now() >= self.0.deadline
    }

    pub(super) fn new(wait: Duration) -> (Self, Reply) {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                accepting: true,
                result: None,
            }),
            changed: Condvar::new(),
            deadline: Instant::now() + wait.min(MAX_WAIT),
        });
        (Self(shared.clone()), Reply(shared))
    }

    /// Nonblocking consumption for a background owner which must keep pumping
    /// transport and cancellation while a source prepares its representation.
    pub(super) fn poll(&self) -> Option<Option<Box<dyn Payload>>> {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        if !state.accepting || self.expired() {
            state.accepting = false;
            state.result = None;
            return Some(None);
        }
        let result = state.result.take()?;
        state.accepting = false;
        Some(result)
    }

    #[cfg(any(target_os = "macos", test))]
    fn wait(&self, current: impl Fn() -> bool) -> Option<Box<dyn Payload>> {
        loop {
            // No native/source calls while holding the response lock.
            if self.expired() || !current() {
                return None;
            }
            let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
            if !state.accepting {
                return None;
            }
            if let Some(result) = state.result.take() {
                state.accepting = false;
                return result;
            }
            let remaining = self.0.deadline.saturating_duration_since(Instant::now());
            let _ = self
                .0
                .changed
                .wait_timeout(state, remaining.min(Duration::from_millis(10)));
        }
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        state.accepting = false;
        state.result = None;
        self.0.changed.notify_all();
    }
}

/// Read an already published source without recursively asking the native
/// pasteboard for its own promise. This uses the same bounded source queue as
/// native demand. Cancellation drops the pending reply; no blocking OS call or
/// additional clipboard owner is involved.
pub async fn read_source(source: &dyn Source, format: &str) -> Option<Box<dyn Payload>> {
    if !source.is_current() {
        return None;
    }
    let (pending, reply) = Pending::new(MAX_WAIT);
    source.request(format, reply);
    let mut timer = tokio::time::interval(Duration::from_millis(10));
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        if !source.is_current() {
            return None;
        }
        if let Some(result) = pending.poll() {
            return result.filter(|data| source.is_current() && data.bytes().is_some());
        }
        timer.tick().await;
    }
}

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::Publication;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct Bytes(Vec<u8>);
    impl Payload for Bytes {
        fn bytes(&self) -> Option<&[u8]> {
            Some(&self.0)
        }
    }

    #[test]
    fn nonblocking_poll_keeps_pending_sources_live_and_consumes_once() {
        let (pending, reply) = Pending::new(MAX_WAIT);
        assert!(pending.poll().is_none());
        assert!(!reply.is_cancelled());
        reply.complete(Box::new(Bytes(vec![7])));
        let data = pending.poll().unwrap().unwrap();
        assert_eq!(data.bytes(), Some(&[7][..]));
        assert!(matches!(pending.poll(), Some(None)));
        let (pending, reply) = Pending::new(MAX_WAIT);
        drop(pending);
        assert!(reply.is_cancelled());
        reply.complete(Box::new(Bytes(vec![8])));
    }

    #[test]
    fn cancellation_wakes_without_waiting_for_a_late_source() {
        let (pending, reply) = Pending::new(Duration::from_secs(5));
        let current = Arc::new(AtomicBool::new(true));
        let revoke = current.clone();
        let worker = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            revoke.store(false, Ordering::Release);
            while !reply.is_cancelled() {
                std::thread::yield_now();
            }
            reply.complete(Box::new(Bytes(vec![1])));
        });
        let start = Instant::now();
        assert!(pending.wait(|| current.load(Ordering::Acquire)).is_none());
        drop(pending);
        worker.join().unwrap();
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn deadline_and_abandoned_response_fail_without_data() {
        let (pending, reply) = Pending::new(Duration::from_millis(20));
        assert!(pending.wait(|| true).is_none());
        assert!(reply.is_cancelled());
        reply.complete(Box::new(Bytes(vec![1])));
        assert!(pending.wait(|| true).is_none());
        let (pending, reply) = Pending::new(MAX_WAIT);
        drop(reply);
        assert!(pending.wait(|| true).is_none());
    }

    #[test]
    fn response_is_consumed_once_and_keeps_payload_fence() {
        let (pending, reply) = Pending::new(MAX_WAIT);
        reply.complete(Box::new(Bytes(vec![1, 2, 3])));
        let data = pending.wait(|| true).unwrap();
        assert_eq!(data.bytes().unwrap(), &[1, 2, 3]);
        assert!(!pending.0.state.lock().unwrap().accepting);
        assert!(pending.wait(|| true).is_none());
    }
}
