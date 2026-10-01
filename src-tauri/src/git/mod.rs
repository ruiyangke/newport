//! Tauri boundary for remote Git. Writes use durable operation IDs.
//!
//! One SSH connection per server with independent metadata, history, diff and remote-advertisement channels. The agent is
//! stateless -- repository ids, snapshots, entry ids and cursors are
//! self-describing tokens it re-verifies on every use -- so nothing a request
//! depends on lives in a connection. A connection can therefore be shared by
//! every part of the app, closed when idle, and replaced after a failure; the
//! next request simply connects again. A request is never re-sent because its
//! connection failed: a failure mid-request is reported as it happened, and a
//! write whose outcome was not seen is reported as possibly completed.
pub(crate) mod client;
pub(crate) mod pending;
#[allow(dead_code)]
#[path = "../../../tools/agent/src/git/protocol.rs"]
pub mod protocol;
mod reads;
use crate::{manager::Shared, ssh::ExecSession};
use client::Client;
use protocol::{Error, Request};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Arc, Weak},
    time::{Duration, Instant},
};
use tauri::State;
use tokio::sync::{watch, Mutex, RwLock};
use uuid::Uuid;

type Stream = russh::ChannelStream<russh::client::Msg>;
struct Session {
    server_id: Uuid,
    revision: u64,
    connection: ExecSession,
    clients: [Mutex<Option<Client<Stream>>>; 4],
    client_id: String,
    access: RwLock<()>,
    cancelled: watch::Sender<bool>,
    /// The agent's hello and capabilities, returned to every caller that asks
    /// to connect while this connection is the server's.
    info: Value,
    used: std::sync::Mutex<Instant>,
}
impl Session {
    fn touch(&self) {
        *self.used.lock().expect("usage clock") = Instant::now();
    }
    async fn close(&self) {
        self.cancelled.send_replace(true);
        self.connection.close().await;
    }
}
#[derive(Default)]
pub struct Sessions {
    /// The live connection for each server, if any.
    entries: Mutex<HashMap<Uuid, Arc<Session>>>,
    /// Same-server initialization is serialized without blocking other servers.
    connecting: Mutex<HashMap<Uuid, Weak<Mutex<()>>>>,
    identity: Mutex<Option<String>>,
    reads: reads::Reads,
}
fn verify_backend(info: &Value) -> Result<(), Error> {
    if info["capabilities"]["backend"].as_str() != Some("cli") {
        return Err(Error::new("UNSUPPORTED_CAPABILITY", "The agent requires an update to support the Git CLI backend. Update the agent and reconnect."));
    }
    Ok(())
}
const DEADLINE: Duration = Duration::from_secs(35);
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
    preferences: &crate::preferences::Preferences,
    state: &Shared,
    sessions: &Sessions,
    server_id: Uuid,
) -> Result<Arc<Session>, Error> {
    let (server, revision) = {
        let manager = state.lock().await;
        (
            manager.server(server_id).map_err(Error::transport)?,
            manager.connection_revision(server_id),
        )
    };
    let live = |sessions: &HashMap<Uuid, Arc<Session>>| {
        sessions
            .get(&server_id)
            .filter(|s| reusable(*s.cancelled.borrow(), s.revision, revision))
            .cloned()
    };
    if let Some(session) = live(&*sessions.entries.lock().await) {
        return Ok(session);
    }
    let connecting = {
        let mut connecting = sessions.connecting.lock().await;
        // Keep only in-flight attempts; removed servers do not leave lock entries.
        connecting.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = connecting.get(&server_id).and_then(Weak::upgrade) {
            lock
        } else {
            let lock = Arc::new(Mutex::new(()));
            connecting.insert(server_id, Arc::downgrade(&lock));
            lock
        }
    };
    let _connecting = connecting.lock().await;
    // Another caller may have connected while this one waited.
    if let Some(session) = live(&*sessions.entries.lock().await) {
        return Ok(session);
    }
    let stale = sessions.entries.lock().await.remove(&server_id);
    if let Some(stale) = stale {
        stale.close().await;
    }
    let client_id = {
        let mut identity = sessions.identity.lock().await;
        if identity.is_none() {
            *identity = Some(preferences.git_client_id().map_err(Error::transport)?);
        }
        identity.clone().expect("initialized identity")
    };
    let connection = ExecSession::connect(&server)
        .await
        .map_err(Error::transport)?;
    let setup = tokio::time::timeout(DEADLINE, async {
        let (stream, _) = crate::agent::launch(&connection, crate::agent::Service::Git)
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
            connection.close().await;
            return Err(e);
        }
    };
    if let Err(error) = verify_backend(&info) {
        connection.close().await;
        return Err(error);
    }
    let current = {
        let manager = state.lock().await;
        manager.server(server_id).is_ok() && manager.connection_revision(server_id) == revision
    };
    if !current {
        connection.close().await;
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
    });
    sessions
        .entries
        .lock()
        .await
        .insert(server_id, session.clone());
    let weak = Arc::downgrade(&session);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(KEEPALIVE).await;
            let Some(session) = weak.upgrade() else {
                break;
            };
            if *session.cancelled.borrow() {
                break;
            }
            // Only close for inactivity when no operation is using the session.
            if let Ok(_access) = session.access.try_write() {
                let last_used = *session.used.lock().expect("usage clock");
                if idle_expired(last_used, Instant::now()) {
                    session.close().await;
                    break;
                }
            }
            // Every agent has its own idle receive deadline. Ping initialized
            // lanes concurrently so keepalive adds one round trip.
            ping_lanes(&session.clients).await;
        }
    });
    Ok(session)
}

