//! One observable connectivity store per application profile. Only the current
//! server owner can publish; pages, probes and PTYs cannot overwrite its state.
use newport_domain::connectivity::{Phase, ServerState, Snapshot};
use newport_ssh::{Login, Reach, Retry, Status};
use serde::Serialize;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio::sync::watch;
use uuid::Uuid;

struct Entry {
    owner: Uuid,
    state: ServerState,
}
struct State {
    entries: HashMap<Uuid, Entry>,
    snapshot: Snapshot,
}
#[derive(Clone)]
pub(crate) struct Store {
    state: Arc<Mutex<State>>,
    updates: watch::Sender<Snapshot>,
}
impl Default for Store {
    fn default() -> Self {
        Self::new(Uuid::new_v4())
    }
}
impl Store {
    pub fn new(instance_id: Uuid) -> Self {
        let snapshot = Snapshot {
            instance_id,
            ..Snapshot::default()
        };
        Self {
            state: Arc::new(Mutex::new(State {
                entries: HashMap::new(),
                snapshot: snapshot.clone(),
            })),
            updates: watch::channel(snapshot).0,
        }
    }
    pub fn subscribe(&self) -> watch::Receiver<Snapshot> {
        self.updates.subscribe()
    }
    pub fn register(&self, server: Uuid, revision: u64) -> Publisher {
        let owner = Uuid::new_v4();
        let mut state = self.state.lock().unwrap();
        state.entries.insert(
            server,
            Entry {
                owner,
                state: ServerState {
                    connection_revision: revision,
                    ..Default::default()
                },
            },
        );
        self.publish(&mut state);
        Publisher {
            store: self.clone(),
            server,
            owner,
        }
    }
    pub fn remove(&self, server: Uuid) {
        let mut state = self.state.lock().unwrap();
        state.entries.remove(&server);
        self.publish(&mut state);
    }
    pub fn clear(&self) {
        let mut state = self.state.lock().unwrap();
        state.entries.clear();
        self.publish(&mut state);
    }
    fn publish(&self, state: &mut State) {
        let servers = state
            .entries
            .iter()
            .map(|(id, entry)| (*id, entry.state.clone()))
            .collect();
        if state.snapshot.servers != servers {
            state.snapshot.servers = servers;
            state.snapshot.revision += 1;
            self.updates.send_replace(state.snapshot.clone());
        }
    }
}

#[derive(Clone)]
pub(crate) struct Publisher {
    store: Store,
    server: Uuid,
    owner: Uuid,
}
impl Publisher {
    /// Publishes the server's link status; `generation` counts how many
    /// times the link has become ready.
    pub fn status(&self, status: &Status, generation: u64) {
        let mut state = self.store.state.lock().unwrap();
        let Some(entry) = state
            .entries
            .get_mut(&self.server)
            .filter(|e| e.owner == self.owner)
        else {
            return;
        };
        entry.state = ServerState {
            connection_revision: entry.state.connection_revision,
            ..server_state(status, generation)
        };
        self.store.publish(&mut state);
    }
}

/// How every surface shows a link's status.
fn server_state(status: &Status, generation: u64) -> ServerState {
    let approval = matches!(status.login, Some(Login::WaitingForApproval));
    let login_error = match &status.login {
        Some(Login::Failed(error)) => Some(error),
        _ => None,
    };
    let (phase, attempt, error) = match &status.reach {
        Reach::Ready { .. } => (Phase::Connected, 0, None),
        Reach::Failed(error) => (Phase::Failed, 0, Some(error)),
        _ if approval => (Phase::AwaitingApproval, 0, None),
        Reach::Idle => (Phase::Disconnected, 0, None),
        Reach::Connecting => (Phase::Connecting, 0, None),
        // Never connected: its retries aren't a reconnection.
        Reach::Reconnecting { attempt, .. } if generation == 0 => match login_error {
            Some(error) => (Phase::Failed, *attempt, Some(error)),
            None => (Phase::Connecting, *attempt, None),
        },
        Reach::Reconnecting { attempt, .. } => (Phase::Reconnecting, *attempt, login_error),
    };
    ServerState {
        connection_revision: 0,
        generation,
        phase,
        attempt,
        error: error.map(ToString::to_string),
        retryable: error.is_some_and(|error| error.retry() == Retry::Backoff),
    }
}

