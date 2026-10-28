//! Shared remote Git service. Writes use durable operation IDs.
//!
//! One shared server transport with independent metadata, history, diff and remote-advertisement channels. The agent is
//! stateless -- repository ids, snapshots, entry ids and cursors are
//! self-describing tokens it re-verifies on every use -- so nothing a request
//! depends on lives in a connection. A connection can therefore be shared by
//! every part of the app, closed when idle, and replaced after a failure; the
//! next request simply connects again. A request is never re-sent because its
//! connection failed: a failure mid-request is reported as it happened, and a
//! write whose outcome was not seen is reported as possibly completed.
pub mod client;
pub mod pending;
#[allow(dead_code)]
#[rustfmt::skip]
#[path = "../../../../tools/agent/src/git/protocol.rs"]
pub mod protocol;
mod queries;
use crate::{manager::Shared, server_session};
use client::Client;
use protocol::{Error, Request};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, RwLock, watch};
use uuid::Uuid;

/// Persistent identity is supplied by the profile's preference store.
/// It must be saved successfully before being returned to an agent.
pub trait ClientIdentity: Send + Sync {
    fn git_client_id(&self) -> IdentityFuture;
}

/// Owned asynchronous lookup; persistence must not block the request executor.
pub type IdentityFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send>>;

/// Receives a scoped command-log event; UI and transport delivery belong to the host.
pub type LogSink = Arc<dyn Fn(Value) + Send + Sync>;

type Stream = crate::agent_runtime::WorkerStream;
struct Session {
    server_id: Uuid,
    revision: u64,
    connection: server_session::Lease,
    clients: [Mutex<Option<Client<Stream>>>; 4],
    client_id: String,
    access: RwLock<()>,
    cancelled: watch::Sender<bool>,
    /// The agent's hello and capabilities, returned to every caller that asks
    /// to connect while this connection is the server's.
    info: Value,
    used: std::sync::Mutex<Instant>,
    queries: queries::Queries,
}
impl Session {
    fn touch(&self) {
        *self.used.lock().expect("usage clock") = Instant::now();
    }
    fn close(&self) {
        self.queries.close();
        self.cancelled.send_replace(true);
        self.connection.release_demand();
        for lane in &self.clients {
            if let Ok(mut client) = lane.try_lock() {
                *client = None;
            }
        }
    }
}
/// The owning ServerSession is the only live Git service store.
#[derive(Default)]
pub(crate) struct Slot {
    open: Mutex<Option<Arc<Session>>>,
    connecting: Mutex<()>,
    epoch: AtomicU64,
    maintenance: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}
impl Slot {
    /// Caller has closed durable admission. Wait for admitted requests to finish
    /// before releasing channels; cancellation of this wait leaves them running.
    #[cfg(test)]
    pub(crate) async fn drain(&self) {
        let _connecting = self.connecting.lock().await;
        let mut open = self.open.lock().await;
        if let Some(session) = open.as_ref().cloned() {
            let _access = session.access.write().await;
            self.epoch.fetch_add(1, Ordering::AcqRel);
            if let Some(task) = self.maintenance.lock().unwrap().take() {
                task.abort();
            }
            session.close();
            *open = None;
        }
    }
    pub(crate) async fn close(&self) {
        // Fence a startup which is still awaiting SSH or the worker handshake.
        // Publication checks this epoch while holding the same slot lock.
        let mut open = self.open.lock().await;
        self.epoch.fetch_add(1, Ordering::AcqRel);
        let session = open.take();
        if let Some(task) = self.maintenance.lock().unwrap().take() {
            task.abort();
        }
        drop(open);
        if let Some(session) = session {
            session.close();
        }
    }
}
impl Drop for Slot {
    fn drop(&mut self) {
        if let Some(task) = self.maintenance.get_mut().unwrap().take() {
            task.abort();
        }
    }
}
/// Outlasts Needle's limit on one Git command and its stop grace
/// (`COMMAND_LIMIT` and `STOP_GRACE` in tools/agent/src/git/cli/command.rs), so
/// a slow command ends with Needle's timeout error rather than the desktop's.
const DEADLINE: Duration = Duration::from_secs(65);
/// A connection unused this long is closed; the next request reconnects. Safe
/// only because the agent keeps no state a request depends on.
const IDLE: Duration = Duration::from_secs(600);
const KEEPALIVE: Duration = Duration::from_secs(15);

async fn ping_idle<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    lane: &Mutex<Option<Client<S>>>,
) {
    let Ok(mut guard) = lane.try_lock() else {
        return;
    };
    if let Some(client) = guard.as_mut() {
        let healthy = matches!(
            tokio::time::timeout(Duration::from_secs(5), client.ping()).await,
            Ok(Ok(()))
        );
        if !healthy {
            // This idle agent may have exited independently of the SSH
            // connection. Discard only its stream; closing the session could
            // interrupt a write on another lane. The next request initializes
            // a fresh agent before sending any operation.
            *guard = None;
        }
    }
}

