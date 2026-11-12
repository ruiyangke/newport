//! Server-session discovery queries with source-aware stale retention.
pub mod cache;
mod telemetry;
use crate::{monitor, server_session::ServerSession};
use newport_protocol::{
    desktop::connection::{PortsObservation, discovery::Discovery},
    monitor::ports::discovery::{Report, Request, Snapshot},
};
use std::{
    future::Future,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

const REPLY_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub enum Error {
    SessionClosed,
    UnexpectedReply,
    Monitor(monitor::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SessionClosed => f.write_str("Server session closed."),
            Self::UnexpectedReply => f.write_str("Invalid port discovery response operation."),
            Self::Monitor(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Monitor(error) => Some(error),
            Self::SessionClosed | Self::UnexpectedReply => None,
        }
    }
}

impl From<monitor::Error> for Error {
    fn from(error: monitor::Error) -> Self {
        Self::Monitor(error)
    }
}

#[derive(Clone, Debug)]
pub enum Failure {
    Query(Arc<Error>),
    Coordinator(String),
    Discovery(Arc<DiscoveryFailure>),
}
impl From<String> for Failure {
    fn from(value: String) -> Self {
        Self::Coordinator(value)
    }
}
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Query(error) => error.fmt(f),
            Self::Coordinator(message) => f.write_str(message),
            Self::Discovery(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for Failure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Discovery(error) => Some(error.as_ref()),
            Self::Query(error) => Some(error.as_ref()),
            Self::Coordinator(_) => None,
        }
    }
}

#[derive(Debug)]
pub struct DiscoveryFailure {
    error: cache::Error,
    scope_epoch: Arc<ScopeEpoch>,
}

impl std::fmt::Display for DiscoveryFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(f)
    }
}

impl std::error::Error for DiscoveryFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

impl DiscoveryFailure {
    fn observation(&self, epoch: &Arc<ScopeEpoch>) -> Option<Discovery> {
        if !Arc::ptr_eq(epoch, &self.scope_epoch) {
            return None;
        }
        match &self.error {
            cache::Error::NoInventory(snapshot) => Some(Discovery {
                current: snapshot.as_ref().clone(),
                retained: Vec::new(),
                retention: Report::complete(),
            }),
            cache::Error::Invalid(_) => None,
        }
    }
}

pub struct Query {
    inner: crate::query::Query<Published, Failure>,
    manual: Arc<AtomicBool>,
    scope_epoch: Arc<Mutex<Arc<ScopeEpoch>>>,
}

#[derive(Debug)]
struct ScopeEpoch;

#[derive(Clone, Debug)]
pub(crate) struct Published {
    scope_epoch: Arc<ScopeEpoch>,
    discovery: Discovery,
}

impl Query {
    pub(crate) fn new<F, Fut>(policy: crate::query::Policy, server: uuid::Uuid, loader: F) -> Self
    where
        F: Fn(bool) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Snapshot, Error>> + Send + 'static,
    {
        let manual = Arc::new(AtomicBool::new(false));
        let scope_epoch = Arc::new(Mutex::new(Arc::new(ScopeEpoch)));
        let cache = Arc::new(tokio::sync::Mutex::new(cache::Cache::default()));
        let telemetry = Arc::new(Mutex::new(telemetry::Telemetry::default()));
        let requested = manual.clone();
        let current = scope_epoch.clone();
        let inner = crate::query::Query::new_with_error(policy, move || {
            let refresh = requested.swap(false, Ordering::AcqRel);
            let result = loader(refresh);
            let cache = cache.clone();
            let current = current.clone();
            let telemetry = telemetry.clone();
            async move {
                let result = result.await;
                let mut cache = cache.lock().await;
                let mut telemetry = telemetry.lock().unwrap_or_else(|error| error.into_inner());
                let snapshot = match result {
                    Ok(snapshot) => snapshot,
                    Err(error) => {
                        telemetry.failed(server, telemetry::Failure::Query);
                        return Err(Failure::Query(Arc::new(error)));
                    }
                };
                let changed = cache.snapshot().is_none_or(|previous| {
                    !newport_protocol::desktop::connection::discovery::same_scope(
                        &previous.current,
                        &snapshot,
                    ) || previous.current.generation >= snapshot.generation
                });
                match cache.apply(snapshot) {
                    Ok(value) => {
                        telemetry.scan(server, &value.current, refresh);
                        telemetry.succeeded(server);
                        let discovery = value.clone();
                        let mut epoch = current.lock().unwrap_or_else(|error| error.into_inner());
                        if changed {
                            *epoch = Arc::new(ScopeEpoch);
                        }
                        Ok(Published {
                            scope_epoch: epoch.clone(),
                            discovery,
                        })
                    }
                    Err(error) => {
                        match &error {
                            cache::Error::NoInventory(snapshot) => {
                                telemetry.scan(server, snapshot, refresh);
                                telemetry.failed(server, telemetry::Failure::NoInventory);
                            }
                            cache::Error::Invalid(_) => {
                                telemetry.failed(server, telemetry::Failure::InvalidSnapshot)
                            }
                        }
                        let mut epoch = current.lock().unwrap_or_else(|error| error.into_inner());
                        if matches!(error, cache::Error::NoInventory(_)) {
                            *epoch = Arc::new(ScopeEpoch);
                        }
                        Err(Failure::Discovery(Arc::new(DiscoveryFailure {
                            error,
                            scope_epoch: epoch.clone(),
                        })))
                    }
                }
            }
        });
        Self {
            inner,
            manual,
            scope_epoch,
        }
    }

