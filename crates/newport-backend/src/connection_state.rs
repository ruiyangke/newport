//! One observable connectivity store per application profile. Only the current
//! server owner can publish; pages, probes and PTYs cannot overwrite its state.
use crate::session_transport;
use newport_domain::connectivity::{Phase, ServerState, Snapshot};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio::sync::watch;
use uuid::Uuid;

struct Entry {
    owner: Uuid,
    revision: u64,
    transport: session_transport::Snapshot,
}
struct State {
    entries: HashMap<Uuid, Entry>,
    approvals: HashMap<Uuid, usize>,
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
                approvals: HashMap::new(),
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
                revision,
                transport: Default::default(),
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
            .map(|(id, entry)| {
                let t = &entry.transport;
                let phase = match t.status {
                    "connected" => Phase::Connected,
                    _ if t.error.is_some() && !t.retryable => Phase::Failed,
                    "connecting" | "recovering" if state.approvals.contains_key(id) => {
                        Phase::AwaitingApproval
                    }
                    "connecting" => Phase::Connecting,
                    // Never connected: its retries aren't a reconnection.
                    "recovering" if t.generation == 0 => Phase::Failed,
                    "recovering" => Phase::Reconnecting,
                    "failed" if t.error.is_none() => Phase::Connecting,
                    "failed" => Phase::Failed,
                    _ => Phase::Disconnected,
                };
                (
                    *id,
                    ServerState {
                        connection_revision: entry.revision,
                        generation: t.generation,
                        phase,
                        attempt: t.attempt,
                        error: t.error.clone(),
                        retryable: t.retryable,
                    },
                )
            })
            .collect();
        if state.snapshot.servers != servers {
            state.snapshot.servers = servers;
            state.snapshot.revision += 1;
            self.updates.send_replace(state.snapshot.clone());
        }
    }
    fn approvals(&self, approvals: HashMap<Uuid, usize>) {
        let mut state = self.state.lock().unwrap();
        state.approvals = approvals;
        self.publish(&mut state);
    }
    pub async fn observe_approvals(self) {
        let mut approvals = crate::agent_approval::subscribe();
        loop {
            self.approvals(approvals.borrow_and_update().clone());
            if approvals.changed().await.is_err() {
                break;
            }
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
    pub fn transport(&self, snapshot: session_transport::Snapshot) {
        let mut state = self.store.state.lock().unwrap();
        let Some(entry) = state
            .entries
            .get_mut(&self.server)
            .filter(|e| e.owner == self.owner)
        else {
            return;
        };
        entry.transport = snapshot;
        self.store.publish(&mut state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replacement_and_removal_fence_late_transport_publications() {
        let store = Store::default();
        let updates = store.subscribe();
        let id = Uuid::new_v4();
        let old = store.register(id, 1);
        let next = store.register(id, 2);
        let connected = session_transport::Snapshot {
            status: "connected",
            generation: 1,
            ..Default::default()
        };
        old.transport(connected.clone());
        assert_eq!(updates.borrow().servers[&id].phase, Phase::Disconnected);
        next.transport(connected.clone());
        assert_eq!(updates.borrow().servers[&id].phase, Phase::Connected);
        assert_eq!(updates.borrow().servers[&id].connection_revision, 2);
        store.remove(id);
        next.transport(connected);
        assert!(updates.borrow().servers.is_empty());
    }
    #[test]
    fn approval_and_retry_publish_once_for_all_subscribers_without_changing_other_servers() {
        let store = Store::default();
        let a = store.subscribe();
        let b = store.subscribe();
        let id = Uuid::new_v4();
        let other = Uuid::new_v4();
        let owner = store.register(id, 0);
        store.register(other, 0);
        owner.transport(session_transport::Snapshot {
            status: "connecting",
            ..Default::default()
        });
        store.approvals(HashMap::from([(id, 1)]));
        assert_eq!(a.borrow().servers[&id].phase, Phase::AwaitingApproval);
        assert_eq!(a.borrow().servers[&other].phase, Phase::Disconnected);
        owner.transport(session_transport::Snapshot {
            status: "connected",
            generation: 1,
            ..Default::default()
        });
        assert_eq!(*a.borrow(), *b.borrow());
        assert_eq!(a.borrow().servers[&id].phase, Phase::Connected);
        let revision = a.borrow().revision;
        store.approvals(HashMap::new());
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
        let refused = |generation| session_transport::Snapshot {
            status: "recovering",
            generation,
            error: Some("Connection refused".into()),
            retryable: true,
            ..Default::default()
        };
        owner.transport(refused(0));
        assert_eq!(updates.borrow().servers[&id].phase, Phase::Failed);
        owner.transport(refused(1));
        assert_eq!(updates.borrow().servers[&id].phase, Phase::Reconnecting);
    }

    #[test]
    fn blocked_authentication_stays_failed_until_explicit_retry() {
        let store = Store::default();
        let updates = store.subscribe();
        let id = Uuid::new_v4();
        let owner = store.register(id, 0);
        // The last request may release demand after receiving the auth error.
        // This must not make the server appear merely idle or hide its error.
        owner.transport(session_transport::Snapshot {
            status: "disconnected",
            error: Some("SSH approval required".into()),
            ..Default::default()
        });
        store.approvals(HashMap::from([(id, 1)]));
        assert_eq!(updates.borrow().servers[&id].phase, Phase::Failed);
        owner.transport(session_transport::Snapshot {
            status: "failed",
            ..Default::default()
        });
        assert_eq!(updates.borrow().servers[&id].phase, Phase::Connecting);
        assert!(updates.borrow().servers[&id].error.is_none());
        owner.transport(session_transport::Snapshot {
            status: "connecting",
            ..Default::default()
        });
        assert_eq!(updates.borrow().servers[&id].phase, Phase::AwaitingApproval);
    }
}