async fn ping_lanes<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    lanes: &[Mutex<Option<Client<S>>>; 4],
) {
    tokio::join!(
        ping_idle(&lanes[0]),
        ping_idle(&lanes[1]),
        ping_idle(&lanes[2]),
        ping_idle(&lanes[3]),
    );
}

fn release_idle_lanes<T>(lanes: &[Mutex<Option<T>>], releases_capacity: impl Fn(&T) -> bool) {
    for lane in lanes {
        if let Ok(mut client) = lane.try_lock()
            && client.as_ref().is_some_and(&releases_capacity)
        {
            *client = None;
        }
    }
}

// Keep related paginated reads on the same agent so its bounded caches remain useful.
fn request_lane(request: &Request) -> usize {
    match request {
        Request::RemoteRefs { .. } => 3,
        Request::History { .. } => 1,
        Request::Tag { .. }
        | Request::Commit { .. }
        | Request::BlobPage { .. }
        | Request::Blob { .. }
        | Request::CommitFiles { .. }
        | Request::CommitDiffPage { .. }
        | Request::CommitDiff { .. }
        | Request::DiffPage { .. }
        | Request::Diff { .. } => 2,
        _ => 0,
    }
}

fn is_mutation(request: &Request) -> bool {
    matches!(
        request,
        Request::Start { .. }
            | Request::Init { .. }
            | Request::Clone { .. }
            | Request::Review { .. }
    )
}

enum Access<'a> {
    Read {
        _guard: tokio::sync::RwLockReadGuard<'a, ()>,
    },
    Write {
        _guard: tokio::sync::RwLockWriteGuard<'a, ()>,
    },
}

async fn request_access<'a>(access: &'a RwLock<()>, request: &Request) -> Access<'a> {
    if is_mutation(request) {
        Access::Write {
            _guard: access.write().await,
        }
    } else {
        Access::Read {
            _guard: access.read().await,
        }
    }
}

/// Whether a server's existing connection may carry the next request: it must
/// still be open, and opened for the server's current configuration.
fn reusable(cancelled: bool, revision: u64, current: u64) -> bool {
    !cancelled && revision == current
}
/// Whether an idle connection should be closed now.
fn idle_expired(last_used: Instant, now: Instant) -> bool {
    now.saturating_duration_since(last_used) >= IDLE
}

/// The server's connection, connecting if it has none or the one it had is
/// closed or was opened for an older configuration. Never called after a
/// request has been sent, so reconnecting can never re-send one.
async fn connection(
    preferences: &dyn ClientIdentity,
    state: &Shared,
    server_id: Uuid,
) -> Result<Arc<Session>, Error> {
    let (server, owner) = state.current(server_id).map_err(Error::transport)?;
    let revision = owner.revision();
    let slot = &owner.git;
    let live = |session: &Option<Arc<Session>>| {
        session
            .as_ref()
            .filter(|s| {
                reusable(*s.cancelled.borrow(), s.revision, revision) && !s.connection.is_closed()
            })
            .cloned()
    };
    if let Some(session) = live(&*slot.open.lock().await) {
        return Ok(session);
    }
    let _connecting = slot.connecting.lock().await;
    if let Some(session) = live(&*slot.open.lock().await) {
        return Ok(session);
    }
    let epoch = slot.epoch.load(Ordering::Acquire);
    let stale = slot.open.lock().await.take();
    if let Some(stale) = stale {
        stale.close();
    }
    let client_id = preferences
        .git_client_id()
        .await
        .map_err(Error::transport)?;
    // Identity persistence can outlive a cancelled connection attempt. Fence
    // its captured owner before starting any SSH work for that old attempt.
    if owner.is_revoked() || slot.epoch.load(Ordering::Acquire) != epoch {
        return Err(Error::new(
            "STALE_CONNECTION",
            "Server connection changed. Reconnect.",
        ));
    }
    let connection = owner.acquire(&server).await.map_err(Error::transport)?;
    let setup = tokio::time::timeout(DEADLINE, async {
        let (stream, _) = connection
            .launch(crate::agent::Service::Git)
            .await
            .map_err(Error::transport)?;
        tokio::time::timeout(
            Duration::from_secs(5),
            Client::start_with_identity(stream, client_id.clone()),
        )
        .await
        .map_err(|_| {
            Error::new(
                "AGENT_UNAVAILABLE",
                "Git agent did not initialize. Install or update the server agent.",
            )
        })?
    })
    .await
    .map_err(|_| Error::transport("Git connection timed out."))
    .and_then(|r| r);
    let (client, info) = match setup {
        Ok(v) => v,
        Err(e) => {
            return Err(e);
        }
    };
    if owner.is_revoked() {
        return Err(Error::new(
            "STALE_CONNECTION",
            "Server configuration changed. Reconnect.",
        ));
    }
    let (cancelled, _) = watch::channel(false);
    let session = Arc::new(Session {
        server_id,
        revision,
        connection,
        clients: [
            Mutex::new(Some(client)),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
        ],
        client_id,
        access: RwLock::new(()),
        cancelled,
        info: serde_json::to_value(&info).unwrap_or(Value::Null),
        used: std::sync::Mutex::new(Instant::now()),
        queries: queries::Queries::default(),
    });
    let mut open = slot.open.lock().await;
    if owner.is_revoked()
        || slot.epoch.load(Ordering::Acquire) != epoch
        || session.connection.is_closed()
    {
        session.close();
        return Err(Error::new(
            "STALE_CONNECTION",
            "Git session changed during startup. Reconnect.",
        ));
    }
    *open = Some(session.clone());
    let weak = Arc::downgrade(&session);
    let mut pressure = session.connection.channel_pressure();
    let mut maintenance = slot.maintenance.lock().unwrap();
    if let Some(previous) = maintenance.take() {
        previous.abort();
    }
    *maintenance = Some(tokio::spawn(async move {
        loop {
            let reclaim = tokio::select! {
                _ = tokio::time::sleep(KEEPALIVE) => false,
                changed = pressure.changed() => {
                    if changed.is_err() { break; }
                    true
                }
            };
            let Some(session) = weak.upgrade() else {
                break;
            };
            if *session.cancelled.borrow() {
                break;
            }
            if reclaim {
                // Lane locks span whole requests, including mutations. Drop
                // only idle cached streams; the next request reopens its lane.
                release_idle_lanes(&session.clients, |client| {
                    client.stream().uses_session_slot()
                });
                continue;
            }
            // Only close for inactivity when no operation is using the session.
            if let Ok(_access) = session.access.try_write() {
                let last_used = *session.used.lock().expect("usage clock");
                if idle_expired(last_used, Instant::now()) {
                    session.close();
                    break;
                }
            }
            // Every agent has its own idle receive deadline. Ping initialized
            // lanes concurrently so keepalive adds one round trip.
            for lane in &session.clients {
                if let Ok(mut client) = lane.try_lock()
                    && client.as_ref().is_some_and(|c| c.stream().is_draining())
                {
                    *client = None;
                }
            }
            session.connection.release_draining_runtime();
            ping_lanes(&session.clients).await;
        }
    }));
    Ok(session)
}

