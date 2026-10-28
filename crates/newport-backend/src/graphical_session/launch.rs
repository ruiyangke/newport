//! Application intent and acknowledgment, independent of the graphical wire.
use std::{future::Future, num::NonZeroU64, pin::Pin, time::Duration};

pub use newport_platform::ipc::applications::{
    Application, Elsewhere, Ending, Notice, Outcome, State, Target,
};

pub type Reply = Pin<Box<dyn Future<Output = Outcome> + Send>>;

/// Observes one already-submitted request. Dropping this handle or timing out
/// does not cancel/replay the request or release its driver's pending demand.
/// It owns no transport, display lease or presentation consumer.
pub struct Pending {
    reply: Reply,
    deadline: tokio::time::Instant,
}
impl Pending {
    pub(super) fn new(reply: Reply) -> Self {
        Self {
            reply,
            deadline: tokio::time::Instant::now() + Duration::from_secs(30),
        }
    }
    pub async fn outcome(self) -> Outcome {
        tokio::time::timeout_at(self.deadline, self.reply)
            .await
            .unwrap_or_else(|_| {
                Outcome::Unknown("Application launch timed out; do not replay automatically".into())
            })
    }
}

/// The adapter must publish pending demand before returning, even if neither
/// this result nor its reply is retained. IDs must be consecutive from one,
/// shared with legacy launches, and never reused within a display. The reply
/// must not own the transport.
pub struct Submitted {
    pub request: NonZeroU64,
    pub reply: Reply,
}
pub struct Started {
    pub application: Application,
    pub pending: Pending,
}