    pub fn observe(&self, force: bool) -> PortsObservation {
        if force {
            self.invalidate();
        }
        self.project(self.inner.observe(force))
    }
    pub fn snapshot(&self) -> PortsObservation {
        self.project(self.inner.snapshot())
    }
    pub fn invalidate(&self) {
        self.manual.store(true, Ordering::Release);
        self.inner.invalidate();
    }
    pub fn close(&self) {
        self.inner.close();
        *self
            .scope_epoch
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Arc::new(ScopeEpoch);
    }
    pub async fn refresh(&self) -> Result<Discovery, Failure> {
        self.invalidate();
        self.inner.refresh().await.map(|value| value.discovery)
    }
    #[cfg(test)]
    pub(crate) fn subscribe(
        &self,
    ) -> tokio::sync::watch::Receiver<crate::query::Snapshot<Published, Failure>> {
        self.inner.subscribe()
    }

    fn project(&self, value: crate::query::Snapshot<Published, Failure>) -> PortsObservation {
        // A scope failure invalidates old query data permanently. A later good
        // fetch cannot re-enable an old snapshot copied before publication.
        let epoch = self
            .scope_epoch
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let data = value
            .data
            .filter(|value| Arc::ptr_eq(&epoch, &value.scope_epoch))
            .map(|value| value.discovery);
        // Failed scans carry diagnostics, never the time of a successful reading.
        let updated_at = data.as_ref().and(value.updated_at);
        let data = data.or_else(|| match value.error.as_ref() {
            Some(Failure::Discovery(error)) => error.observation(&epoch),
            _ => None,
        });
        drop(epoch);
        let mut error = value.error.map(|error| error.to_string());
        let unavailable = data
            .as_ref()
            .is_some_and(|data| !data.has_current_inventory());
        if unavailable && error.is_none() {
            error =
                Some("Port inventories could not be refreshed. Showing last observed data.".into());
        }
        PortsObservation {
            data,
            error,
            fetching: value.fetching,
            stale: value.stale || unavailable,
            revision: value.revision,
            updated_at,
        }
    }
}

pub(crate) fn query(owner: &Arc<ServerSession>) -> &Query {
    let query = owner.ports.get_or_init(|| {
        let weak = Arc::downgrade(owner);
        Query::new(
            crate::query::Policy {
                stale_after: Duration::from_secs(30),
                retry_after: Duration::from_secs(10),
                timeout: Duration::from_secs(35),
                observation_lease: Duration::from_secs(5),
            },
            owner.server().id,
            move |refresh| {
                let weak = weak.clone();
                async move {
                    let owner = weak.upgrade().ok_or(Error::SessionClosed)?;
                    let mut cancelled = owner.cancellation();
                    if *cancelled.borrow() {
                        return Err(Error::SessionClosed);
                    }
                    tokio::select! {
                        biased;
                        _ = cancelled.changed() => Err(Error::SessionClosed),
                        result = discover_with(&owner, refresh) => result,
                    }
                }
            },
        )
    });
    if owner.is_revoked() {
        query.close();
    }
    query
}

pub async fn discover(owner: &Arc<ServerSession>) -> Result<Snapshot, Error> {
    discover_with(owner, false).await
}
async fn discover_with(owner: &Arc<ServerSession>, refresh: bool) -> Result<Snapshot, Error> {
    let reply = monitor::ask(
        owner,
        &newport_protocol::monitor::Request::Ports(Request { refresh }),
        REPLY_TIMEOUT,
    )
    .await?;
    let newport_protocol::monitor::Reply::Ports(reply) = reply else {
        return Err(Error::UnexpectedReply);
    };
    Ok(*reply)
}

#[cfg(test)]
pub(crate) mod tests;