pub async fn git_connect(
    preferences: &dyn ClientIdentity,
    state: &Shared,
    server_id: Uuid,
) -> Result<Value, Error> {
    let session = connection(preferences, state, server_id).await?;
    Ok(json!({"serverId":server_id,"info":session.info}))
}

/// A refresh barrier for one checkout's observations. It neither opens an SSH
/// channel nor acquires configuration control. Subsequent reads share fresh
/// query work; other repositories and servers retain their cached pages.
pub async fn git_refresh_repository(
    state: &Shared,
    server_id: Uuid,
    repo_id: String,
) -> Result<Value, Error> {
    if repo_id.is_empty() {
        return Err(Error::invalid("Repository ID is required."));
    }
    let owner = state.owner(server_id).map_err(Error::transport)?;
    let session = owner.git.open.lock().await.clone();
    if owner.is_revoked() {
        return Err(Error::transport("Server session changed."));
    }
    if let Some(session) = session {
        session.queries.invalidate_repository(Some(&repo_id));
    }
    Ok(json!({"repoId":repo_id}))
}

pub async fn git_request(
    events: LogSink,
    preferences: &dyn ClientIdentity,
    state: &Shared,
    pending: &pending::PendingOperations,
    server_id: Uuid,
    request: Request,
) -> Result<Value, Error> {
    // Connect, or reconnect, BEFORE sending -- never after.
    let session = connection(preferences, state, server_id).await?;
    if queries::cacheable(&request) {
        return queries::read(&session, events, state, pending, request).await;
    }
    let mutation = is_mutation(&request) || matches!(request, Request::Close { .. });
    if mutation {
        session.queries.invalidate(&request);
    }
    let invalidation = request.clone();
    let result = execute(&session, events, state, pending, request).await;
    if mutation {
        session.queries.invalidate(&invalidation);
        if let Ok(owner) = server_session::resolve(state, server_id).await {
            owner.files.listings.invalidate(|_| true);
            owner.files.documents.invalidate(|_, _| true);
        }
    }
    result
}

