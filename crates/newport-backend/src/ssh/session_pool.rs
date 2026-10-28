//! Bounded session admission. Extra transports are owned by the primary
//! connection and are used only before sending an exec/subsystem request.
use super::{Connection, Server, client};
use anyhow::{Context, Result, bail};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use tokio::sync::{Semaphore, watch};
use tokio::time::Instant;

pub(super) const EXTRA_TRANSPORTS: usize = 4;
const WAITERS: usize = 32;
const CAPACITY_WAIT: Duration = Duration::from_secs(5);
const RECLAIM_GRACE: Duration = Duration::from_millis(100);

pub(super) struct Pool {
    server: Server,
    opening: tokio::sync::Mutex<()>,
    waiters: Semaphore,
    children: Mutex<Vec<Arc<Connection>>>,
    stopped: watch::Sender<bool>,
    admitted: AtomicBool,
    dial: Mutex<DialState>,
}
impl Pool {
    pub(super) fn new(server: &Server) -> Self {
        Self {
            server: server.clone(),
            opening: tokio::sync::Mutex::new(()),
            waiters: Semaphore::new(WAITERS),
            children: Mutex::new(Vec::new()),
            stopped: watch::channel(false).0,
            admitted: AtomicBool::new(false),
            dial: Mutex::default(),
        }
    }
    pub(super) fn is_stopped(&self) -> bool {
        *self.stopped.borrow()
    }
    #[cfg(test)]
    pub(super) fn child_count(&self) -> usize {
        self.children.lock().unwrap().len()
    }
    #[cfg(test)]
    pub(super) fn waiter_count(&self) -> usize {
        WAITERS - self.waiters.available_permits()
    }
    pub(super) fn stop(&self) {
        self.stopped.send_replace(true);
        for child in self.children.lock().unwrap().drain(..) {
            child.abort();
        }
    }
    pub(super) fn retry(&self) {
        self.dial.lock().unwrap().retry();
    }
    async fn open(&self, primary: &Connection) -> Result<russh::Channel<client::Msg>> {
        let _waiter = self.waiters.try_acquire().map_err(|_| capacity_error())?;
        let mut stopped = self.stopped.subscribe();
        if *stopped.borrow_and_update() || primary.handle.is_closed() {
            bail!(russh::Error::Disconnect);
        }
        tokio::select! {
            biased;
            _ = stopped.changed() => Err(russh::Error::Disconnect.into()),
            result = self.admit(primary) => result,
        }
    }
    async fn admit(&self, primary: &Connection) -> Result<russh::Channel<client::Msg>> {
        let _opening = tokio::time::timeout(CAPACITY_WAIT, self.opening.lock())
            .await
            .map_err(|_| capacity_error())?;
        let deadline = Instant::now() + CAPACITY_WAIT;
        let mut reclaim = true;
        loop {
            if let Some(channel) = primary.try_session().await? {
                self.admitted.store(true, Ordering::Release);
                return Ok(channel);
            }
            let children = {
                let mut children = self.children.lock().unwrap();
                children.retain(|child| !child.handle.is_closed() && !child.sessions.is_stopped());
                children.clone()
            };
            for child in &children {
                match child.try_session().await {
                    Ok(Some(channel)) => return Ok(channel),
                    Ok(None) => (),
                    Err(error)
                        if super::is_transient_connection_error(&error)
                            || child.handle.is_closed()
                            || child.sessions.is_stopped() =>
                    {
                        // Only channel admission ran: no exec/subsystem bytes
                        // exist to replay. Retire this child and try another.
                        child.abort();
                    }
                    Err(error) => return Err(error),
                }
            }
            // Reuse any available transport before disturbing cached workers.
            // Only if the whole pool is full, ask idle exec-backed monitor/Git
            // lanes to yield before authenticating another connection.
            if reclaim {
                primary
                    .channel_pressure
                    .send_modify(|epoch| *epoch = epoch.wrapping_add(1));
                reclaim = false;
                tokio::time::sleep(RECLAIM_GRACE).await;
                continue;
            }
            // A server refusing its very first session (including MaxSessions=0)
            // must not cause a connection fan-out. A fresh secondary that also
            // refuses its first channel is discarded for the same reason.
            let live = children
                .iter()
                .filter(|child| !child.sessions.is_stopped())
                .count();
            if live < EXTRA_TRANSPORTS && self.admitted.load(Ordering::Acquire) {
                let attempt = DialAttempt::begin(&self.dial)?;
                // Authentication is the exceptional path. Keeping its large
                // future out of every command's state avoids multiplying stack
                // usage through application startup and service composition.
                let (candidate, channel) = attempt.complete(
                    async {
                        let candidate =
                            Arc::new(Box::pin(Connection::connect_once(&self.server)).await?);
                        let channel = candidate.try_session().await?.ok_or_else(capacity_error)?;
                        Ok((candidate, channel))
                    }
                    .await,
                )?;
                let mut children = self.children.lock().unwrap();
                if self.is_stopped() || primary.handle.is_closed() {
                    candidate.abort();
                    bail!(russh::Error::Disconnect);
                }
                children.push(candidate);
                return Ok(channel);
            }
            if Instant::now() >= deadline {
                return Err(capacity_error());
            }
            tokio::time::sleep(RECLAIM_GRACE).await;
        }
    }
}

