//! Connection supervision shared by server services and dedicated terminals.
//! Each instance owns one transport, its demand and its retry policy.
use crate::{
    model::Server,
    ssh::{ConnectFailure, ExecSession},
};
use serde::Serialize;
use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{
    sync::{Notify, watch},
    time::Instant,
};

const RETIRED: &str = "Server session changed. Reconnect before retrying.";
type Dial = Arc<
    dyn Fn() -> Pin<Box<dyn Future<Output = Result<ExecSession, ConnectFailure>> + Send>>
        + Send
        + Sync,
>;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub status: &'static str,
    pub generation: u64,
    pub attempt: u32,
    pub error: Option<String>,
    pub retryable: bool,
    pub demand: usize,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            status: "disconnected",
            generation: 0,
            attempt: 0,
            error: None,
            retryable: false,
            demand: 0,
        }
    }
}
#[derive(Default)]
struct State {
    snapshot: Snapshot,
    connection: Option<Arc<ExecSession>>,
    retry_at: Option<Instant>,
}
#[derive(Clone, Copy, Default)]
struct Retry {
    version: u64,
    fresh: u64,
}
struct Shared {
    publication: Option<crate::connection_state::Publisher>,
    state: Mutex<State>,
    demand: watch::Sender<usize>,
    changes: watch::Sender<u64>,
    retry: watch::Sender<Retry>,
    generation: Arc<AtomicU64>,
    connected: Arc<Notify>,
    permissions: Arc<crate::integration_permissions::Gate>,
}
impl Shared {
    fn update(&self, change: impl FnOnce(&mut State)) {
        {
            let mut state = self.state.lock().unwrap();
            change(&mut state);
            if let Some(publication) = &self.publication {
                publication.transport(state.snapshot.clone());
            }
        }
        self.changes
            .send_modify(|version| *version = version.wrapping_add(1));
    }
}
pub(crate) struct Demand(Arc<Shared>);
impl Demand {
    fn new(shared: &Arc<Shared>) -> Self {
        shared.demand.send_modify(|count| *count += 1);
        Self(shared.clone())
    }
}
impl Drop for Demand {
    fn drop(&mut self) {
        self.0.demand.send_modify(|count| *count -= 1);
    }
}
pub(crate) struct Transport {
    shared: Arc<Shared>,
    dial: Dial,
    retired: watch::Receiver<bool>,
    recovery: crate::recovery::Recovery,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}
impl Transport {
    /// A terminal shares profile retirement and recovery policy, but never the
    /// app transport, connectivity publisher, generation or integration gate.
    pub fn terminal(
        server: Server,
        retired: watch::Receiver<bool>,
        admissions: Arc<tokio::sync::Semaphore>,
    ) -> Self {
        let mut transport = Self::new(
            server.clone(),
            retired,
            Arc::new(AtomicU64::new(0)),
            Arc::new(Notify::new()),
            Arc::new(crate::integration_permissions::Gate::new(false, false)),
            None,
        );
        // Opening many tabs must not create a burst of unauthenticated sockets
        // or simultaneous agent prompts. Connected terminals run independently.
        transport.dial = Arc::new(move || {
            let server = server.clone();
            let admissions = admissions.clone();
            Box::pin(async move {
                let _permit = admissions
                    .acquire()
                    .await
                    .expect("terminal admissions open");
                ExecSession::connect_session(&server).await
            })
        });
        transport
    }