async fn execute(
    session: &Arc<Session>,
    events: LogSink,
    state: &Shared,
    pending: &pending::PendingOperations,
    request: Request,
) -> Result<Value, Error> {
    let server_id = session.server_id;
    let mut cancel = session.cancelled.subscribe();
    if *cancel.borrow() {
        return Err(Error::transport("Git connection is closed. Retry."));
    }
    // Each channel owns a sequential protocol stream. Independent reads may use
    // different channels; mutations retain the original exclusive ordering.
    let _access = tokio::select! {
        biased;
        access = request_access(&session.access, &request) => access,
        _ = cancel.changed() => return Err(Error::transport("Git connection closed. Retry.")),
    };
    let mut guard = tokio::select! {
        biased;
        guard = session.clients[request_lane(&request)].lock() => guard,
        _ = cancel.changed() => return Err(Error::transport("Git connection closed. Retry.")),
    };
    if *cancel.borrow() {
        return Err(Error::transport("Git connection is closed. Retry."));
    }
    // Admission happens before sending a request. Retire this lane between
    // operations; never cancel or replay work already using its old runtime.
    if guard.as_ref().is_some_and(|c| c.stream().is_draining()) {
        *guard = None;
        session.connection.release_draining_runtime();
    }
    if guard.is_none() {
        let setup = async {
            let (stream, _) = session
                .connection
                .launch(crate::agent::Service::Git)
                .await
                .map_err(Error::transport)?;
            let (client, info) =
                Client::start_with_identity(stream, session.client_id.clone()).await?;
            Ok((client, info))
        };
        let initialized = tokio::select! {
            biased;
            result = tokio::time::timeout(DEADLINE, setup) => result
                .map_err(|_| Error::transport("Git channel initialization timed out."))
                .and_then(|result| result),
            _ = cancel.changed() => return Err(Error::transport("Git connection closed. Retry.")),
        };
        let (client, _) = match initialized {
            Ok(client) => client,
            Err(error) => {
                if !is_mutation(&request) && !session.connection.is_closed() {
                    return Err(Error::new(
                        "READ_CHANNEL_ERROR",
                        format!("Could not start the Git read channel: {}", error.message),
                    ));
                }
                // A failed idle lane is recreated here. If the underlying SSH
                // connection is also gone, do not leave it reusable forever.
                // Access is held and no request has been sent, so no write can
                // be interrupted or replayed by this connection reset.
                session.close();
                return Err(error);
            }
        };
        *guard = Some(client);
    }
    {
        let params =
            serde_json::to_value(&request).map_err(|_| Error::invalid("Invalid request"))?;
        let repo_id = params["params"]["repoId"].clone();
        let operation_id = params["params"]["operationId"].clone();
        if let Some(client) = guard.as_mut() {
            client.on_log = Some(Box::new(move |entry| {
                events(
                    serde_json::json!({"serverId":server_id,"repoId":repo_id,"operationId":operation_id,"entry":entry}),
                );
            }));
        }
    }
    session.touch();
    let operation_id = match &request {
        Request::Start { operation_id, .. }
        | Request::Init { operation_id, .. }
        | Request::Clone { operation_id, .. } => Some(operation_id.clone()),
        _ => None,
    };
    let observed_id = operation_id.clone().or_else(|| match &request {
        Request::Get { operation_id } | Request::Review { operation_id } => {
            Some(operation_id.clone())
        }
        _ => None,
    });
    let repository_write = matches!(&request, Request::Start { .. });
    let bootstrap_write = matches!(&request, Request::Init { .. } | Request::Clone { .. });
    let lookup = matches!(&request, Request::Get { .. });
    if let Some(id) = &operation_id {
        let id = Uuid::parse_str(id).map_err(|_| Error::invalid("Operation ID must be a UUID."))?;
        let value = serde_json::to_value(&request).map_err(|e| Error::invalid(e.to_string()))?;
        let action = value["params"]["action"]["kind"]
            .as_str()
            .or_else(|| value["method"].as_str())
            .ok_or_else(|| Error::invalid("Missing Git action."))?
            .to_owned();
        let ledger = pending.clone();
        let server_id = session.server_id;
        tokio::task::spawn_blocking(move || ledger.begin(server_id, id, action))
            .await
            .map_err(|e| Error::new("LOCAL_JOURNAL_UNAVAILABLE", e.to_string()))?
            .map_err(|e| Error::new("LOCAL_JOURNAL_UNAVAILABLE", e))?;
    }
    let deadline = if matches!(
        &request,
        Request::Clone { .. }
            | Request::RemoteRefs { .. }
            | Request::Start {
                action: protocol::Action::WorktreeRemove { .. }
                    | protocol::Action::WorktreePrune { .. }
                    | protocol::Action::WorktreeAdd { .. }
                    | protocol::Action::Fetch { .. }
                    | protocol::Action::BranchDeleteRemote { .. }
                    | protocol::Action::TagDeleteRemote { .. }
                    | protocol::Action::PushWithLease { .. }
                    | protocol::Action::Push { .. }
                    | protocol::Action::TagPush { .. }
                    | protocol::Action::PullFastForward { .. }
                    | protocol::Action::Rebase { .. }
                    | protocol::Action::IntegrationContinue { .. }
                    | protocol::Action::IntegrationSkip {},
                ..
            }
    ) {
        Duration::from_secs(300)
    } else {
        DEADLINE
    };
    let mut result = client::response(&mut guard, request, deadline, &mut cancel).await;
    if guard.as_ref().is_some_and(|c| c.stream().is_draining()) {
        *guard = None;
        session.connection.release_draining_runtime();
    }
    // A retiring runtime must not publish stale view data. Durable operation
    // receipts retain their own identity and continue through reconciliation.
    if result.is_ok() && observed_id.is_none() && session.connection.is_closed() {
        result = Err(Error::transport(
            "Needle runtime changed during this Git read. Request it again.",
        ));
    }
    if result
        .as_ref()
        .is_err_and(|e| e.code == "READ_CHANNEL_ERROR")
        && session.connection.is_closed()
    {
        // Only a confirmed transport loss escalates a read-channel failure
        // into a shared-session reset. A timed-out agent may be lane-local.
        session.close();
        return Err(Error::transport(
            "The SSH connection closed during this Git read.",
        ));
    }
    if result.as_ref().is_err_and(|e| {
        matches!(
            e.code.as_str(),
            "TRANSPORT_ERROR" | "PROTOCOL_ERROR" | "CANCELLED"
        )
    }) {
        *guard = None;
        session.cancelled.send_replace(true);
        session.close();
        if let Some(id) = &operation_id {
            result = Err(uncertain(id));
        }
    }
    session.touch();
    let current = state
        .owner(session.server_id)
        .is_ok_and(|owner| owner.revision() == session.revision && !session.connection.is_closed());
    if !current {
        session.cancelled.send_replace(true);
        session.close();
        return Err(operation_id.as_deref().map(uncertain).unwrap_or_else(|| {
            Error::new(
                "STALE_CONNECTION",
                "Server configuration changed during this request.",
            )
        }));
    }
    if let (Some(id), Ok(value)) = (observed_id.as_deref(), &result) {
        let outcome = pending::remote_outcome(value, id).map_err(|_| uncertain(id))?;
        let ledger = pending.clone();
        let server_id = session.server_id;
        let operation =
            Uuid::parse_str(id).map_err(|_| Error::invalid("Operation ID must be a UUID."))?;
        tokio::task::spawn_blocking(move || ledger.observe(server_id, operation, outcome))
            .await
            .map_err(|_| uncertain(id))?
            .map_err(|_| uncertain(id))?;
    }
    // The agent writes a running journal record before it executes, atomically and
    // synced, so a missing record proves the write never began. Resolving it here
    // is what stops one uncertain operation from blocking every later write.
    if let (Some(id), Err(error)) = (observed_id.as_deref(), &result)
        && lookup
        && error.code == "OPERATION_NOT_FOUND"
    {
        let ledger = pending.clone();
        let server_id = session.server_id;
        let operation =
            Uuid::parse_str(id).map_err(|_| Error::invalid("Operation ID must be a UUID."))?;
        tokio::task::spawn_blocking(move || {
            ledger.observe(server_id, operation, pending::ReceiptState::Rejected)
        })
        .await
        .map_err(|_| uncertain(id))?
        .map_err(|_| uncertain(id))?;
    }
    if let (Some(id), Err(error)) = (operation_id.as_deref(), &result)
        && ((repository_write && pending::rejected_before_mutation(&error.code))
            || (bootstrap_write && pending::bootstrap_rejected_before_mutation(&error.code)))
    {
        let ledger = pending.clone();
        let server_id = session.server_id;
        let operation =
            Uuid::parse_str(id).map_err(|_| Error::invalid("Operation ID must be a UUID."))?;
        tokio::task::spawn_blocking(move || {
            ledger.observe(server_id, operation, pending::ReceiptState::Rejected)
        })
        .await
        .map_err(|_| uncertain(id))?
        .map_err(|_| uncertain(id))?;
    }
    result
}

