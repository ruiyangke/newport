//! Application-owned asynchronous query state. A typed Query is one cache key;
//! its owner supplies scope (for example server incarnation + feature + path).
//! Observers renew short demand leases. Navigation never owns the fetch task.
mod registry;
pub use registry::Registry;

use serde::Serialize;
use std::{
    future::Future,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{sync::watch, task::JoinHandle, time::Instant};

#[derive(Clone, Copy)]
pub struct Policy {
    pub stale_after: Duration,
    pub retry_after: Duration,
    pub timeout: Duration,
    /// Observers renew this lease while visible; background polling stops when
    /// all observers disappear, including crashed/disconnected clients.
    pub observation_lease: Duration,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot<T, E = String> {
    pub data: Option<T>,
    pub error: Option<E>,
    pub fetching: bool,
    pub stale: bool,
    pub revision: u64,
    pub updated_at: Option<u64>,
    #[serde(skip)]
    fresh_at: Option<Instant>,
    #[serde(skip)]
    epoch: u64,
    #[serde(skip)]
    settled_force: u64,
}

impl<T, E> Default for Snapshot<T, E> {
    fn default() -> Self {
        Self {
            data: None,
            error: None,
            fetching: false,
            stale: true,
            revision: 0,
            updated_at: None,
            fresh_at: None,
            epoch: 0,
            settled_force: 0,
        }
    }
}

#[derive(Clone)]
struct Control {
    active_until: Instant,
    force: u64,
    epoch: u64,
    closed: bool,
}

/// One coordinator and at most one bounded fetch per key. Snapshots and demand
/// updates never acquire a mutex held by network or disk work.
pub struct Query<T, E = String> {
    policy: Policy,
    control: watch::Sender<Control>,
    snapshots: watch::Receiver<Snapshot<T, E>>,
    task: JoinHandle<()>,
}

impl<T: Clone + Send + Sync + 'static> Query<T> {
    pub fn new<F, Fut>(policy: Policy, loader: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<T, String>> + Send + 'static,
    {
        Self::new_with_error(policy, loader)
    }
}

impl<T: Clone + Send + Sync + 'static, E: Clone + From<String> + Send + Sync + 'static>
    Query<T, E>
{
    pub fn new_with_error<F, Fut>(policy: Policy, loader: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<T, E>> + Send + 'static,
    {
        assert!(
            !policy.stale_after.is_zero()
                && !policy.retry_after.is_zero()
                && !policy.timeout.is_zero()
                && !policy.observation_lease.is_zero()
        );
        let (control, requests) = watch::channel(Control {
            active_until: Instant::now(),
            force: 0,
            epoch: 0,
            closed: false,
        });
        let (publish, snapshots) = watch::channel(Snapshot::default());
        let task = tokio::spawn(run(policy, loader, requests, publish));
        Self {
            policy,
            control,
            snapshots,
            task,
        }
    }

    /// Immediately return the last observation and renew demand. A forced
    /// refresh joins a running fetch; it does not queue another identical fetch.
    pub fn observe(&self, force: bool) -> Snapshot<T, E> {
        self.control.send_modify(|control| {
            if !control.closed {
                control.active_until = Instant::now() + self.policy.observation_lease;
                if force {
                    control.force += 1;
                }
            }
        });
        self.snapshot()
    }

    /// Subscribe to changes without creating demand. Call observe periodically
    /// while the consumer is active. Watching alone never keeps SSH alive.
    pub fn subscribe(&self) -> watch::Receiver<Snapshot<T, E>> {
        self.snapshots.clone()
    }

    pub fn snapshot(&self) -> Snapshot<T, E> {
        let control = self.control.borrow().clone();
        if control.closed {
            return Snapshot {
                error: Some("Query scope closed.".to_owned().into()),
                ..Snapshot::default()
            };
        }
        let mut snapshot = self.snapshots.borrow().clone();
        snapshot.fetching |= control.force > snapshot.settled_force
            || snapshot.data.is_none()
                && snapshot.error.is_none()
                && control.active_until > Instant::now();
        snapshot.stale = snapshot.epoch != control.epoch
            || snapshot.error.is_some()
            || snapshot
                .fresh_at
                .is_none_or(|at| at.elapsed() >= self.policy.stale_after);
        snapshot
    }

    /// Read a fresh value, sharing any running fetch. Unlike refresh, a cache
    /// hit does not trigger I/O. Failed reads retain their error until retry is
    /// due; callers can keep their last displayed data without hiding failures.
    pub async fn read(&self) -> Result<T, E> {
        let mut changes = self.subscribe();
        self.observe(false);
        loop {
            let snapshot = self.snapshot();
            let control = self.control.borrow().clone();
            if control.closed {
                return Err("Query scope closed.".to_owned().into());
            }
            if snapshot.epoch == control.epoch && !snapshot.fetching {
                if let Some(error) = snapshot.error {
                    return Err(error);
                }
                if !snapshot.stale
                    && let Some(data) = snapshot.data
                {
                    return Ok(data);
                }
            }
            tokio::select! {
                result = changes.changed() => result.map_err(|_| "Query worker stopped.".to_owned())?,
                _ = tokio::time::sleep(self.policy.observation_lease / 2) => { self.observe(false); }
            }
        }
    }

    fn active(&self) -> bool {
        self.control.borrow().active_until > Instant::now()
    }

    /// Explicit refresh for callers that need a fresh result. Cancelling this
    /// waiter does not cancel the shared request or discard its cached result.
    pub async fn refresh(&self) -> Result<T, E> {
        let mut changes = self.subscribe();
        self.observe(true);
        let ticket = self.control.borrow().force;
        loop {
            let snapshot = self.snapshot();
            if self.control.borrow().closed {
                return Err("Query scope closed.".to_owned().into());
            }
            if snapshot.settled_force >= ticket {
                return match snapshot.error {
                    Some(error) => Err(error),
                    None => snapshot
                        .data
                        .ok_or_else(|| "Query returned no data.".to_owned().into()),
                };
            }
            changes
                .changed()
                .await
                .map_err(|_| "Query worker stopped.".to_owned())?;
        }
    }

    /// Mutations invalidate observations, never authoritative live resources.
    /// Retain old data as stale; cancel and fence any pre-invalidation request.
    pub fn invalidate(&self) {
        self.control.send_modify(|control| control.epoch += 1);
    }

    /// Retiring a server scope immediately hides its cache and cancels work.
    pub fn close(&self) {
        self.control.send_modify(|control| control.closed = true);
    }
}

impl<T, E> Drop for Query<T, E> {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct Fetch<T, E>(JoinHandle<Result<T, E>>);
impl<T, E> Drop for Fetch<T, E> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn run<T, E, F, Fut>(
    policy: Policy,
    loader: F,
    mut requests: watch::Receiver<Control>,
    publish: watch::Sender<Snapshot<T, E>>,
) where
    T: Clone + Send + Sync + 'static,
    E: Clone + From<String> + Send + Sync + 'static,
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<T, E>> + Send + 'static,
{
    let loader = std::sync::Arc::new(loader);
    let mut state = Snapshot::default();
    let mut attempted = None::<Instant>;
    'coordinator: loop {
        let control = requests.borrow_and_update().clone();
        if control.closed {
            publish.send_replace(Snapshot {
                error: Some("Query scope closed.".to_owned().into()),
                ..Snapshot::default()
            });
            return;
        }
        let invalidated = state.epoch != control.epoch;
        if invalidated {
            state.epoch = control.epoch;
            state.fetching = false;
            state.stale = true;
            state.fresh_at = None;
            state.error = None;
            attempted = None;
            publish.send_replace(state.clone());
        }
        let delay = if state.error.is_some() {
            policy.retry_after
        } else {
            policy.stale_after
        };
        let next = attempted.map(|at| at + delay).unwrap_or_else(Instant::now);
        let active = control.active_until > Instant::now();
        if active && (invalidated || control.force > state.settled_force || next <= Instant::now())
        {
            state.fetching = true;
            state.stale = invalidated
                || state.error.is_some()
                || state
                    .fresh_at
                    .is_none_or(|at| at.elapsed() >= policy.stale_after);
            publish.send_replace(state.clone());
            let loader = loader.clone();
            let mut fetch = Fetch(tokio::spawn(async move { loader().await }));
            let deadline = tokio::time::sleep(policy.timeout);
            tokio::pin!(deadline);
            let result = loop {
                tokio::select! {
                    biased;
                    changed = requests.changed() => {
                        if changed.is_err() { return; }
                        let latest = requests.borrow().clone();
                        if latest.closed || latest.epoch != control.epoch {
                            state.fetching = false;
                            continue 'coordinator;
                        }
                    }
                    result = &mut fetch.0 => break result.unwrap_or_else(|error| Err(format!("Query worker failed: {error}").into())),
                    _ = &mut deadline => break Err("Query refresh timed out.".to_owned().into()),
                }
            };
            // Validate once more after the await; an invalidation may arrive
            // together with completion. Old results must never become fresh.
            let latest = requests.borrow().clone();
            if latest.closed || latest.epoch != control.epoch {
                continue;
            }
            attempted = Some(Instant::now());
            state.epoch = control.epoch;
            state.settled_force = latest.force;
            state.fetching = false;
            match result {
                Ok(data) => {
                    state.data = Some(data);
                    state.error = None;
                    state.stale = false;
                    state.fresh_at = attempted;
                    state.updated_at = Some(
                        SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as u64,
                    );
                    state.revision += 1;
                }
                Err(error) => {
                    state.error = Some(error);
                    state.stale = true;
                }
            }
            publish.send_replace(state.clone());
        } else if active {
            tokio::select! {
                changed = requests.changed() => if changed.is_err() { return; },
                _ = tokio::time::sleep_until(next.min(control.active_until)) => {},
            }
        } else if requests.changed().await.is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests;