// A declined sign request must not be retried by unrelated background work.
// Explicit server retry clears the failure without closing healthy transports.
// Network errors back off; cancellation also fences the abandoned attempt so a
// request deadline cannot turn into a repeated local approval prompt.
#[derive(Default)]
struct DialState {
    revision: u64,
    failures: u32,
    failure: Option<(SharedFailure, Option<Instant>)>,
}
impl DialState {
    fn retry(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.failures = 0;
        self.failure = None;
    }
}
#[derive(Clone, Debug)]
struct SharedFailure(Arc<anyhow::Error>);
impl std::fmt::Display for SharedFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Additional SSH connection failed")
    }
}
impl SharedFailure {
    fn error(&self) -> anyhow::Error {
        let error = anyhow::Error::new(self.clone());
        // anyhow's typed context downcasts do not traverse an arbitrary
        // std::error::Error source. Preserve these public retry categories.
        if self.0.is::<super::auth::AgentApprovalRequired>() {
            error.context(super::auth::AgentApprovalRequired)
        } else if self.0.is::<super::auth::SelectedAgentUnavailable>() {
            error.context(super::auth::SelectedAgentUnavailable)
        } else if self.0.is::<CapacityUnavailable>() {
            error.context(CapacityUnavailable)
        } else {
            error
        }
    }
}
impl std::error::Error for SharedFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.0.as_ref().as_ref())
    }
}
struct DialAttempt<'a> {
    state: &'a Mutex<DialState>,
    revision: u64,
    completed: bool,
}
impl<'a> DialAttempt<'a> {
    fn begin(state: &'a Mutex<DialState>) -> Result<Self> {
        let dial = state.lock().unwrap();
        if let Some((error, retry_at)) = &dial.failure
            && retry_at.is_none_or(|at| Instant::now() < at)
        {
            return Err(error.error());
        }
        Ok(Self {
            state,
            revision: dial.revision,
            completed: false,
        })
    }
    fn complete<T>(mut self, result: Result<T>) -> Result<T> {
        self.completed = true;
        let mut state = self.state.lock().unwrap();
        match result {
            Ok(value) => {
                if state.revision == self.revision {
                    state.failures = 0;
                    state.failure = None;
                }
                Ok(value)
            }
            Err(error) => {
                let retryable = super::is_transient_connection_error(&error);
                let error = SharedFailure(Arc::new(error));
                if state.revision == self.revision {
                    state.failures = (state.failures + 1).min(5);
                    let delay = Duration::from_secs((5 << (state.failures - 1)).min(60));
                    state.failure =
                        Some((error.clone(), retryable.then(|| Instant::now() + delay)));
                }
                Err(error.error())
            }
        }
    }
}
impl Drop for DialAttempt<'_> {
    fn drop(&mut self) {
        if !self.completed {
            let mut state = self.state.lock().unwrap();
            if state.revision == self.revision {
                state.failure = Some((
                    SharedFailure(Arc::new(anyhow::anyhow!(
                        "Additional SSH connection setup was cancelled. Retry the server operation explicitly to allow another connection."
                    ))),
                    None,
                ));
            }
        }
    }
}

#[derive(Debug)]
struct CapacityUnavailable;
impl std::fmt::Display for CapacityUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SSH session capacity unavailable. Retry after active operations finish.")
    }
}
impl std::error::Error for CapacityUnavailable {}
fn capacity_error() -> anyhow::Error {
    CapacityUnavailable.into()
}