/// Closes Git channels without closing the shared server transport. In-flight
/// requests observe cancellation; the next request initializes fresh Git channels.
pub async fn git_disconnect(state: &Shared, server_id: Uuid) -> Result<(), Error> {
    let owner = state.sessions.get(server_id);
    if let Some(owner) = owner {
        owner.git.close().await;
    }
    Ok(())
}

// Test the exact remote reader on Unix without importing the Linux display stack.

#[cfg(all(test, unix))]
#[allow(dead_code)]
#[rustfmt::skip]
#[path = "../../../../tools/agent/src/git/journal.rs"]
mod journal;

fn uncertain(operation_id: &str) -> Error {
    Error::new(
        "OUTCOME_UNKNOWN",
        format!(
            "Operation {operation_id} may have completed. Reconnect and query operation.get with this ID; do not repeat the write."
        ),
    )
}

#[cfg(all(test, unix))]
#[allow(dead_code)]
#[rustfmt::skip]
#[path = "../../../../tools/agent/src/git/metrics.rs"]
#[allow(clippy::collapsible_if)] // Shared agent source uses Rust 2021.
mod metrics;

#[cfg(all(test, unix))]
#[allow(dead_code)]
#[rustfmt::skip]
#[path = "../../../../tools/agent/src/git/remotes.rs"]
#[allow(clippy::collapsible_if)] // Shared agent source uses Rust 2021.
mod remotes;

#[cfg(all(test, unix))]
#[rustfmt::skip]
#[path = "../../../../tools/agent/src/git/bootstrap.rs"]
mod bootstrap;