/// A server's link as the app's pages read it from the snapshot.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Transport {
    pub status: &'static str,
    /// How many times the link has become ready.
    pub generation: u64,
    /// Requests waiting for a connection or a free session.
    pub waiting: usize,
}
impl Transport {
    pub(crate) fn new(status: &Status, generation: u64) -> Self {
        let status_text = match server_state(status, generation).phase {
            Phase::Connected => "connected",
            Phase::Connecting | Phase::AwaitingApproval => "connecting",
            Phase::Reconnecting => "recovering",
            Phase::Failed => "failed",
            Phase::Disconnected => "disconnected",
        };
        Self {
            status: status_text,
            generation,
            waiting: status.waiting,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use newport_ssh::Error;

    fn status(reach: Reach, login: Option<Login>) -> Status {
        Status {
            reach,
            login,
            waiting: 0,
        }
    }
    fn ready() -> Status {
        status(Reach::Ready { connections: 1 }, None)
    }

    #[test]
    fn replacement_and_removal_fence_late_publications() {
        let store = Store::default();
        let updates = store.subscribe();
        let id = Uuid::new_v4();
        let old = store.register(id, 1);
        let next = store.register(id, 2);
        old.status(&ready(), 1);
        assert_eq!(updates.borrow().servers[&id].phase, Phase::Disconnected);
        next.status(&ready(), 1);
        assert_eq!(updates.borrow().servers[&id].phase, Phase::Connected);
        assert_eq!(updates.borrow().servers[&id].connection_revision, 2);
        store.remove(id);
        next.status(&ready(), 1);
        assert!(updates.borrow().servers.is_empty());
    }

    #[test]
    fn approval_publishes_once_for_all_subscribers_without_changing_other_servers() {
        let store = Store::default();
        let a = store.subscribe();
        let b = store.subscribe();
        let id = Uuid::new_v4();
        let other = Uuid::new_v4();
        let owner = store.register(id, 0);
        store.register(other, 0);
        owner.status(&status(Reach::Connecting, Some(Login::InProgress)), 0);
        assert_eq!(a.borrow().servers[&id].phase, Phase::Connecting);
        owner.status(
            &status(Reach::Connecting, Some(Login::WaitingForApproval)),
            0,
        );
        assert_eq!(a.borrow().servers[&id].phase, Phase::AwaitingApproval);
        assert_eq!(a.borrow().servers[&other].phase, Phase::Disconnected);
        owner.status(&ready(), 1);
        assert_eq!(*a.borrow(), *b.borrow());
        assert_eq!(a.borrow().servers[&id].phase, Phase::Connected);
        let revision = a.borrow().revision;
        owner.status(&ready(), 1);
        assert_eq!(
            a.borrow().revision,
            revision,
            "unchanged connectivity is not republished"
        );
    }

    #[test]
    fn a_server_that_never_connected_fails_rather_than_reconnects() {
        let store = Store::default();
        let updates = store.subscribe();
        let id = Uuid::new_v4();
        let owner = store.register(id, 0);
        let refused = status(
            Reach::Reconnecting {
                attempt: 2,
                next: None,
            },
            Some(Login::Failed(Error::Unreachable(
                "Connection refused".into(),
            ))),
        );
        owner.status(&refused, 0);
        let state = updates.borrow().servers[&id].clone();
        assert_eq!(state.phase, Phase::Failed);
        assert_eq!(state.attempt, 2);
        assert!(state.retryable);
        assert!(state.error.unwrap().contains("Connection refused"));
        owner.status(&refused, 1);
        assert_eq!(updates.borrow().servers[&id].phase, Phase::Reconnecting);
        assert_eq!(updates.borrow().servers[&id].attempt, 2);
    }

    #[test]
    fn a_link_that_stopped_trying_stays_failed_until_retry() {
        let store = Store::default();
        let updates = store.subscribe();
        let id = Uuid::new_v4();
        let owner = store.register(id, 0);
        // The last request may leave after the error; the server must not
        // look merely idle, or lose its error.
        owner.status(
            &status(
                Reach::Failed(Error::ApprovalRequired),
                Some(Login::Failed(Error::ApprovalRequired)),
            ),
            0,
        );
        let state = updates.borrow().servers[&id].clone();
        assert_eq!(state.phase, Phase::Failed);
        assert!(!state.retryable);
        assert_eq!(
            state.error.as_deref(),
            Some(Error::ApprovalRequired.to_string().as_str())
        );
        owner.status(
            &status(Reach::Connecting, Some(Login::WaitingForApproval)),
            0,
        );
        assert_eq!(updates.borrow().servers[&id].phase, Phase::AwaitingApproval);
        assert!(updates.borrow().servers[&id].error.is_none());
        owner.status(&status(Reach::Idle, None), 0);
        assert_eq!(updates.borrow().servers[&id].phase, Phase::Disconnected);
    }

    #[test]
    fn the_snapshot_transport_names_its_status_and_counts_readiness() {
        let transport = Transport::new(&ready(), 3);
        assert_eq!((transport.status, transport.generation), ("connected", 3));
        let lost = status(
            Reach::Reconnecting {
                attempt: 1,
                next: None,
            },
            Some(Login::InProgress),
        );
        assert_eq!(Transport::new(&lost, 3).status, "recovering");
        assert_eq!(
            Transport::new(&status(Reach::Idle, None), 0).status,
            "disconnected"
        );
    }
}
