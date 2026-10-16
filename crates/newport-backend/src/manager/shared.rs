//! Published server owners are available independently of profile persistence.
//! Configuration admission is private to the persistence coordinator and
//! shutdown. Feature reads and SSH acquisition use published owners.
use super::Manager;
use crate::server_session::{Registry, ServerSession};
use std::{
    ops::{Deref, DerefMut},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
use uuid::Uuid;

pub type Shared = Arc<SharedState>;

pub struct SharedState {
    control: tokio::sync::Mutex<Manager>,
    snapshots: Arc<super::snapshot::Source>,
    pub(super) forwarding: Arc<super::forwarding::Forwarding>,
    pub(crate) sessions: Arc<Registry>,
    pub(crate) history_barrier: Arc<super::history::HistoryGates>,
    pub(crate) instance_id: Uuid,
    profile_directory: std::path::PathBuf,
    configuration_available: AtomicBool,
    pub(super) configuration_revision: AtomicU64,
}

pub fn share(manager: Manager) -> Shared {
    manager.publish_snapshot();
    Arc::new(SharedState {
        snapshots: manager.snapshots.clone(),
        forwarding: manager.forwarding.clone(),
        sessions: manager.sessions.clone(),
        history_barrier: manager.history_barrier.clone(),
        instance_id: manager.instance_id,
        profile_directory: manager.store.directory.clone(),
        configuration_available: AtomicBool::new(manager.load_error.is_none()),
        control: tokio::sync::Mutex::new(manager),
        configuration_revision: AtomicU64::new(0),
    })
}

impl SharedState {
    /// Capture committed metadata and its owner without entering the profile
    /// writer. Identity storage runs on the blocking executor, outside owners.
    pub(crate) async fn prepare_integration(
        &self,
        id: Uuid,
    ) -> Result<crate::integration_service::Start, String> {
        let (server, owner) = self.current(id)?;
        let directory = self.profile_directory.clone();
        let client =
            tokio::task::spawn_blocking(move || crate::clipboard::client_identity(&directory, id))
                .await
                .map_err(|e| format!("Integration identity task failed: {e}"))
                .and_then(|result| result);
        if owner.is_revoked() {
            return Err("Server session changed. Reconnect before retrying.".into());
        }
        let client = client.inspect_err(|error| {
            owner
                .integration
                .set_state(crate::model::ConnectionState::error(error.clone()));
        })?;
        Ok(crate::integration_service::Start {
            owner,
            server,
            client,
        })
    }

    pub(crate) fn forward_open_endpoint(
        &self,
        id: Uuid,
        remote_port: Option<u16>,
        expected: Option<&crate::model::Tunnel>,
        expected_connection: Option<&serde_json::Value>,
    ) -> Result<std::net::SocketAddrV4, String> {
        self.forwarding.open_endpoint(
            self.instance_id,
            id,
            remote_port,
            expected,
            expected_connection,
        )
    }
    pub(crate) fn connect_tunnel(&self, id: Uuid) -> Result<(), String> {
        self.forwarding.connect(id)
    }
    pub(crate) fn disconnect_tunnel(&self, id: Uuid) -> Option<Arc<super::TunnelCleanup>> {
        self.forwarding.disconnect(id)
    }
    pub(crate) fn tunnel_cleanup(&self, id: Uuid) -> Option<Arc<super::TunnelCleanup>> {
        self.forwarding.cleanup(id)
    }
    pub(crate) fn connect_discovered(
        &self,
        server: Uuid,
        address: &str,
        port: u16,
    ) -> Result<bool, String> {
        self.forwarding.connect_discovered(server, address, port)
    }
    pub(crate) fn connect_tunnel_batch(
        &self,
        server: Uuid,
        tunnels: &[crate::model::Tunnel],
        expected: Option<&serde_json::Value>,
    ) -> Result<Vec<crate::model::Tunnel>, String> {
        self.forwarding
            .connect_batch(self.instance_id, server, tunnels, expected)
    }

    pub fn snapshot(&self) -> super::Snapshot {
        self.snapshots.read()
    }

    pub(crate) fn observe_connection(
        &self,
        server: &crate::model::Server,
        revision: u64,
        started: std::time::Instant,
        result: Result<(), String>,
    ) {
        if let Ok(owner) = self.owner(server.id)
            && owner.revision() == revision
            && owner.server().same_connection(server)
        {
            owner.connectivity.lock().unwrap().observe(started, result);
        }
    }
    pub(crate) fn current(
        &self,
        id: Uuid,
    ) -> Result<(crate::model::Server, Arc<ServerSession>), String> {
        let current = self
            .sessions
            .current(id)
            .ok_or("Server no longer exists.")?;
        if current.1.is_revoked() {
            return Err("Server session changed. Reconnect before retrying.".into());
        }
        Ok(current)
    }
    pub(crate) fn owner(&self, id: Uuid) -> Result<Arc<ServerSession>, String> {
        let owner = self.sessions.get(id).ok_or("Server no longer exists.")?;
        if owner.is_revoked() {
            return Err("Server session changed. Reconnect before retrying.".into());
        }
        Ok(owner)
    }

    pub(crate) fn configuration_available(&self) -> bool {
        self.configuration_available.load(Ordering::Acquire)
    }

    pub(super) async fn configuration_control(&self) -> ControlGuard<'_> {
        ControlGuard {
            state: self,
            guard: self.control.lock().await,
            dirty: false,
        }
    }
    // Tests deliberately hold the writer to prove feature independence. Do not
    // expose a general manager lock to production feature/page dispatch.
    #[cfg(test)]
    pub(crate) async fn lock(&self) -> ControlGuard<'_> {
        self.configuration_control().await
    }
    /// Only the short, already-prepared commit runs on a blocking worker.
    pub(super) fn blocking_control(&self) -> ControlGuard<'_> {
        ControlGuard {
            state: self,
            guard: self.control.blocking_lock(),
            dirty: false,
        }
    }

    #[cfg(test)]
    pub(crate) fn try_lock(&self) -> Result<ControlGuard<'_>, tokio::sync::TryLockError> {
        Ok(ControlGuard {
            state: self,
            guard: self.control.try_lock()?,
            dirty: false,
        })
    }
}