#[derive(Default)]
pub(super) struct Probe {
    retry_at: Option<Instant>,
    failures: u32,
}
impl Probe {
    fn ready(&self) -> bool {
        self.retry_at.is_none_or(|at| Instant::now() >= at)
    }
    fn refused(&mut self) {
        self.failures = (self.failures + 1).min(5);
        let delay = Duration::from_millis((100 << (self.failures - 1)).min(1000));
        self.retry_at = Some(Instant::now() + delay);
    }
}

impl Connection {
    pub(super) async fn open_session(&self) -> Result<russh::Channel<client::Msg>> {
        self.sessions.open(self).await
    }
    // This method never opens another connection. Pool admission is serialized
    // on the primary, so secondary transports cannot recursively grow a pool.
    async fn try_session(&self) -> Result<Option<russh::Channel<client::Msg>>> {
        if self.sessions.is_stopped() || self.handle.is_closed() {
            bail!(russh::Error::Disconnect);
        }
        if !self.session_probe.lock().unwrap().ready() {
            return Ok(None);
        }
        match tokio::time::timeout(CAPACITY_WAIT, self.handle.channel_open_session())
            .await
            .context("SSH session admission timed out")?
        {
            Ok(channel) => {
                *self.session_probe.lock().unwrap() = Probe::default();
                Ok(Some(channel))
            }
            Err(russh::Error::ChannelOpenFailure(
                russh::ChannelOpenFailure::ConnectFailed
                | russh::ChannelOpenFailure::ResourceShortage,
            )) => {
                self.session_probe.lock().unwrap().refused();
                Ok(None)
            }
            Err(error) => Err(error.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn secondary_approval_waits_and_denial_requires_explicit_retry() {
        let state = Mutex::new(DialState::default());
        let attempt = DialAttempt::begin(&state).unwrap();
        tokio::time::sleep(Duration::from_secs(60)).await;
        let error = attempt
            .complete::<()>(Err(super::super::auth::AgentApprovalRequired.into()))
            .unwrap_err();
        assert!(error.is::<super::super::auth::AgentApprovalRequired>());
        tokio::time::advance(Duration::from_secs(3600)).await;
        let error = DialAttempt::begin(&state).err().unwrap();
        assert!(error.is::<super::super::auth::AgentApprovalRequired>());
        assert!(!super::super::is_transient_connection_error(&error));
        state.lock().unwrap().retry();
        DialAttempt::begin(&state)
            .unwrap()
            .complete(Ok(()))
            .unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn secondary_network_errors_back_off_and_success_resets_backoff() {
        let state = Mutex::new(DialState::default());
        for seconds in [5, 10, 20, 40, 60, 60] {
            let attempt = DialAttempt::begin(&state).unwrap();
            let error = attempt
                .complete::<()>(Err(std::io::Error::from(
                    std::io::ErrorKind::ConnectionReset,
                )
                .into()))
                .unwrap_err();
            assert!(super::super::is_transient_connection_error(&error));
            tokio::time::advance(Duration::from_secs(seconds - 1)).await;
            assert!(DialAttempt::begin(&state).is_err());
            tokio::time::advance(Duration::from_secs(1)).await;
        }
        DialAttempt::begin(&state)
            .unwrap()
            .complete(Ok(()))
            .unwrap();
        assert_eq!(state.lock().unwrap().failures, 0);
    }

    #[test]
    fn cancelled_secondary_dial_does_not_repeat_or_overwrite_explicit_retry() {
        let state = Mutex::new(DialState::default());
        drop(DialAttempt::begin(&state).unwrap());
        assert!(DialAttempt::begin(&state).is_err());
        state.lock().unwrap().retry();
        let old = DialAttempt::begin(&state).unwrap();
        state.lock().unwrap().retry();
        drop(old);
        DialAttempt::begin(&state)
            .unwrap()
            .complete(Ok(()))
            .unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn capacity_probe_backs_off_and_recovers_without_busy_polling() {
        let mut probe = Probe::default();
        assert!(probe.ready());
        for delay in [100, 200, 400, 800, 1000, 1000] {
            probe.refused();
            assert!(!probe.ready());
            tokio::time::advance(Duration::from_millis(delay - 1)).await;
            assert!(!probe.ready());
            tokio::time::advance(Duration::from_millis(1)).await;
            assert!(probe.ready());
        }
        probe = Probe::default();
        probe.refused();
        tokio::time::advance(RECLAIM_GRACE).await;
        assert!(probe.ready());
    }
}
