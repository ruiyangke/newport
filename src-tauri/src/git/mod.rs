//! Tauri boundary for remote Git. Writes use durable operation IDs.
//!
//! One connection per server, reused by every Git request to it. The agent is
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
use crate::{manager::Shared, ssh::ExecSession};
use client::Client;
use protocol::{Error, Request};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tauri::State;
use tokio::sync::{watch, Mutex};
use uuid::Uuid;

type Stream = russh::ChannelStream<russh::client::Msg>;
struct Session {
    server_id: Uuid,
    revision: u64,
    connection: ExecSession,
    client: Mutex<Option<Client<Stream>>>,
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
    /// Held while connecting, so concurrent first requests share one
    /// connection instead of racing to open several.
    connecting: Mutex<()>,
    identity: Mutex<Option<String>>,
}
const DEADLINE: Duration = Duration::from_secs(35);
/// A connection unused this long is closed; the next request reconnects. Safe
/// only because the agent keeps no state a request depends on.
const IDLE: Duration = Duration::from_secs(600);
const KEEPALIVE: Duration = Duration::from_secs(15);

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
    let _connecting = sessions.connecting.lock().await;
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
        let stream = connection
            .stream("exec \"$HOME/.local/bin/newport-agent\" git-rpc --stdio")
            .await
            .map_err(Error::transport)?;
        tokio::time::timeout(
            Duration::from_secs(5),
            Client::start_with_identity(stream, client_id),
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
        client: Mutex::new(Some(client)),
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
            // Only an idle connection is checked or closed: a request in
            // flight holds the client, and is never interrupted by this loop.
            if let Ok(mut guard) = session.client.try_lock() {
                let Some(client) = guard.as_mut() else {
                    break;
                };
                let last_used = *session.used.lock().expect("usage clock");
                if idle_expired(last_used, Instant::now())
                    || !matches!(
                        tokio::time::timeout(Duration::from_secs(5), client.ping()).await,
                        Ok(Ok(()))
                    )
                {
                    *guard = None;
                    session.close().await;
                    break;
                }
            };
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
pub async fn git_request(
    preferences: State<'_, crate::preferences::Preferences>,
    state: State<'_, Shared>,
    sessions: State<'_, Sessions>,
    pending: State<'_, pending::PendingOperations>,
    server_id: Uuid,
    request: Request,
) -> Result<Value, Error> {
    // Connect, or reconnect, BEFORE sending -- never after.
    let session = connection(&preferences, &state, &sessions, server_id).await?;
    let mut cancel = session.cancelled.subscribe();
    if *cancel.borrow() {
        return Err(Error::transport("Git connection is closed. Retry."));
    }
    // The agent serves one request at a time. Everything in the app shares
    // this connection, so a request waits its turn rather than being refused.
    let mut guard = tokio::select! {
        guard = session.client.lock() => guard,
        _ = cancel.changed() => return Err(Error::transport("Git connection closed. Retry.")),
    };
    session.touch();
    let client = guard
        .as_mut()
        .ok_or_else(|| Error::transport("Git connection is closed. Retry."))?;
    let operation_id = match &request {
        Request::Start { operation_id, .. }
        | Request::Init { operation_id, .. }
        | Request::Clone { operation_id, .. } => Some(operation_id.clone()),
        _ => None,
    };
    let observed_id = operation_id.clone().or_else(|| match &request {
        Request::Get { operation_id } => Some(operation_id.clone()),
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
    let mut result = tokio::select! {
        _=cancel.changed()=>Err(Error::new("CANCELLED","Git session was disconnected.")),
        response=tokio::time::timeout(deadline,client.request(request))=>response.unwrap_or_else(|_|Err(Error::transport("Git request timed out. Reconnect before continuing."))),
    };
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
#[path = "../../../tools/agent/src/git/repository.rs"]
mod repository;

#[cfg(all(test, unix))]
#[allow(dead_code)]
#[path = "../../../tools/agent/src/git/journal.rs"]
mod journal;
#[cfg(all(test, unix))]
#[allow(dead_code)]
#[path = "../../../tools/agent/src/git/operations.rs"]
mod operations;

fn uncertain(operation_id: &str) -> Error {
    Error::new("OUTCOME_UNKNOWN", format!("Operation {operation_id} may have completed. Reconnect and query operation.get with this ID; do not repeat the write."))
}

#[cfg(all(test, unix))]
#[allow(dead_code)]
#[path = "../../../tools/agent/src/git/branches.rs"]
mod branches;

#[cfg(all(test, unix))]
#[allow(dead_code)]
#[path = "../../../tools/agent/src/git/checkout.rs"]
mod checkout;

#[cfg(all(test, unix))]
#[allow(dead_code)]
#[path = "../../../tools/agent/src/git/remotes.rs"]
mod remotes;

#[cfg(all(test, unix))]
#[allow(dead_code)]
#[path = "../../../tools/agent/src/git/integration.rs"]
mod integration;

#[cfg(all(test, unix))]
#[allow(dead_code)]
#[path = "../../../tools/agent/src/git/stash.rs"]
mod stash;

#[cfg(all(test, unix))]
#[path = "../../../tools/agent/src/git/tags.rs"]
mod tags;

#[cfg(all(test, unix))]
#[path = "../../../tools/agent/src/git/replay.rs"]
mod replay;

#[cfg(all(test, unix))]
#[path = "../../../tools/agent/src/git/rebase.rs"]
mod rebase;

#[cfg(all(test, unix))]
#[path = "../../../tools/agent/src/git/reset.rs"]
mod reset;

#[cfg(all(test, unix))]
#[path = "../../../tools/agent/src/git/discard.rs"]
mod discard;

#[cfg(all(test, unix))]
#[path = "../../../tools/agent/src/git/conflicts.rs"]
mod conflicts;

#[cfg(all(test, unix))]
#[path = "../../../tools/agent/src/git/remote_rename.rs"]
mod remote_rename;

#[cfg(all(test, unix))]
#[path = "../../../tools/agent/src/git/bootstrap.rs"]
mod bootstrap;

#[cfg(all(test, unix))]
#[path = "../../../tools/agent/src/git/worktrees.rs"]
mod worktrees;

#[cfg(all(test, unix))]
#[path = "../../../tools/agent/src/git/cloning.rs"]
mod cloning;

#[cfg(all(test, unix))]
#[path = "../../../tools/agent/src/git/hunks.rs"]
mod hunks;

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
    #[test]
    fn the_slot_limit_is_gone() {
        let source = include_str!("mod.rs");
        let needle = ["Close an unused", " Git session first."].concat();
        assert!(!source.contains(&needle));
        assert!(!source.contains(&["Sema", "phore"].concat()));
    }
}