pub(crate) struct ControlGuard<'a> {
    state: &'a SharedState,
    guard: tokio::sync::MutexGuard<'a, Manager>,
    dirty: bool,
}
impl Deref for ControlGuard<'_> {
    type Target = Manager;
    fn deref(&self) -> &Manager {
        &self.guard
    }
}
impl DerefMut for ControlGuard<'_> {
    fn deref_mut(&mut self) -> &mut Manager {
        self.dirty = true;
        &mut self.guard
    }
}
impl Drop for ControlGuard<'_> {
    fn drop(&mut self) {
        if self.dirty {
            self.state
                .configuration_revision
                .fetch_add(1, Ordering::AcqRel);
            // Publish in-memory edits as well as saved edits before releasing
            // the writer. Session replacement revokes its old queries/leases.
            self.guard.publish_snapshot();
            self.state
                .configuration_available
                .store(self.guard.load_error.is_none(), Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{model::Server, profile::Store};
    use std::time::Instant;

    #[tokio::test]
    async fn integration_preparation_bypasses_writer_and_keeps_the_published_owner() {
        let directory = tempfile::tempdir().unwrap();
        let mut manager = Manager::new(Store::for_test(directory.path().into()));
        let server: Server = serde_json::from_value(serde_json::json!({
            "id":Uuid::new_v4(), "name":"Integration fixture",
            "sshHost":"unconnected.invalid", "sshUser":"fixture", "sshPort":22
        }))
        .unwrap();
        manager.config.servers.push(server.clone());
        let state = share(manager);
        let original = state.owner(server.id).unwrap();
        let mut writer = state.lock().await;
        writer.config.servers[0].name = "Uncommitted name".into();
        let mut requests = tokio::task::JoinSet::new();
        for _ in 0..16 {
            let state = state.clone();
            requests.spawn(async move { state.prepare_integration(server.id).await });
        }
        let mut identity = None;
        // The writer stays locked until every request completes, which proves
        // independence. This deadline only catches a deadlock: first-use
        // identities also perform real file writes and fsync on loaded CI hosts.
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            while let Some(result) = requests.join_next().await {
                let prepared = result.unwrap().unwrap();
                assert!(state.try_lock().is_err(), "profile writer must remain held");
                assert!(Arc::ptr_eq(&prepared.owner, &original));
                assert_eq!(prepared.server.name, server.name);
                assert_eq!(prepared.client, *identity.get_or_insert(prepared.client));
            }
        })
        .await
        .expect("integration preparation must not wait for profile storage");
        writer.advance_connection_revision(server.id);
        drop(writer);
        assert!(original.is_revoked());
        let replacement = state.prepare_integration(server.id).await.unwrap();
        assert!(!Arc::ptr_eq(&replacement.owner, &original));
        assert_eq!(replacement.client, identity.unwrap());
        assert_eq!(replacement.server.name, "Uncommitted name");
        crate::manager::shutdown(&state).await;
    }

    #[tokio::test]
    async fn setup_metadata_publishes_without_replacing_the_connection_and_health_is_scoped() {
        let directory = tempfile::tempdir().unwrap();
        let mut manager = Manager::new(Store::for_test(directory.path().into()));
        let a: Server = serde_json::from_value(serde_json::json!({"id":Uuid::new_v4(),
            "name":"A", "sshHost":"unconnected.invalid", "sshUser":"fixture", "sshPort":22}))
        .unwrap();
        let b = Server {
            id: Uuid::new_v4(),
            ..a.clone()
        };
        manager.config.servers = vec![a.clone(), b.clone()];
        let state = share(manager);
        let original = state.owner(a.id).unwrap();
        let mut writer = state.lock().await;
        state.observe_connection(&a, 0, Instant::now(), Ok(()));
        state.observe_connection(&b, 0, Instant::now(), Ok(()));
        assert_eq!(
            original
                .connectivity
                .lock()
                .unwrap()
                .health(Instant::now())
                .status,
            "reachable"
        );
        writer.config.servers[0].agent_install_approved = true;
        writer.config.servers[0].setup_complete = true;
        drop(writer);
        let (profile, owner) = state.current(a.id).unwrap();
        assert!(profile.agent_install_approved && profile.setup_complete);
        assert!(Arc::ptr_eq(&original, &owner));
        assert!(
            state
                .sessions
                .profiles()
                .iter()
                .any(|(profile, _)| profile.id == a.id && profile.setup_complete)
        );
        state.lock().await.advance_connection_revision(a.id);
        state.observe_connection(&a, 0, Instant::now(), Ok(()));
        assert!(original.is_revoked());
        assert_eq!(
            state
                .owner(a.id)
                .unwrap()
                .connectivity
                .lock()
                .unwrap()
                .health(Instant::now())
                .status,
            "unknown"
        );
        assert_eq!(
            state
                .owner(b.id)
                .unwrap()
                .connectivity
                .lock()
                .unwrap()
                .health(Instant::now())
                .status,
            "reachable"
        );
        crate::manager::shutdown(&state).await;
    }
}