#[cfg(all(test, unix))]
#[rustfmt::skip]
#[path = "../../../../tools/agent/src/git/cloning.rs"]
mod cloning;

#[cfg(all(test, unix))]
#[allow(dead_code)]
#[rustfmt::skip]
#[path = "../../../../tools/agent/src/git/tokens.rs"]
mod tokens;

#[cfg(test)]
mod connection_tests {
    use super::*;
    struct Identity;
    impl ClientIdentity for Identity {
        fn git_client_id(&self) -> IdentityFuture {
            Box::pin(std::future::ready(Ok(Uuid::nil().to_string())))
        }
    }
    #[derive(Default)]
    struct GatedIdentity {
        resume: Arc<tokio::sync::Notify>,
    }
    impl ClientIdentity for GatedIdentity {
        fn git_client_id(&self) -> IdentityFuture {
            let resume = self.resume.clone();
            Box::pin(async move {
                resume.notified().await;
                Ok(Uuid::nil().to_string())
            })
        }
    }
    #[tokio::test]
    async fn slow_identity_is_server_scoped_and_disconnected_startup_never_reaches_ssh() {
        let directory = tempfile::tempdir().unwrap();
        let server: crate::model::Server = serde_json::from_value(json!({
            "id": Uuid::new_v4(), "name":"Slow identity", "sshUser":"fixture",
            "sshHost":"127.0.0.1", "sshPort":1
        }))
        .unwrap();
        let mut other = server.clone();
        other.id = Uuid::new_v4();
        let mut manager =
            crate::manager::Manager::new(crate::profile::Store::for_test(directory.path().into()));
        manager
            .config
            .servers
            .extend([server.clone(), other.clone()]);
        let owner = manager.sessions.session(&server, 0);
        let other_owner = manager.sessions.session(&other, 0);
        let state = crate::manager::share(manager);
        let identity = GatedIdentity::default();
        let resume = identity.resume.clone();
        let pending_state = state.clone();
        let id = server.id;
        let pending = tokio::spawn(async move { connection(&identity, &pending_state, id).await });
        tokio::time::timeout(Duration::from_secs(1), async {
            while owner.git.connecting.try_lock().is_ok() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        // No global manager or other-server startup gate is held. Same-server
        // disconnect can also fence the in-progress connection immediately.
        assert!(other_owner.git.connecting.try_lock().is_ok());
        assert!(state.current(other.id).is_ok());
        struct FailedIdentity;
        impl ClientIdentity for FailedIdentity {
            fn git_client_id(&self) -> IdentityFuture {
                Box::pin(std::future::ready(Err(
                    "Independent identity failure".into()
                )))
            }
        }
        let other_error = tokio::time::timeout(
            Duration::from_secs(1),
            connection(&FailedIdentity, &state, other.id),
        )
        .await
        .unwrap()
        .err()
        .expect("second server reached its identity provider");
        assert_eq!(other_error.message, "Independent identity failure");
        tokio::time::timeout(Duration::from_secs(1), git_disconnect(&state, server.id))
            .await
            .unwrap()
            .unwrap();
        resume.notify_one();
        let error = pending
            .await
            .unwrap()
            .err()
            .expect("stale startup rejected");
        // Port 1 cannot supply SSH; a transport failure would prove the stale
        // attempt proceeded too far instead of being fenced after persistence.
        assert_eq!(error.code, "STALE_CONNECTION");
        assert!(owner.git.open.lock().await.is_none());
    }

    #[tokio::test]
    #[ignore = "Run with cargo xtask test ssh -- git::connection_tests::service_owner"]
    async fn removal_drain_waits_for_admitted_work_and_leaves_ssh_alive() {
        let directory = tempfile::tempdir().unwrap();
        let server: crate::model::Server = serde_json::from_value(json!({
            "id": Uuid::new_v4(), "name":"Git drain fixture", "sshUser":"fixture", "sshHost":"127.0.0.1", "agentInstallApproved":true,
            "sshPort":std::env::var("NEWPORT_TEST_SSH_PORT").unwrap().parse::<u16>().unwrap(),
            "identityFile":std::env::var("NEWPORT_TEST_IDENTITY").unwrap()
        })).unwrap();
        let mut manager =
            crate::manager::Manager::new(crate::profile::Store::for_test(directory.path().into()));
        manager.config.servers.push(server.clone());
        let owner = manager.sessions.session(&server, 0);
        let state = crate::manager::share(manager);
        let ssh = owner.acquire(&server).await.unwrap();
        ssh.install(false).await.unwrap();
        let session = connection(&Identity, &state, server.id).await.unwrap();
        // Hold exactly the guard used for an admitted Git mutation. A bounded
        // uninstall drain must return busy without closing that request's stream.
        let admitted = session.access.write().await;
        assert!(
            tokio::time::timeout(Duration::from_millis(50), owner.git.drain())
                .await
                .is_err()
        );
        assert!(!*session.cancelled.borrow());
        assert!(session.clients[0].lock().await.is_some());
        assert_eq!(ssh.execute("printf alive", None).await.unwrap(), "alive");
        drop(admitted);
        tokio::time::timeout(Duration::from_secs(5), owner.git.drain())
            .await
            .unwrap();
        assert!(*session.cancelled.borrow());
        assert!(session.clients[0].lock().await.is_none());
        assert!(owner.git.open.lock().await.is_none());
        assert_eq!(
            ssh.execute("printf still-alive", None).await.unwrap(),
            "still-alive"
        );
        owner.agent_runtime.reset().await;
    }

    #[tokio::test]
    #[ignore = "Run with cargo xtask test ssh -- git::connection_tests::service_owner"]
    async fn service_owner_shares_channels_and_fences_interrupted_startup() {
        let directory = tempfile::tempdir().unwrap();
        let server: crate::model::Server = serde_json::from_value(json!({
            "id": Uuid::new_v4(), "name": "Git ownership fixture", "sshUser": "fixture", "agentInstallApproved":true,
            "sshHost": "127.0.0.1", "sshPort": std::env::var("NEWPORT_TEST_SSH_PORT").unwrap().parse::<u16>().unwrap(),
            "identityFile": std::env::var("NEWPORT_TEST_IDENTITY").unwrap()
        })).unwrap();
        let mut manager =
            crate::manager::Manager::new(crate::profile::Store::for_test(directory.path().into()));
        manager.config.servers.push(server.clone());
        let owner = manager.sessions.session(&server, 0);
        let state = crate::manager::share(manager);
        let ssh = owner.acquire(&server).await.unwrap();
        ssh.install(false).await.unwrap();
        let (a, b) = tokio::join!(
            connection(&Identity, &state, server.id),
            connection(&Identity, &state, server.id)
        );
        let a = a.unwrap();
        let b = b.unwrap();
        assert!(
            Arc::ptr_eq(&a, &b),
            "service state belongs to the server, not a caller facade"
        );
        let transport = owner.acquire(&server).await.unwrap();
        git_disconnect(&state, server.id).await.unwrap();
        assert!(*a.cancelled.borrow());
        assert_eq!(transport.execute("printf ok", None).await.unwrap(), "ok");

        // Hold identity acquisition after the per-owner startup gate is entered.
        let identity = GatedIdentity::default();
        let resume = identity.resume.clone();
        let pending_state = state.clone();
        let id = server.id;
        let pending = tokio::spawn(async move { connection(&identity, &pending_state, id).await });
        tokio::time::timeout(Duration::from_secs(3), async {
            while owner.git.connecting.try_lock().is_ok() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        git_disconnect(&state, server.id).await.unwrap();
        resume.notify_one();
        let error = pending
            .await
            .unwrap()
            .err()
            .expect("disconnected startup must not publish");
        assert_eq!(error.code, "STALE_CONNECTION");
        assert!(owner.git.open.lock().await.is_none());

        let next = connection(&Identity, &state, server.id).await.unwrap();
        state.lock().await.advance_connection_revision(server.id);
        tokio::time::timeout(Duration::from_secs(3), async {
            while !*next.cancelled.borrow() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let replacement = connection(&Identity, &state, server.id).await.unwrap();
        assert!(!Arc::ptr_eq(&next, &replacement));
        assert!(!*replacement.cancelled.borrow());
        state.lock().await.shutdown().await;
        assert!(*replacement.cancelled.borrow());
    }
    #[test]
    fn a_live_connection_for_the_current_configuration_is_reused() {
        assert!(reusable(false, 3, 3));
        // Closed, or opened for a configuration that has since changed: never.
        assert!(!reusable(true, 3, 3));
        assert!(!reusable(false, 2, 3));
    }
    #[test]
    fn only_a_connection_idle_for_the_whole_period_is_closed() {
        let start = Instant::now();
        assert!(!idle_expired(start, start));
        assert!(!idle_expired(start, start + IDLE - Duration::from_secs(1)));
        assert!(idle_expired(start, start + IDLE));
        // A clock that reads earlier than the last use is not idleness.
        assert!(!idle_expired(start + IDLE, start));
    }
    fn request(value: Value) -> Request {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn read_lanes_preserve_affinity_and_mutations_use_the_control_lane() {
        for (method, params, lane, mutation) in [
            ("repo.status", json!({"repoId":"r"}), 0, false),
            ("repo.branches", json!({"repoId":"r"}), 0, false),
            ("repo.history", json!({"repoId":"r"}), 1, false),
            (
                "repo.remote_refs",
                json!({"repoId":"r","remote":"origin","expectedToken":"t"}),
                3,
                false,
            ),
            ("repo.tag", json!({"repoId":"r","oid":"c"}), 2, false),
            (
                "repo.commit",
                json!({"repoId":"r","commitOid":"c"}),
                2,
                false,
            ),
            (
                "repo.history",
                json!({"repoId":"r","cursor":"next"}),
                1,
                false,
            ),
            (
                "repo.commit_files",
                json!({"repoId":"r","commitOid":"c"}),
                2,
                false,
            ),
            (
                "repo.commit_diff",
                json!({"repoId":"r","commitOid":"c"}),
                2,
                false,
            ),
            ("repo.blob", json!({"repoId":"r","oid":"b"}), 2, false),
            ("operation.get", json!({"operationId":"o"}), 0, false),
            ("operation.review", json!({"operationId":"o"}), 0, true),
            (
                "operation.start",
                json!({"operationId":"o","repoId":"r","expectedSnapshot":"s","action":{"kind":"integration.abort"}}),
                0,
                true,
            ),
            (
                "repo.init",
                json!({"operationId":"o","path":{"bytesB64":"L3RtcC9yZXBv"},"initialBranch":"main"}),
                0,
                true,
            ),
            (
                "repo.clone",
                json!({"operationId":"o","path":{"bytesB64":"L3RtcC9yZXBv"},"url":"https://example.com/repo"}),
                0,
                true,
            ),
        ] {
            let request = request(json!({"method":method,"params":params}));
            assert_eq!(request_lane(&request), lane, "{method}");
            assert_eq!(is_mutation(&request), mutation, "{method}");
        }
    }

    #[tokio::test]
    async fn reads_overlap_but_pending_mutations_exclude_and_order_later_reads() {
        let access = RwLock::new(());
        let read = request(json!({"method":"operation.get","params":{"operationId":"o"}}));
        let write = request(
            json!({"method":"operation.start","params":{"operationId":"o","repoId":"r","expectedSnapshot":"s","action":{"kind":"integration.abort"}}}),
        );
        let first = request_access(&access, &read).await;
        let second =
            tokio::time::timeout(Duration::from_millis(100), request_access(&access, &read))
                .await
                .unwrap();
        let mutation = request_access(&access, &write);
        tokio::pin!(mutation);
        assert!(
            tokio::time::timeout(Duration::from_millis(10), &mut mutation)
                .await
                .is_err()
        );
        drop(first);
        assert!(
            tokio::time::timeout(Duration::from_millis(10), &mut mutation)
                .await
                .is_err()
        );
        let later_read = request_access(&access, &read);
        tokio::pin!(later_read);
        assert!(
            tokio::time::timeout(Duration::from_millis(10), &mut later_read)
                .await
                .is_err()
        );
        drop(second);
        let exclusive = mutation.await;
        assert!(
            tokio::time::timeout(Duration::from_millis(10), &mut later_read)
                .await
                .is_err()
        );
        drop(exclusive);
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut later_read)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn keepalive_skips_active_and_uninitialized_lanes() {
        let lane: Mutex<Option<Client<tokio::io::DuplexStream>>> = Mutex::new(None);
        ping_idle(&lane).await;
        let _active = lane.lock().await;
        tokio::time::timeout(Duration::from_millis(100), ping_idle(&lane))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn channel_pressure_releases_idle_lanes_without_interrupting_inflight_work() {
        let lanes = [
            Mutex::new(Some(("mutation", true))),
            Mutex::new(Some(("idle exec read", true))),
            Mutex::new(Some(("idle forwarded read", false))),
        ];
        let active = lanes[0].lock().await;
        release_idle_lanes(&lanes, |(_, session)| *session);
        assert_eq!(*active, Some(("mutation", true)));
        assert_eq!(*lanes[1].lock().await, None);
        assert_eq!(*lanes[2].lock().await, Some(("idle forwarded read", false)));
        drop(active);
        release_idle_lanes(&lanes, |(_, session)| *session);
        assert_eq!(*lanes[0].lock().await, None);
    }

    #[test]
    fn the_slot_limit_is_gone() {
        let source = include_str!("mod.rs");
        let needle = ["Close an unused", " Git session first."].concat();
        assert!(!source.contains(&needle));
        assert!(!source.contains(&["Sema", "phore"].concat()));
    }
}

#[cfg(all(test, unix))]
#[allow(dead_code)]
#[rustfmt::skip]
#[path = "../../../../tools/agent/src/git/backend.rs"]
mod backend;

// The shared Git contract harness runs locally; production Needle supplies its
// private runtime directory through the same adapter in the agent's Git module.
#[cfg(all(test, unix))]
fn temporary_directory() -> std::io::Result<std::path::PathBuf> {
    Ok(std::env::temp_dir())
}

#[cfg(all(test, unix))]
#[allow(dead_code)]
#[rustfmt::skip]
#[path = "../../../../tools/agent/src/git/cli/mod.rs"]
#[allow(clippy::collapsible_if)] // Shared agent source uses Rust 2021.
mod cli;

#[cfg(all(test, unix))]
#[allow(dead_code)]
#[rustfmt::skip]
#[path = "../../../../tools/agent/src/git/command_log.rs"]
#[allow(clippy::collapsible_if)] // Shared agent source uses Rust 2021.
mod command_log;