#[tauri::command]
pub async fn git_connect(
    preferences: State<'_, crate::preferences::Preferences>,
    state: State<'_, Shared>,
    sessions: State<'_, Sessions>,
    server_id: Uuid,
) -> Result<Value, Error> {
    let session = connection(&preferences, &state, &sessions, server_id).await?;
    Ok(json!({"serverId":server_id,"info":session.info}))
}

#[tauri::command]
pub fn git_register_read(sessions: State<'_, Sessions>, server_id: Uuid) -> Result<Uuid, Error> {
    sessions.reads.register(server_id)
}
#[tauri::command]
pub fn git_cancel_read(sessions: State<'_, Sessions>, server_id: Uuid, read_id: Uuid) {
    sessions.reads.cancel(server_id, read_id);
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri injects application state separately from wire arguments.
pub async fn git_request(
    app: tauri::AppHandle,
    preferences: State<'_, crate::preferences::Preferences>,
    state: State<'_, Shared>,
    sessions: State<'_, Sessions>,
    pending: State<'_, pending::PendingOperations>,
    server_id: Uuid,
    request: Request,
    read_id: Option<Uuid>,
) -> Result<Value, Error> {
    if read_id.is_some() && is_mutation(&request) {
        return Err(Error::invalid("Writes cannot use read cancellation."));
    }
    let mut read = read_id
        .map(|id| sessions.reads.claim(server_id, id))
        .transpose()?;
    // Connect, or reconnect, BEFORE sending -- never after.
    let session = tokio::select! {
        biased;
        _ = reads::cancelled(&mut read) => return Err(reads::cancelled_error()),
        result = connection(&preferences, &state, &sessions, server_id) => result?,
    };
    let mut cancel = session.cancelled.subscribe();
    if *cancel.borrow() {
        return Err(Error::transport("Git connection is closed. Retry."));
    }
    // Each channel owns a sequential protocol stream. Independent reads may use
    // different channels; mutations retain the original exclusive ordering.
    let _access = tokio::select! {
        biased;
        _ = reads::cancelled(&mut read) => return Err(reads::cancelled_error()),
        access = request_access(&session.access, &request) => access,
        _ = cancel.changed() => return Err(Error::transport("Git connection closed. Retry.")),
    };
    let mut guard = tokio::select! {
        biased;
        _ = reads::cancelled(&mut read) => return Err(reads::cancelled_error()),
        guard = session.clients[request_lane(&request)].lock() => guard,
        _ = cancel.changed() => return Err(Error::transport("Git connection closed. Retry.")),
    };
    if *cancel.borrow() {
        return Err(Error::transport("Git connection is closed. Retry."));
    }
    if guard.is_none() {
        let setup = async {
            let (stream, _) = crate::agent::launch(&session.connection, crate::agent::Service::Git)
                .await
                .map_err(Error::transport)?;
            let (client, info) =
                Client::start_with_identity(stream, session.client_id.clone()).await?;
            verify_backend(&info)?;
            Ok((client, info))
        };
        let initialized = tokio::select! {
            biased;
            _ = reads::cancelled(&mut read) => return Err(reads::cancelled_error()),
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
                session.close().await;
                return Err(error);
            }
        };
        *guard = Some(client);
    }
    {
        use tauri::Emitter;
        let params =
            serde_json::to_value(&request).map_err(|_| Error::invalid("Invalid request"))?;
        let repo_id = params["params"]["repoId"].clone();
        let operation_id = params["params"]["operationId"].clone();
        if let Some(client) = guard.as_mut() {
            client.on_log = Some(Box::new(move |entry| {
                let _ = app.emit("git-command-log", serde_json::json!({"serverId":server_id,"repoId":repo_id,"operationId":operation_id,"entry":entry}));
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
        let ledger = pending.inner().clone();
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
    let mut result = reads::response(&mut guard, request, deadline, &mut cancel, &mut read).await;
    if result.as_ref().is_err_and(|e| e.code == "READ_CANCELLED") {
        return result;
    }
    if result
        .as_ref()
        .is_err_and(|e| e.code == "READ_CHANNEL_ERROR")
        && session.connection.is_closed()
    {
        // Only a confirmed transport loss escalates a read-channel failure
        // into a shared-session reset. A timed-out agent may be lane-local.
        session.close().await;
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
        session.connection.close().await;
        if let Some(id) = &operation_id {
            result = Err(uncertain(id));
        }
    }
    session.touch();
    let current = {
        let manager = state.lock().await;
        manager.server(session.server_id).is_ok()
            && manager.connection_revision(session.server_id) == session.revision
    };
    if !current {
        session.cancelled.send_replace(true);
        session.connection.close().await;
        return Err(operation_id.as_deref().map(uncertain).unwrap_or_else(|| {
            Error::new(
                "STALE_CONNECTION",
                "Server configuration changed during this request.",
            )
        }));
    }
    if let (Some(id), Ok(value)) = (observed_id.as_deref(), &result) {
        let outcome = pending::remote_outcome(value, id).map_err(|_| uncertain(id))?;
        let ledger = pending.inner().clone();
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
    if let (Some(id), Err(error)) = (observed_id.as_deref(), &result) {
        if lookup && error.code == "OPERATION_NOT_FOUND" {
            let ledger = pending.inner().clone();
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
    }
    if let (Some(id), Err(error)) = (operation_id.as_deref(), &result) {
        if (repository_write && pending::rejected_before_mutation(&error.code))
            || (bootstrap_write && pending::bootstrap_rejected_before_mutation(&error.code))
        {
            let ledger = pending.inner().clone();
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
    }
    result
}

/// Closes the server's connection: for a server that was removed or edited, or
/// an explicit reset. Idempotent. Also cancels an in-flight read; closing the
/// transport does not wait for its mutex. The next request reconnects.
#[tauri::command]
pub async fn git_disconnect(sessions: State<'_, Sessions>, server_id: Uuid) -> Result<(), Error> {
    let removed = sessions.entries.lock().await.remove(&server_id);
    if let Some(session) = removed {
        session.close().await;
    }
    Ok(())
}

// Test the exact remote reader on Unix without importing the Linux display stack.

#[cfg(all(test, unix))]
#[allow(dead_code)]
#[path = "../../../tools/agent/src/git/journal.rs"]
mod journal;

fn uncertain(operation_id: &str) -> Error {
    Error::new("OUTCOME_UNKNOWN", format!("Operation {operation_id} may have completed. Reconnect and query operation.get with this ID; do not repeat the write."))
}

#[cfg(all(test, unix))]
#[allow(dead_code)]
#[path = "../../../tools/agent/src/git/metrics.rs"]
mod metrics;

#[cfg(all(test, unix))]
#[allow(dead_code)]
#[path = "../../../tools/agent/src/git/remotes.rs"]
mod remotes;

#[cfg(all(test, unix))]
#[path = "../../../tools/agent/src/git/bootstrap.rs"]
mod bootstrap;

#[cfg(all(test, unix))]
#[path = "../../../tools/agent/src/git/cloning.rs"]
mod cloning;

#[cfg(all(test, unix))]
#[allow(dead_code)]
#[path = "../../../tools/agent/src/git/tokens.rs"]
mod tokens;

#[cfg(test)]
mod connection_tests {
    use super::*;
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

    #[test]
    fn the_slot_limit_is_gone() {
        let source = include_str!("mod.rs");
        let needle = ["Close an unused", " Git session first."].concat();
        assert!(!source.contains(&needle));
        assert!(!source.contains(&["Sema", "phore"].concat()));
    }
}

#[cfg(test)]
mod backend_selection_tests {
    use super::*;
    #[test]
    fn selection_is_fixed_and_never_silently_falls_back() {
        assert!(verify_backend(&json!({"capabilities":{}})).is_err());
        assert!(verify_backend(&json!({"capabilities":{"backend":"cli"}})).is_ok());
        assert!(verify_backend(&json!({"capabilities":{"backend":"git2"}})).is_err());
    }
}

#[cfg(all(test, unix))]
#[allow(dead_code)]
#[path = "../../../tools/agent/src/git/backend.rs"]
mod backend;

#[cfg(all(test, unix))]
#[allow(dead_code)]
#[path = "../../../tools/agent/src/git/cli/mod.rs"]
mod cli;

#[cfg(all(test, unix))]
#[allow(dead_code)]
#[path = "../../../tools/agent/src/git/command_log.rs"]
mod command_log;