    pub fn new(
        server: Server,
        retired: watch::Receiver<bool>,
        generation: Arc<AtomicU64>,
        connected: Arc<Notify>,
        permissions: Arc<crate::integration_permissions::Gate>,
        publication: Option<crate::connection_state::Publisher>,
    ) -> Self {
        let dial: Dial = Arc::new(move || {
            let server = server.clone();
            Box::pin(async move { ExecSession::connect_session(&server).await })
        });
        Self {
            shared: Arc::new(Shared {
                publication,
                state: Mutex::default(),
                demand: watch::channel(0).0,
                changes: watch::channel(0).0,
                retry: watch::channel(Retry::default()).0,
                generation,
                connected,
                permissions,
            }),
            dial,
            retired,
            recovery: crate::recovery::subscribe(),
            task: Mutex::new(None),
        }
    }
    pub fn snapshot(&self) -> Snapshot {
        let mut snapshot = self.shared.state.lock().unwrap().snapshot.clone();
        snapshot.demand = *self.shared.demand.borrow();
        snapshot
    }
    pub fn demand(&self) -> Demand {
        let demand = Demand::new(&self.shared);
        let mut task = self.task.lock().unwrap();
        if !*self.retired.borrow() && task.as_ref().is_none_or(|task| task.is_finished()) {
            *task = Some(tokio::spawn(run(
                self.shared.clone(),
                self.dial.clone(),
                self.retired.clone(),
                self.recovery.clone(),
            )));
        }
        demand
    }
    pub fn observe(&self) -> Option<(Arc<ExecSession>, u64)> {
        if *self.retired.borrow() {
            return None;
        }
        let state = self.shared.state.lock().unwrap();
        state
            .connection
            .as_ref()
            .filter(|c| !c.is_closed())
            .map(|c| (c.clone(), state.snapshot.generation))
    }
    pub async fn acquire(&self) -> Result<(Arc<ExecSession>, u64, Demand), String> {
        let demand = self.demand();
        let mut changes = self.shared.changes.subscribe();
        let mut retired = self.retired.clone();
        loop {
            changes.borrow_and_update();
            if *retired.borrow() {
                return Err(RETIRED.into());
            }
            {
                let state = self.shared.state.lock().unwrap();
                if let Some(connection) = &state.connection
                    && !connection.is_closed()
                {
                    return Ok((connection.clone(), state.snapshot.generation, demand));
                }
                if let Some(error) = &state.snapshot.error
                    && (!state.snapshot.retryable
                        || state.retry_at.is_some_and(|at| at > Instant::now()))
                {
                    return Err(error.clone());
                }
            }
            tokio::select! {
                biased;
                _ = retired.changed() => return Err(RETIRED.into()),
                _ = changes.changed() => {},
            }
        }
    }
    /// Request-oriented consumers report a blocked authentication attempt.
    pub async fn ready(&self) -> Result<(), String> {
        self.wait_ready(false, |_| {}).await
    }
    /// Keep enabled services subscribed through authentication failures. This
    /// observes the shared retry policy; it never clears a failure or retries
    /// authentication. Only an explicit retry can unblock a permanent error.
    pub async fn follow_ready(&self, waiting: impl Fn(Option<String>)) -> Result<(), String> {
        self.wait_ready(true, waiting).await
    }
    async fn wait_ready(
        &self,
        persistent: bool,
        waiting: impl Fn(Option<String>),
    ) -> Result<(), String> {
        let _demand = self.demand();
        let mut changes = self.shared.changes.subscribe();
        let mut retired = self.retired.clone();
        loop {
            changes.borrow_and_update();
            if *retired.borrow() {
                return Err(RETIRED.into());
            }
            if self.observe().is_some() {
                return Ok(());
            }
            let snapshot = self.snapshot();
            if !snapshot.retryable
                && let Some(error) = snapshot.error
            {
                if !persistent {
                    return Err(error);
                }
                waiting(Some(error));
            } else {
                waiting(None);
            }
            tokio::select! {
                _ = retired.changed() => return Err(RETIRED.into()),
                _ = changes.changed() => {},
            }
        }
    }
    pub fn retry(&self) {
        self.request_retry(false);
    }
    /// Another connection to the same server just succeeded. A transport
    /// waiting out its backoff retries now, so the server isn't shown as
    /// unreachable after it came back; any other transport is left alone.
    pub fn reachable(&self) {
        let waiting = self.shared.state.lock().unwrap().retry_at.is_some();
        if waiting {
            self.retry();
        }
    }
    /// A user explicitly retrying a failed checkout needs a new foreground
    /// signing request, not an older background approval that may be hidden.
    pub fn retry_fresh(&self) {
        self.request_retry(true);
    }
    fn request_retry(&self, fresh: bool) {
        if let Some((connection, _)) = self.observe() {
            connection.retry_session_admission();
            return;
        }
        self.shared.update(|state| {
            state.snapshot.error = None;
            state.retry_at = None;
        });
        self.shared.retry.send_modify(|retry| {
            retry.version = retry.version.wrapping_add(1);
            if fresh {
                retry.fresh = retry.version;
            }
        });
    }
    pub async fn close(&self) {
        self.shared.permissions.disconnected(None);
        let task = self.task.lock().unwrap().take();
        if let Some(task) = task {
            task.abort();
            let _ = task.await;
        }
        let connection = {
            let mut state = self.shared.state.lock().unwrap();
            let connection = state.connection.take();
            *state = State::default();
            state.snapshot.generation = self.shared.generation.load(Ordering::Acquire);
            connection
        };
        if let Some(connection) = connection {
            connection.close().await;
        }
        self.shared.update(|_| {});
    }
}
impl Drop for Transport {
    fn drop(&mut self) {
        self.shared.permissions.disconnected(None);
        if let Some(task) = self.task.get_mut().unwrap().take() {
            task.abort();
        }
        if let Some(connection) = self.shared.state.lock().unwrap().connection.take() {
            connection.abort_transport();
        }
    }
}
pub(crate) fn backoff(attempt: u32, random: u64) -> Duration {
    let base = 2_000u64.saturating_mul(1u64 << attempt.saturating_sub(1).min(4));
    // 80–120% jitter, bounded even at the maximum backoff.
    Duration::from_millis((base * (800 + random % 401) / 1000).min(30_000))
}
async fn run(
    shared: Arc<Shared>,
    dial: Dial,
    mut retired: watch::Receiver<bool>,
    mut recovery: crate::recovery::Recovery,
) {
    let mut demand = shared.demand.subscribe();
    let mut retry = shared.retry.subscribe();
    let mut failures = 0u32;
    let mut connected_at: Option<Instant> = None;
    loop {
        if *retired.borrow() {
            return;
        }
        if retry.has_changed().unwrap_or(false) {
            retry.borrow_and_update();
            failures = 0;
            shared.update(|s| {
                s.snapshot.error = None;
                s.retry_at = None;
            });
        }
        let connection = shared.state.lock().unwrap().connection.clone();
        if let Some(connection) = connection {
            if !connection.is_closed() {
                tokio::select! {
                    biased;
                    _ = retired.changed() => return,
                    _ = connection.disconnected() => {},
                    _ = retry.changed() => {},
                    _ = recovery.changed() => {
                        let responsive = tokio::select! {
                            biased;
                            _ = retired.changed() => return,
                            responsive = recovery.revalidate(connection.responsive()) => responsive,
                        };
                        if !responsive {
                            // A socket can remain open across sleep and a network
                            // change. Retire it before another caller can reuse it.
                            connection.abort_transport();
                            shared.permissions.disconnected(None);
                            shared.update(|s| {
                                s.connection = None;
                                s.snapshot.status = "recovering";
                                s.snapshot.error = None;
                                s.retry_at = None;
                            });
                            failures = 0;
                            connected_at = None;
                            continue;
                        }
                    },
                }
                if !connection.is_closed() {
                    continue;
                }
            }
            shared.permissions.disconnected(None);
            if connected_at
                .take()
                .is_some_and(|at| at.elapsed() >= Duration::from_secs(60))
            {
                failures = 0;
            }
            failures = failures.saturating_add(1);
            let delay = backoff(failures, uuid::Uuid::new_v4().as_u128() as u64);
            shared.update(|s| {
                s.connection = None;
                s.snapshot.status = "recovering";
                s.snapshot.error = Some("SSH transport interrupted: connection closed".into());
                s.snapshot.retryable = true;
                s.retry_at = Some(Instant::now() + delay);
            });
        }
        if *demand.borrow_and_update() == 0 {
            shared.update(|s| s.snapshot.status = "disconnected");
            tokio::select! {
                biased;
                _ = retired.changed() => return,
                _ = retry.changed() => { failures = 0; },
                _ = demand.changed() => {},
            }
            continue;
        }
        let (permanent, retry_at) = {
            let state = shared.state.lock().unwrap();
            (
                state.snapshot.error.is_some() && !state.snapshot.retryable,
                state.retry_at,
            )
        };
        if permanent {
            tokio::select! {
                biased;
                _ = retired.changed() => return,
                _ = retry.changed() => {
                    failures = 0;
                    shared.update(|s| { s.snapshot.error = None; s.retry_at = None; });
                },
                _ = demand.changed() => {},
            }
            continue;
        }
        if let Some(at) = retry_at
            && at > Instant::now()
        {
            tokio::select! {
                biased;
                _ = retired.changed() => return,
                _ = retry.changed() => { failures = 0; shared.update(|s| s.retry_at = None); },
                _ = demand.changed() => {},
                _ = recovery.wait(at.saturating_duration_since(Instant::now())) => {
                    shared.update(|s| s.retry_at = None);
                },
            }
            continue;
        }
        tokio::select! {
            biased;
            _ = retired.changed() => return,
            _ = recovery.ready() => {},
        }
        if *demand.borrow() == 0 {
            continue;
        }
        shared.update(|s| {
            s.snapshot.status = if shared.generation.load(Ordering::Acquire) == 0 {
                "connecting"
            } else {
                "recovering"
            };
            s.snapshot.attempt = failures.saturating_add(1);
            s.snapshot.error = None;
            s.retry_at = None;
        });
        // This dial satisfies retry requests received before it started.
        // Routine retries preserve pending approval. Explicit checkout Retry
        // replaces it so a suppressed background request can prompt again.
        let fresh = retry.borrow_and_update().fresh;
        let mut restart = retry.clone();
        let sleep_epoch = recovery.sleep_epoch();
        let attempt = dial();
        tokio::pin!(attempt);
        let result = loop {
            tokio::select! {
                biased;
                _ = retired.changed() => return,
                _ = restart.changed() => {
                    if restart.borrow_and_update().fresh != fresh {
                        break None;
                    }
                },
                _ = demand.changed() => { if *demand.borrow() == 0 { break None; } },
                _ = recovery.changed() => {
                    // Approval may legitimately take a long time, but an attempt
                    // started before sleep must not hold recovery hostage after wake.
                    if recovery.sleeping() || recovery.sleep_epoch() != sleep_epoch {
                        break None;
                    }
                },
                result = &mut attempt => break Some(result),
            }
        };
        match result {
            None => {
                shared.update(|s| s.snapshot.status = "disconnected");
            }
            Some(Ok(connection)) => {
                connected_at = Some(Instant::now());
                let generation = shared.generation.fetch_add(1, Ordering::AcqRel) + 1;
                shared.update(|s| {
                    s.connection = Some(Arc::new(connection));
                    s.snapshot = Snapshot {
                        status: "connected",
                        generation,
                        ..Snapshot::default()
                    };
                    s.retry_at = None;
                });
                shared.connected.notify_one();
            }
            Some(Err(_)) if retry.has_changed().unwrap_or(false) => {
                // Do not publish a superseded failure: acquire/ready waiters
                // could otherwise terminate before the next loop clears it.
                // The queued retry resets failures and starts the next dial.
                continue;
            }
            Some(Err(error)) => {
                failures = failures.saturating_add(1);
                let random = uuid::Uuid::new_v4().as_u128() as u64;
                shared.update(|s| {
                    s.snapshot.status = if error.retryable {
                        "recovering"
                    } else {
                        "failed"
                    };
                    s.snapshot.error = Some(error.message);
                    s.snapshot.retryable = error.retryable;
                    s.retry_at = error
                        .retryable
                        .then(|| Instant::now() + backoff(failures, random));
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn fixture(dial: Dial) -> (Arc<Transport>, watch::Sender<bool>) {
        let server = serde_json::from_value(serde_json::json!({
            "id": uuid::Uuid::new_v4(), "name": "Supervisor test", "sshHost": "127.0.0.1", "sshUser": "fixture", "sshPort": 9
        })).unwrap();
        let (stop, retired) = watch::channel(false);
        let mut transport = Transport::new(
            server,
            retired,
            Arc::default(),
            Arc::default(),
            Arc::new(crate::integration_permissions::Gate::new(false, false)),
            None,
        );
        transport.dial = dial;
        (Arc::new(transport), stop)
    }
    async fn until(condition: impl Fn() -> bool) {
        tokio::time::timeout(Duration::from_secs(1), async {
            while !condition() {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
    }
    #[tokio::test(start_paused = true)]
    async fn sleep_cancels_old_approval_and_wake_retries_without_manual_test() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let (mut transport, _stop) = fixture({
            let attempts = attempts.clone();
            Arc::new(move || {
                attempts.fetch_add(1, Ordering::SeqCst);
                Box::pin(std::future::pending())
            })
        });
        let (recovery, report) = crate::recovery::fixture();
        Arc::get_mut(&mut transport).unwrap().recovery = recovery;
        let _persistent = transport.demand();
        until(|| attempts.load(Ordering::SeqCst) == 1).await;
        report(crate::recovery::Event::NetworkChanged);
        tokio::task::yield_now().await;
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            1,
            "network hints alone must preserve pending approval"
        );
        report(crate::recovery::Event::Sleep);
        until(|| transport.snapshot().status == "disconnected").await;
        tokio::time::advance(Duration::from_secs(3600)).await;
        tokio::task::yield_now().await;
        assert_eq!(attempts.load(Ordering::SeqCst), 1, "no dial while asleep");
        report(crate::recovery::Event::Wake);
        until(|| attempts.load(Ordering::SeqCst) == 2).await;
        assert_eq!(transport.snapshot().status, "connecting");
        transport.close().await;
    }

    #[tokio::test]
    async fn coalesced_sleep_and_wake_still_replace_pre_sleep_attempt() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let (mut transport, _stop) = fixture({
            let attempts = attempts.clone();
            Arc::new(move || {
                attempts.fetch_add(1, Ordering::SeqCst);
                Box::pin(std::future::pending())
            })
        });
        let (recovery, report) = crate::recovery::fixture();
        Arc::get_mut(&mut transport).unwrap().recovery = recovery;
        let _persistent = transport.demand();
        until(|| attempts.load(Ordering::SeqCst) == 1).await;
        report(crate::recovery::Event::Sleep);
        report(crate::recovery::Event::Wake);
        until(|| attempts.load(Ordering::SeqCst) == 2).await;
        transport.close().await;
    }

    #[tokio::test]
    async fn cancelling_one_caller_keeps_shared_startup_for_other_waiters() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let release = Arc::new(Notify::new());
        let (transport, _stop) = fixture({
            let attempts = attempts.clone();
            let release = release.clone();
            Arc::new(move || {
                attempts.fetch_add(1, Ordering::SeqCst);
                let release = release.clone();
                Box::pin(async move {
                    release.notified().await;
                    Err(ConnectFailure {
                        message: "denied".into(),
                        retryable: false,
                    })
                })
            })
        });
        assert!(transport.observe().is_none());
        assert_eq!(attempts.load(Ordering::SeqCst), 0);
        let first = {
            let transport = transport.clone();
            tokio::spawn(async move { transport.acquire().await })
        };
        let second = {
            let transport = transport.clone();
            tokio::spawn(async move { transport.acquire().await })
        };
        until(|| transport.snapshot().demand == 2 && attempts.load(Ordering::SeqCst) == 1).await;
        first.abort();
        let _ = first.await;
        assert_eq!(transport.snapshot().demand, 1);
        release.notify_one();
        assert_eq!(second.await.unwrap().err().unwrap(), "denied");
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        transport.close().await;
    }
    #[tokio::test(start_paused = true)]
    async fn transient_backoff_is_shared_and_stops_without_demand() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let (transport, _stop) = fixture({
            let attempts = attempts.clone();
            Arc::new(move || {
                attempts.fetch_add(1, Ordering::SeqCst);
                Box::pin(async {
                    Err(ConnectFailure {
                        message: "offline".into(),
                        retryable: true,
                    })
                })
            })
        });
        assert_eq!(transport.acquire().await.err().unwrap(), "offline");
        for _ in 0..12 {
            assert!(transport.acquire().await.is_err());
        }
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            1,
            "callers must share one failed attempt and deadline"
        );
        tokio::time::advance(Duration::from_secs(60)).await;
        tokio::task::yield_now().await;
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            1,
            "no background dial without demand"
        );
        assert!(transport.acquire().await.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        let persistent = transport.demand();
        tokio::time::advance(Duration::from_secs(31)).await;
        until(|| attempts.load(Ordering::SeqCst) >= 3).await;
        drop(persistent);
        transport.close().await;
        for attempt in 1..100 {
            assert!(backoff(attempt, 400) <= Duration::from_secs(30));
        }
        assert!(backoff(1, 0) >= Duration::from_millis(1600));
    }
    #[tokio::test(start_paused = true)]
    async fn permanent_failures_wait_for_explicit_retry() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let (transport, _stop) = fixture({
            let attempts = attempts.clone();
            Arc::new(move || {
                attempts.fetch_add(1, Ordering::SeqCst);
                Box::pin(async {
                    Err(ConnectFailure {
                        message: "host key rejected".into(),
                        retryable: false,
                    })
                })
            })
        });
        let _persistent = transport.demand();
        assert!(transport.acquire().await.is_err());
        tokio::time::advance(Duration::from_secs(3600)).await;
        assert!(transport.acquire().await.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        transport.retry();
        assert!(transport.acquire().await.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        transport.close().await;
    }
    #[tokio::test(start_paused = true)]
    async fn persistent_service_observes_failure_and_retry_without_redialing_itself() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let (transport, stop) = fixture({
            let attempts = attempts.clone();
            Arc::new(move || {
                let attempt = attempts.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move {
                    if attempt > 0 {
                        return std::future::pending().await;
                    }
                    Err(ConnectFailure {
                        message: "SSH approval required".into(),
                        retryable: false,
                    })
                })
            })
        });
        let observed = Arc::new(Mutex::new(None));
        let service = {
            let transport = transport.clone();
            let observed = observed.clone();
            tokio::spawn(async move {
                transport
                    .follow_ready(|error| *observed.lock().unwrap() = error)
                    .await
            })
        };
        until(|| observed.lock().unwrap().is_some()).await;
        assert_eq!(
            transport.acquire().await.err().unwrap(),
            "SSH approval required"
        );
        tokio::time::advance(Duration::from_secs(3600)).await;
        tokio::task::yield_now().await;
        assert!(
            !service.is_finished(),
            "enabled service must retain its subscription"
        );
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            1,
            "no automatic consent retry"
        );

        transport.retry_fresh();
        until(|| attempts.load(Ordering::SeqCst) == 2 && observed.lock().unwrap().is_none()).await;
        assert!(!service.is_finished());
        stop.send_replace(true);
        assert_eq!(service.await.unwrap().unwrap_err(), RETIRED);
        assert_eq!(transport.snapshot().demand, 0);
        transport.close().await;
    }

    #[tokio::test(start_paused = true)]
    async fn explicit_retry_bypasses_shared_backoff_immediately() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let (transport, _stop) = fixture({
            let attempts = attempts.clone();
            Arc::new(move || {
                attempts.fetch_add(1, Ordering::SeqCst);
                Box::pin(async {
                    Err(ConnectFailure {
                        message: "agent temporarily unavailable".into(),
                        retryable: true,
                    })
                })
            })
        });
        let _persistent = transport.demand();
        assert!(transport.acquire().await.is_err());
        let deadline = transport.shared.state.lock().unwrap().retry_at.unwrap();
        let now = Instant::now();
        assert!(deadline > now);
        transport.retry();
        assert!(transport.acquire().await.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert_eq!(
            Instant::now(),
            now,
            "explicit retry must not wait for backoff"
        );
        assert_eq!(transport.snapshot().attempt, 1);
        transport.close().await;
    }

    #[tokio::test(start_paused = true)]
    async fn another_connection_reaching_the_server_ends_the_backoff() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let (transport, _stop) = fixture({
            let attempts = attempts.clone();
            Arc::new(move || {
                attempts.fetch_add(1, Ordering::SeqCst);
                Box::pin(async {
                    Err(ConnectFailure {
                        message: "No route to host".into(),
                        retryable: true,
                    })
                })
            })
        });
        let version = || transport.shared.retry.borrow().version;
        let before = version();
        transport.reachable();
        assert_eq!(version(), before, "nothing to retry before a failure");
        let _persistent = transport.demand();
        assert!(transport.acquire().await.is_err());
        assert!(transport.shared.state.lock().unwrap().retry_at.is_some());
        let now = Instant::now();
        transport.reachable();
        assert!(transport.acquire().await.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert_eq!(Instant::now(), now, "the backoff ended at once");
        transport.close().await;
    }

    #[tokio::test]
    async fn explicit_fresh_retry_withdraws_background_approval_before_new_dial() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let cancelled = Arc::new(AtomicUsize::new(0));
        struct PendingApproval(Arc<AtomicUsize>);
        impl Drop for PendingApproval {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let (transport, _stop) = fixture({
            let attempts = attempts.clone();
            let cancelled = cancelled.clone();
            Arc::new(move || {
                let attempt = attempts.fetch_add(1, Ordering::SeqCst);
                if attempt > 0 {
                    assert_eq!(cancelled.load(Ordering::SeqCst), attempt);
                }
                let approval = PendingApproval(cancelled.clone());
                Box::pin(async move {
                    let _approval = approval;
                    std::future::pending().await
                })
            })
        });
        let _persistent = transport.demand();
        until(|| attempts.load(Ordering::SeqCst) == 1).await;
        transport.retry_fresh();
        transport.retry_fresh();
        until(|| attempts.load(Ordering::SeqCst) == 2).await;
        assert_eq!(cancelled.load(Ordering::SeqCst), 1);
        assert!(transport.snapshot().error.is_none());
        assert_eq!(transport.snapshot().attempt, 1);
        transport.close().await;
        assert_eq!(cancelled.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn retry_preserves_pending_approval_and_suppresses_its_superseded_failure() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let release = Arc::new(Notify::new());
        let (transport, _stop) = fixture({
            let attempts = attempts.clone();
            let release = release.clone();
            Arc::new(move || {
                let attempt = attempts.fetch_add(1, Ordering::SeqCst);
                let release = release.clone();
                Box::pin(async move {
                    if attempt == 0 {
                        release.notified().await;
                    }
                    Err(ConnectFailure {
                        message: if attempt == 0 {
                            "old failure"
                        } else {
                            "new failure"
                        }
                        .into(),
                        retryable: false,
                    })
                })
            })
        });
        let _persistent = transport.demand();
        let waiter = {
            let transport = transport.clone();
            tokio::spawn(async move { transport.acquire().await })
        };
        until(|| attempts.load(Ordering::SeqCst) == 1).await;
        let before_retry = *transport.shared.changes.borrow();
        // Multiple callers can request retry while a shared approval is open.
        // Keep that prompt alive and coalesce their requests into one retry.
        transport.retry();
        transport.retry();
        tokio::task::yield_now().await;
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert!(!waiter.is_finished());
        release.notify_one();
        assert_eq!(waiter.await.unwrap().err().unwrap(), "new failure");
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert_eq!(transport.snapshot().attempt, 1);
        // Only retry resets, the new attempt and its failure are published;
        // the obsolete failure must never become observable to waiters.
        let publications = transport.shared.changes.borrow().wrapping_sub(before_retry);
        assert_eq!(publications, 5, "obsolete failures must not be published");
        transport.close().await;
    }
    #[tokio::test]
    async fn retirement_and_zero_demand_cancel_pending_authentication() {
        struct Cancelled(Arc<AtomicUsize>);
        impl Drop for Cancelled {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let cancelled = Arc::new(AtomicUsize::new(0));
        let (transport, stop) = fixture({
            let cancelled = cancelled.clone();
            Arc::new(move || {
                let guard = Cancelled(cancelled.clone());
                Box::pin(async move {
                    let _guard = guard;
                    std::future::pending().await
                })
            })
        });
        let waiter = {
            let t = transport.clone();
            tokio::spawn(async move { t.acquire().await })
        };
        until(|| transport.snapshot().status == "connecting").await;
        waiter.abort();
        let _ = waiter.await;
        until(|| cancelled.load(Ordering::SeqCst) == 1).await;
        let waiter = {
            let t = transport.clone();
            tokio::spawn(async move { t.acquire().await })
        };
        until(|| transport.snapshot().status == "connecting").await;
        stop.send_replace(true);
        assert_eq!(waiter.await.unwrap().err().unwrap(), RETIRED);
        until(|| cancelled.load(Ordering::SeqCst) == 2).await;
        transport.close().await;
    }
}
