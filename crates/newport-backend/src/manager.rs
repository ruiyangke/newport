use crate::{model::*, profile::Store, ssh};
use serde::Serialize;
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{sync::watch, task::JoinHandle};
use uuid::Uuid;

#[derive(Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Runtime {
    pub agent_transport: HashMap<Uuid, crate::session_transport::Snapshot>,
    pub agent_runtime: HashMap<Uuid, crate::agent_runtime::Snapshot>,
    pub agent_permissions: HashMap<Uuid, crate::integration_permissions::Snapshot>,
    pub agent_status: HashMap<Uuid, crate::agent_lifecycle::Status>,
    pub agent_installation: HashMap<Uuid, crate::agent_install::Progress>,
    pub agent_operations: HashMap<Uuid, serde_json::Value>,
    pub tunnels: HashMap<Uuid, ConnectionState>,
    pub tunnel_health: HashMap<Uuid, Vec<DestinationHealth>>,
    pub clipboard: HashMap<Uuid, ConnectionState>,
    pub clipboard_messages: HashMap<Uuid, String>,
    pub clipboard_path_needed: HashMap<Uuid, bool>,
    pub health: HashMap<Uuid, String>,
    pub connectivity: HashMap<Uuid, crate::connectivity::Health>,
    pub connection_revisions: HashMap<Uuid, u64>,
    pub awaiting_agent: Vec<Uuid>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub instance_id: Uuid,
    pub revision: u64,
    pub config: Config,
    pub runtime: Runtime,
    pub load_error: Option<String>,
}
mod configuration;
mod forwarding;
mod history;
mod shared;
mod snapshot;
pub use shared::{Shared, SharedState, share};
// Acquire before the manager lock. History I/O must not block snapshots or unrelated commands.
pub async fn history_guard(state: &Shared) -> tokio::sync::OwnedRwLockWriteGuard<()> {
    state.history_barrier.clear().await
}
pub async fn server_history_guard(state: &Shared, id: Uuid) -> history::ServerHistoryGuard {
    state.history_barrier.server(id).await
}
pub struct Manager {
    pub sessions: Arc<crate::server_session::Registry>,
    pub store: Arc<Store>,
    pub config: Config,
    pub runtime: Arc<Mutex<Runtime>>,
    pub load_error: Option<String>,
    pub history_barrier: Arc<history::HistoryGates>,
    forwarding: Arc<forwarding::Forwarding>,
    profile_cleanup: Vec<Arc<ProfileCleanup>>,
    next_connection_revision: u64,
    snapshots: Arc<snapshot::Source>,
    instance_id: Uuid,
    configuration_closed: bool,
}
struct Running {
    cancel: watch::Sender<bool>,
    task: JoinHandle<()>,
    publication: TunnelPublication,
    local_ports: Vec<u16>,
    previous: Vec<Arc<TunnelCleanup>>,
}
impl Running {
    fn retire(&self) {
        self.publication.live.store(false, Ordering::Release);
        self.cancel.send_replace(true);
    }
    async fn stop(mut self) {
        self.retire();
        let _ = (&mut self.task).await;
        // A cancelled replacement may not have reached its predecessor wait.
        // Keep the full cleanup chain before admitting another listener.
        for previous in &self.previous {
            previous.wait().await;
        }
    }
}
impl Drop for Running {
    fn drop(&mut self) {
        self.retire();
        self.task.abort();
    }
}

/// Owned by Forwarding even if the requesting command or window goes away.
pub(crate) struct TunnelCleanup {
    local_ports: Vec<u16>,
    task: JoinHandle<()>,
    finished: watch::Receiver<bool>,
}
impl TunnelCleanup {
    fn start(running: Running) -> Arc<Self> {
        running.retire();
        let mut local_ports = running.local_ports.clone();
        for previous in &running.previous {
            local_ports.extend_from_slice(&previous.local_ports);
        }
        local_ports.sort_unstable();
        local_ports.dedup();
        let (done, finished) = watch::channel(false);
        let task = tokio::spawn(async move {
            running.stop().await;
            done.send_replace(true);
        });
        Arc::new(Self {
            local_ports,
            task,
            finished,
        })
    }
    pub(crate) async fn wait(&self) {
        let mut finished = self.finished.clone();
        while !*finished.borrow_and_update() {
            if finished.changed().await.is_err() {
                break;
            }
        }
    }
}
impl Drop for TunnelCleanup {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub(crate) struct ProfileCleanup {
    task: JoinHandle<()>,
    outcome: watch::Receiver<Option<Result<(), String>>>,
}
impl ProfileCleanup {
    pub(crate) async fn wait(&self) -> Result<(), String> {
        let mut outcome = self.outcome.clone();
        loop {
            if let Some(result) = outcome.borrow_and_update().clone() {
                return result;
            }
            if outcome.changed().await.is_err() {
                return Err("Server removed, but local cleanup was interrupted.".into());
            }
        }
    }
}
impl Drop for ProfileCleanup {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[derive(Clone)]
struct TunnelPublication {
    runtime: Arc<Mutex<Runtime>>,
    id: Uuid,
    live: Arc<AtomicBool>,
}
impl TunnelPublication {
    fn state(&self, state: ConnectionState) {
        let mut runtime = self.runtime.lock().unwrap();
        if self.live.load(Ordering::Acquire) {
            if state.status != Status::Connected {
                runtime.tunnel_health.remove(&self.id);
            }
            runtime.tunnels.insert(self.id, state);
        }
    }
    fn health(&self, health: Vec<DestinationHealth>) {
        let mut runtime = self.runtime.lock().unwrap();
        if self.live.load(Ordering::Acquire) {
            runtime.tunnel_health.insert(self.id, health);
        }
    }
}
struct Shutdown {
    sessions: Arc<crate::server_session::Registry>,
    tunnels: Vec<Running>,
    stopping: Vec<Arc<TunnelCleanup>>,
    profiles: Vec<Arc<ProfileCleanup>>,
}
impl Shutdown {
    async fn run(self) {
        self.sessions.shutdown().await;
        for tunnel in self.tunnels {
            tunnel.stop().await;
        }
        for cleanup in self.stopping {
            cleanup.wait().await;
        }
        for cleanup in self.profiles {
            if let Err(error) = cleanup.wait().await {
                log::warn!("{error}");
            }
        }
    }
}
/// Close configuration admission and publish cancellation, then await scoped
/// service cleanup outside the coordinator.
pub async fn shutdown(state: &Shared) {
    let shutdown = state.configuration_control().await.begin_shutdown();
    shutdown.run().await;
}
impl Manager {
    pub fn new(store: Store) -> Self {
        let (config, load_error) = match store.load() {
            Ok(c) => (c, None),
            Err(e) => (Config::default(), Some(e)),
        };
        let runtime = Arc::new(Mutex::new(Runtime::default()));
        let instance_id = Uuid::new_v4();
        let manager = Self {
            sessions: Arc::new(crate::server_session::Registry::with_instance(
                &store.directory,
                instance_id,
            )),
            store: Arc::new(store),
            config,
            load_error,
            runtime: runtime.clone(),
            forwarding: Arc::new(forwarding::Forwarding::new(runtime.clone())),
            profile_cleanup: Vec::new(),
            history_barrier: Arc::default(),
            next_connection_revision: 0,
            snapshots: Arc::new(snapshot::Source::new(instance_id, runtime)),
            instance_id,
            configuration_closed: false,
        };
        manager.publish_sessions();
        manager
    }
    fn publish_sessions(&self) {
        for owner in self.sessions.all() {
            if !self
                .config
                .servers
                .iter()
                .any(|server| server.id == owner.server().id)
            {
                self.sessions.invalidate(owner.server().id);
            }
        }
        for server in &self.config.servers {
            // Session owners and gates are cheap; SSH starts only with demand.
            self.sessions
                .session(server, self.connection_revision(server.id));
        }
    }
    pub fn integration_access(&self) -> crate::integration_permissions::Access {
        self.publish_sessions();
        crate::integration_permissions::Access::new(self.sessions.clone(), self.instance_id)
    }
    pub fn connection_revision(&self, id: Uuid) -> u64 {
        self.runtime
            .lock()
            .unwrap()
            .connection_revisions
            .get(&id)
            .copied()
            .unwrap_or(0)
    }
    /// Identity observed by a native view, including credentials revision and
    /// backend lifetime. Constructing it must not collect a full runtime snapshot.
    pub fn connection_scope(&self, id: Uuid) -> Result<serde_json::Value, String> {
        let server = serde_json::to_value(self.server(id)?).map_err(|e| e.to_string())?;
        Ok(newport_domain::connection_scope::connection(
            &server,
            &serde_json::json!({
                "instanceId": self.instance_id,
                "runtime": {"connectionRevisions": {id.to_string(): self.connection_revision(id)}}
            }),
        ))
    }

    pub fn advance_connection_revision(&mut self, id: Uuid) {
        self.sessions.invalidate(id);
        self.next_connection_revision += 1;
        self.runtime
            .lock()
            .unwrap()
            .connection_revisions
            .insert(id, self.next_connection_revision);
        if let Ok(server) = self.server(id) {
            self.sessions.session(&server, self.connection_revision(id));
        }
    }
    pub fn matches_connection(&self, server: &Server, revision: u64) -> bool {
        self.connection_revision(server.id) == revision
            && self
                .config
                .servers
                .iter()
                .any(|current| current.id == server.id && current.same_connection(server))
    }
    pub fn observe_connection(
        &self,
        server: &Server,
        revision: u64,
        started: Instant,
        result: Result<(), String>,
    ) {
        if self.matches_connection(server, revision) {
            self.sessions
                .session(server, revision)
                .connectivity
                .lock()
                .unwrap()
                .observe(started, result);
        }
    }
    fn publish_snapshot(&self) {
        self.publish_sessions();
        self.publish_forwarding();
        let owners = self
            .config
            .servers
            .iter()
            .filter_map(|server| self.sessions.get(server.id))
            .collect();
        self.snapshots
            .publish(self.config.clone(), self.load_error.clone(), owners);
    }
    pub fn snapshot(&self) -> Snapshot {
        self.publish_snapshot();
        self.snapshots.read()
    }
    #[cfg(test)]
    pub async fn save(&mut self, config: Config) -> Result<(), String> {
        self.save_with_password(config, None).await
    }
    #[cfg(test)]
    pub async fn save_with_password(
        &mut self,
        config: Config,
        password: Option<(Uuid, zeroize::Zeroizing<String>)>,
    ) -> Result<(), String> {
        if let Some(e) = &self.load_error {
            return Err(e.clone());
        }
        let password_id = password.as_ref().map(|(id, _)| *id);
        let store = self.store.clone();
        let saved = config.clone();
        tokio::task::spawn_blocking(move || {
            store.save_with_password(
                &saved,
                password.as_ref().map(|(id, secret)| (*id, secret.as_str())),
            )
        })
        .await
        .map_err(|e| format!("Profile storage task failed: {e}"))??;
        self.accept_saved(config, password_id);
        Ok(())
    }
    fn accept_saved(&mut self, config: Config, password_id: Option<Uuid>) {
        let changed: Vec<_> = config
            .servers
            .iter()
            .filter(|server| {
                password_id == Some(server.id)
                    || self
                        .config
                        .servers
                        .iter()
                        .find(|old| old.id == server.id)
                        .is_none_or(|old| !old.same_connection(server))
            })
            .map(|server| server.id)
            .collect();
        self.config = config;
        for id in changed {
            self.advance_connection_revision(id);
        }
        self.publish_sessions();
        self.publish_forwarding();
    }
    pub fn server(&self, id: Uuid) -> Result<Server, String> {
        self.config
            .servers
            .iter()
            .find(|s| s.id == id)
            .cloned()
            .ok_or("Server no longer exists.".into())
    }
    pub(crate) fn track_profile_cleanup(
        &mut self,
        cleanup: impl std::future::Future<Output = Result<(), String>> + Send + 'static,
    ) -> Arc<ProfileCleanup> {
        self.profile_cleanup
            .retain(|cleanup| !cleanup.task.is_finished());
        let (done, outcome) = watch::channel(None);
        let task = tokio::spawn(async move {
            done.send_replace(Some(cleanup.await));
        });
        let cleanup = Arc::new(ProfileCleanup { task, outcome });
        self.profile_cleanup.push(cleanup.clone());
        cleanup
    }
    fn publish_forwarding(&self) {
        let revisions = self.runtime.lock().unwrap().connection_revisions.clone();
        self.forwarding
            .publish(&self.config, revisions, self.load_error.clone());
    }
    pub fn connect(&mut self, id: Uuid) -> Result<(), String> {
        self.publish_forwarding();
        self.forwarding.connect(id)
    }
    #[cfg(test)]
    pub(crate) fn begin_disconnect(&mut self, id: Uuid) -> Option<Arc<TunnelCleanup>> {
        self.forwarding.disconnect(id)
    }
    pub(crate) fn tunnel_cleanup(&self, id: Uuid) -> Option<Arc<TunnelCleanup>> {
        self.forwarding.cleanup(id)
    }
    #[cfg(test)]
    pub async fn disconnect(&mut self, id: Uuid) {
        if let Some(cleanup) = self.begin_disconnect(id) {
            cleanup.wait().await;
        }
    }
    #[cfg(test)]
    pub(crate) fn prepare_integration(
        &self,
        id: Uuid,
    ) -> Result<crate::integration_service::Start, String> {
        let server = self.server(id)?;
        let owner = self.sessions.session(&server, self.connection_revision(id));
        // Reusing a session must not replay saved consent over a newer local
        // revocation whose persistence failed. Explicit preference actions own
        // updates; a newly created session starts from the saved configuration.
        let client = crate::clipboard::client_identity(&self.store.directory, id)?;
        Ok(crate::integration_service::Start {
            owner,
            server,
            client,
        })
    }
    pub fn active_tunnels(&self, server: Uuid) -> Vec<Uuid> {
        self.forwarding.active(server)
    }
    #[cfg(test)]
    pub(crate) fn forward_open_endpoint(
        &self,
        id: Uuid,
        remote_port: Option<u16>,
        expected: Option<&Tunnel>,
        expected_connection: Option<&serde_json::Value>,
    ) -> Result<std::net::SocketAddrV4, String> {
        self.publish_forwarding();
        self.forwarding.open_endpoint(
            self.instance_id,
            id,
            remote_port,
            expected,
            expected_connection,
        )
    }
    #[cfg(test)]
    pub async fn remember_clipboard(&mut self, id: Uuid, enabled: bool) -> Result<(), String> {
        self.remember_integration(id, false, enabled).await
    }
    #[cfg(test)]
    pub async fn remember_integration(
        &mut self,
        id: Uuid,
        browser: bool,
        enabled: bool,
    ) -> Result<(), String> {
        let original = self.server(id)?;
        let owner = self
            .sessions
            .session(&original, self.connection_revision(id));
        let preference = owner.permissions.begin_preference(browser, enabled);
        self.remember_integration_preference(owner, preference)
            .await
    }
    #[cfg(test)]
    pub(crate) async fn remember_integration_preference(
        &mut self,
        owner: Arc<crate::server_session::ServerSession>,
        intent: crate::integration_permissions::Preference,
    ) -> Result<(), String> {
        let id = owner.server().id;
        let current = self.sessions.get(id).ok_or("Server connection changed.")?;
        if !Arc::ptr_eq(&current, &owner) || owner.is_revoked() {
            return Err("Server connection changed. Review its integrations and try again.".into());
        }
        if !owner.permissions.preference_is_current(intent) {
            return Err("Integration preference was superseded by a newer choice.".into());
        }
        let browser = intent.browser;
        let enabled = intent.enabled;
        let mut config = self.config.clone();
        let server = config
            .servers
            .iter_mut()
            .find(|server| server.id == id)
            .ok_or("Server no longer exists.")?;
        let preference = if browser {
            &mut server.browser_enabled
        } else {
            &mut server.clipboard_enabled
        };
        if *preference == enabled {
            return owner
                .permissions
                .commit_preference(intent)
                .then_some(())
                .ok_or_else(|| "Integration preference was superseded by a newer choice.".into());
        }
        *preference = enabled;
        self.save(config).await.map_err(|error| {
            if enabled { error } else {
                format!("Access is disabled for this connection, but the preference could not be saved. Retry before restarting Newport: {error}")
            }
        })?;
        owner
            .permissions
            .commit_preference(intent)
            .then_some(())
            .ok_or_else(|| {
                "Integration preference changed while saving. Review it and try again.".into()
            })
    }
    fn begin_shutdown(&mut self) -> Shutdown {
        self.configuration_closed = true;
        self.sessions.request_shutdown();
        let (tunnels, stopping) = self.forwarding.shutdown();
        Shutdown {
            sessions: self.sessions.clone(),
            tunnels,
            stopping,
            profiles: std::mem::take(&mut self.profile_cleanup),
        }
    }
    pub async fn shutdown(&mut self) {
        self.begin_shutdown().run().await;
    }
}

#[cfg(test)]
#[path = "manager_tunnel_tests.rs"]
mod tunnel_cleanup_tests;
fn set_tunnel_state(runtime: &Arc<Mutex<Runtime>>, id: Uuid, state: ConnectionState) {
    let mut r = runtime.lock().unwrap();
    if state.status != Status::Connected {
        r.tunnel_health.remove(&id);
    }
    r.tunnels.insert(id, state);
}
fn spawn(
    server: Server,
    tunnel: Tunnel,
    id: Uuid,
    runtime: Arc<Mutex<Runtime>>,
    previous: Vec<Arc<TunnelCleanup>>,
) -> Result<Running, String> {
    let local_ports = if tunnel.direction == Direction::Local {
        tunnel.pairs()?.into_iter().map(|(port, _)| port).collect()
    } else {
        Vec::new()
    };
    let publication = TunnelPublication {
        runtime,
        id,
        live: Arc::new(AtomicBool::new(true)),
    };
    let publish = publication.clone();
    let waiting = previous.clone();
    let (cancel, mut rx) = watch::channel(false);
    let task = tokio::spawn(async move {
        for cleanup in waiting {
            tokio::select! {
                biased;
                _ = rx.changed() => return,
                _ = cleanup.wait() => {},
            }
        }
        let mut attempt: u32 = 0;
        let mut recovery = crate::recovery::subscribe();
        loop {
            if *rx.borrow() {
                break;
            }
            tokio::select! {
                biased;
                _ = rx.changed() => break,
                _ = recovery.ready() => {},
            }
            let connected = tokio::select! {
                _ = rx.changed() => break,
                result = ssh::Forwarding::start(&server, Some(&tunnel)) => result,
            };
            let (message, retryable) = match connected {
                Ok(forwarding) => {
                    publish.state(ConnectionState::new(Status::Connected));
                    let started = Instant::now();
                    let failure = loop {
                        tokio::select! {
                            _ = rx.changed() => break None,
                            _ = tokio::time::sleep(Duration::from_millis(250)) => {
                                if let Some(error) = forwarding.error() { break Some(error); }
                                publish.health(forwarding.destination_health());
                                if started.elapsed() > Duration::from_secs(60) { attempt = 0; }
                            }
                        }
                    };
                    forwarding.shutdown().await;
                    let Some(message) = failure else {
                        break;
                    };
                    (message, true)
                }
                Err(error) => (
                    format!("{error:#}"),
                    ssh::is_transient_connection_error(&error),
                ),
            };
            if *rx.borrow() {
                break;
            }
            if !retryable || !tunnel.auto_reconnect {
                publish.state(ConnectionState::error(message));
                break;
            }
            attempt = attempt.saturating_add(1);
            publish.state(ConnectionState {
                status: Status::Reconnecting,
                error_message: Some(message),
                reconnect_attempt: attempt,
            });
            let delay = (2u64.pow(attempt.min(5))).min(30);
            tokio::select! { biased; _ = rx.changed() => break, _ = recovery.wait(Duration::from_secs(delay)) => {} }
        }
    });
    Ok(Running {
        cancel,
        task,
        publication,
        local_ports,
        previous,
    })
}
pub async fn connection_events(
    state: Shared,
    mut events: tokio::sync::broadcast::Receiver<crate::connectivity::Observation>,
) {
    loop {
        match events.recv().await {
            Ok(event) => {
                if let Ok(owner) = state.owner(event.server.id)
                    && owner.server().same_connection(&event.server)
                    && event.started >= owner.created_at
                {
                    owner.connectivity.lock().unwrap().observe_at(
                        event.started,
                        event.finished,
                        event.finished_wall,
                        event.result,
                    );
                }
            }
            // Probes and future operations reconcile after an overflow.
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        }
    }
}

pub async fn background(state: Shared, app: crate::desktop_integration::Desktop) {
    state.forwarding.auto_connect();
    let profiles = state.sessions.profiles();
    let mut starting = tokio::task::JoinSet::new();
    for (profile, _) in profiles {
        if !profile.clipboard_enabled && !profile.browser_enabled {
            continue;
        }
        let app = app.clone();
        let state = state.clone();
        starting.spawn(async move {
            match state.prepare_integration(profile.id).await {
                Ok(start) => {
                    let _ = start.run(app).await;
                }
                Err(error) => log::warn!("Cannot restore integration for {}: {error}", profile.id),
            }
        });
    }
    while starting.join_next().await.is_some() {}
    let mut timer = tokio::time::interval(Duration::from_secs(30));
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        timer.tick().await;
        let servers = state
            .sessions
            .all()
            .into_iter()
            .filter(|owner| !owner.is_revoked())
            .map(|owner| owner.server().id)
            .collect::<Vec<_>>();
        // Independent probes prevent a slow server from delaying every other server.
        let mut probes = tokio::task::JoinSet::new();
        for server in servers {
            let state = state.clone();
            probes.spawn(async move {
                let _ = crate::commands::probe_connection(&state, server, false).await;
            });
        }
        while probes.join_next().await.is_some() {}
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }
    async fn wait_status(m: &Manager, id: Uuid, status: Status) {
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let current = m
                    .runtime
                    .lock()
                    .unwrap()
                    .tunnels
                    .get(&id)
                    .cloned()
                    .unwrap_or_default();
                if current.status == status {
                    return;
                }
                if current.status == Status::Error && status != Status::Error {
                    panic!("Unexpected SSH error: {:?}", current.error_message);
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("Timed out waiting for {status:?}"));
    }
    #[tokio::test]
    #[ignore = "Run with npm run test:ssh and its ephemeral SSH fixture"]
    async fn real_ssh_forward_reconnect_and_cancel() {
        let dir = tempfile::tempdir().unwrap();
        let server = Server {
            id: Uuid::new_v4(),
            name: "Fixture".into(),
            ssh_user: "fixture".into(),
            ssh_host: "127.0.0.1".into(),
            ssh_port: std::env::var("NEWPORT_TEST_SSH_PORT")
                .unwrap()
                .parse()
                .unwrap(),
            identity_file: Some(std::env::var("NEWPORT_TEST_IDENTITY").unwrap()),
            agent_source: None,
            agent_key_fingerprint: None,
            auth_method: AuthMethod::PublicKey,
            clipboard_enabled: false,
            browser_enabled: false,
            agent_install_approved: true,
            setup_complete: true,
        };
        let port = free_port();
        let tunnel = Tunnel {
            id: Uuid::new_v4(),
            name: "Echo".into(),
            server_id: server.id,
            local_port: port,
            local_port_end: None,
            remote_host: "127.0.0.1".into(),
            remote_port: 12345,
            remote_port_end: None,
            auto_connect: false,
            auto_reconnect: true,
            direction: Default::default(),
            project: None,
        };
        let mut m = Manager::new(Store::for_test(dir.path().into()));
        m.save(Config {
            servers: vec![server.clone()],
            tunnels: vec![tunnel.clone()],
        })
        .await
        .unwrap();
        assert_eq!(
            ssh::execute(&server, "printf ok", None).await.unwrap(),
            "ok"
        );
        let ports =
            crate::ports::parse_ports(&ssh::execute(&server, "discover", None).await.unwrap());
        assert_eq!(ports[0].port, 5432);
        m.connect(tunnel.id).unwrap();
        wait_status(&m, tunnel.id, Status::Connected).await;
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        stream.write_all(b"newport roundtrip").await.unwrap();
        let mut received = [0; 17];
        tokio::time::timeout(Duration::from_secs(3), stream.read_exact(&mut received))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&received, b"newport roundtrip");
        drop(stream);
        // A second profile cannot claim a live local port.
        let mut overlap = tunnel.clone();
        overlap.id = Uuid::new_v4();
        m.config.tunnels.push(overlap.clone());
        assert!(m.connect(overlap.id).unwrap_err().contains("overlap"));
        // The fixture drops every SSH connection on this fixed command.
        let _ = ssh::execute(&server, "drop", None).await;
        wait_status(&m, tunnel.id, Status::Reconnecting).await;
        wait_status(&m, tunnel.id, Status::Connected).await;
        let _ = ssh::execute(&server, "drop", None).await;
        wait_status(&m, tunnel.id, Status::Reconnecting).await;
        m.disconnect(tunnel.id).await;
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert_eq!(
            m.runtime.lock().unwrap().tunnels[&tunnel.id].status,
            Status::Disconnected
        );
        let listener =
            std::net::TcpListener::bind(("127.0.0.1", port)).expect("Stop must release local port");
        drop(listener);
        // A bind failure must never settle in connected state.
        let occupied = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
        m.config.tunnels[0].auto_reconnect = false;
        m.connect(tunnel.id).unwrap();
        wait_status(&m, tunnel.id, Status::Error).await;
        drop(occupied);
        m.shutdown().await;
    }
}

#[cfg(test)]
mod configuration_tests {
    use super::*;
    fn server() -> Server {
        serde_json::from_value(serde_json::json!({"id":Uuid::new_v4(),"name":"Saved","sshHost":"fixture.example","sshUser":"fixture","sshPort":22})).unwrap()
    }
    #[test]
    fn consent_snapshot_is_profile_scoped_and_clears_after_last_wait() {
        let directory = tempfile::tempdir().unwrap();
        let mut manager = Manager::new(Store::for_test(directory.path().into()));
        let server = server();
        manager.config.servers.push(server.clone());
        let before = manager.snapshot();
        let unrelated = crate::agent_approval::Wait::begin(Uuid::new_v4());
        assert_eq!(manager.snapshot().revision, before.revision);
        let first = crate::agent_approval::Wait::begin(server.id);
        let second = crate::agent_approval::Wait::begin(server.id);
        let waiting = manager.snapshot();
        assert_eq!(waiting.runtime.awaiting_agent, vec![server.id]);
        assert!(waiting.revision > before.revision);
        assert_eq!(waiting.runtime.health, before.runtime.health);
        drop(first);
        assert_eq!(manager.snapshot().revision, waiting.revision);
        drop(second);
        let completed = manager.snapshot();
        assert!(completed.runtime.awaiting_agent.is_empty());
        assert!(completed.revision > waiting.revision);
        let _removed = crate::agent_approval::Wait::begin(server.id);
        manager.config.servers.clear();
        assert!(manager.snapshot().runtime.awaiting_agent.is_empty());
        drop(unrelated);
    }
    #[test]
    fn snapshots_are_versioned_and_obsolete_observations_are_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let mut manager = Manager::new(Store::for_test(directory.path().into()));
        let server = server();
        manager.config.servers.push(server.clone());
        let initial = manager.snapshot();
        assert_eq!(initial.revision, manager.snapshot().revision);
        manager.observe_connection(&server, 0, Instant::now(), Ok(()));
        let online = manager.snapshot();
        assert!(online.revision > initial.revision);
        assert_eq!(online.runtime.health[&server.id], "reachable");
        manager.advance_connection_revision(server.id);
        manager.observe_connection(&server, 0, Instant::now(), Ok(()));
        assert_eq!(manager.snapshot().runtime.health[&server.id], "unknown");
        manager.config.servers.clear();
        manager.observe_connection(
            &server,
            manager.connection_revision(server.id),
            Instant::now(),
            Ok(()),
        );
        assert!(!manager.snapshot().runtime.health.contains_key(&server.id));
        let restarted = Manager::new(Store::for_test(directory.path().into()));
        assert_ne!(restarted.snapshot().instance_id, initial.instance_id);
    }
    #[tokio::test]
    async fn history_io_does_not_hold_the_manager_lock() {
        let directory = tempfile::tempdir().unwrap();
        let state = crate::manager::share(Manager::new(Store::for_test(directory.path().into())));
        let _history = history_guard(&state).await;
        tokio::time::timeout(Duration::from_millis(100), async {
            state.lock().await.snapshot();
        })
        .await
        .expect("snapshot must not wait for history I/O");
    }
    #[test]
    fn connection_revisions_keep_metadata_and_reject_obsolete_credentials() {
        let directory = tempfile::tempdir().unwrap();
        let mut manager = Manager::new(Store::for_test(directory.path().into()));
        let original = server();
        manager.config.servers.push(original.clone());
        assert!(manager.matches_connection(&original, 0));
        manager.config.servers[0].name = "Renamed".into();
        manager.config.servers[0].clipboard_enabled = true;
        assert!(manager.matches_connection(&original, 0));
        manager.advance_connection_revision(original.id);
        let revision = manager.connection_revision(original.id);
        assert!(revision > 0);
        assert!(!manager.matches_connection(&original, 0));
        assert!(manager.matches_connection(&original, revision));
        manager.config.servers[0].ssh_host = "new.example.com".into();
        assert!(!manager.matches_connection(&original, revision));
        manager.advance_connection_revision(original.id);
        assert!(manager.connection_revision(original.id) > revision);
    }
    #[tokio::test]
    async fn clipboard_preference_survives_restart_shutdown_and_disable() {
        let directory = tempfile::tempdir().unwrap();
        let mut manager = Manager::new(Store::for_test(directory.path().into()));
        let server = server();
        let id = server.id;
        manager
            .save(Config {
                servers: vec![server],
                tunnels: vec![],
            })
            .await
            .unwrap();
        assert!(manager.config.integration_servers().is_empty());
        manager.remember_clipboard(id, true).await.unwrap();
        manager.shutdown().await;
        let mut reopened = Manager::new(Store::for_test(directory.path().into()));
        assert_eq!(reopened.config.integration_servers(), vec![id]);
        reopened.remember_clipboard(id, false).await.unwrap();
        let reopened = Manager::new(Store::for_test(directory.path().into()));
        assert!(reopened.config.integration_servers().is_empty());
        assert!(reopened.load_error.is_none());
    }
    #[tokio::test]
    async fn revocation_during_profile_storage_fences_the_older_enable_commit() {
        struct Release(Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>);
        impl Drop for Release {
            fn drop(&mut self) {
                *self.0.0.lock().unwrap() = true;
                self.0.1.notify_all();
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let mut manager = Manager::new(Store::for_test(directory.path().into()));
        let mut server = server();
        server.browser_enabled = true;
        let id = server.id;
        manager
            .save(Config {
                servers: vec![server],
                tunnels: vec![],
            })
            .await
            .unwrap();
        let access = manager.integration_access();
        let expected = manager.connection_scope(id).unwrap();
        let owner = manager.sessions.get(id).unwrap();
        let release = Release(Arc::new((
            std::sync::Mutex::new(false),
            std::sync::Condvar::new(),
        )));
        let blocked = release.0.clone();
        let (started, entered) = tokio::sync::oneshot::channel();
        let started = std::sync::Mutex::new(Some(started));
        // The key adapter runs inside the real blocking persistence task. Hold
        // it without a timing assumption or a production-only save test hook.
        manager.store = Arc::new(
            Store::open(directory.path().into(), move |_| {
                started.lock().unwrap().take().unwrap().send(()).unwrap();
                let guard = blocked.0.lock().unwrap();
                drop(blocked.1.wait_while(guard, |released| !*released).unwrap());
                Ok(zeroize::Zeroizing::new(vec![7; 32]))
            })
            .unwrap(),
        );
        let mut saving = Box::pin(manager.remember_clipboard(id, true));
        tokio::select! {
            result = &mut saving => panic!("save must wait for key adapter: {result:?}"),
            result = tokio::time::timeout(Duration::from_secs(3), entered) => result.unwrap().unwrap(),
        }
        let (_, off) = access.begin(id, false, false, &expected).unwrap();
        assert!(!owner.permissions.snapshot().desired.clipboard);
        assert!(!owner.permissions.clipboard_enabled());
        drop(release);
        assert!(saving.await.unwrap_err().contains("changed while saving"));
        assert!(
            manager.config.servers[0].clipboard_enabled,
            "disk save completed, access did not"
        );
        assert!(!owner.permissions.snapshot().desired.clipboard);
        manager.prepare_integration(id).unwrap();
        assert!(!owner.permissions.snapshot().desired.clipboard);
        manager
            .remember_integration_preference(owner.clone(), off)
            .await
            .unwrap();
        assert!(!manager.store.load().unwrap().servers[0].clipboard_enabled);
        assert!(owner.permissions.snapshot().desired.browser);
        assert_eq!(owner.transport_snapshot().demand, 0);
    }

    #[tokio::test]
    async fn saved_profiles_publish_idle_gates_and_replacements_retire_the_previous_identity() {
        let directory = tempfile::tempdir().unwrap();
        let mut manager = Manager::new(Store::for_test(directory.path().into()));
        let server = server();
        let id = server.id;
        manager
            .save(Config {
                servers: vec![server],
                tunnels: vec![],
            })
            .await
            .unwrap();
        let access = manager.integration_access();
        let old = manager.connection_scope(id).unwrap();
        let owner = manager.sessions.get(id).unwrap();
        assert_eq!(owner.transport_snapshot().demand, 0);
        let mut config = manager.config.clone();
        config.servers[0].name = "Renamed".into();
        manager.save(config.clone()).await.unwrap();
        assert_eq!(manager.connection_scope(id).unwrap(), old);
        assert!(Arc::ptr_eq(&manager.sessions.get(id).unwrap(), &owner));
        manager
            .save_with_password(
                config,
                Some((id, zeroize::Zeroizing::new("fixture-password".into()))),
            )
            .await
            .unwrap();
        assert!(owner.is_revoked());
        assert!(access.begin(id, false, false, &old).is_err());
        let current = manager.connection_scope(id).unwrap();
        assert_ne!(current, old);
        let (replacement, _) = access.begin(id, false, false, &current).unwrap();
        assert_eq!(replacement.transport_snapshot().demand, 0);
        manager.save(Config::default()).await.unwrap();
        assert!(replacement.is_revoked());
        assert!(access.begin(id, false, false, &current).is_err());
        assert!(manager.sessions.get(id).is_none());
    }

    #[tokio::test]
    async fn failed_save_does_not_change_clipboard_consent() {
        let directory = tempfile::tempdir().unwrap();
        let mut manager = Manager::new(Store::for_test(directory.path().into()));
        let server = server();
        let id = server.id;
        manager
            .save(Config {
                servers: vec![server],
                tunnels: vec![],
            })
            .await
            .unwrap();
        let vault = directory.path().join("profiles.stronghold");
        let backup = directory.path().join("original.stronghold");
        std::fs::rename(&vault, &backup).unwrap();
        std::fs::create_dir(&vault).unwrap();
        assert!(manager.remember_clipboard(id, true).await.is_err());
        assert!(manager.remember_integration(id, true, true).await.is_err());
        assert!(manager.config.integration_servers().is_empty());
        assert!(
            manager
                .remember_clipboard(Uuid::new_v4(), true)
                .await
                .is_err()
        );
        std::fs::remove_dir(&vault).unwrap();
        std::fs::rename(&backup, &vault).unwrap();
        let reopened = Manager::new(Store::for_test(directory.path().into()));
        assert!(reopened.config.integration_servers().is_empty());
    }
    #[tokio::test]
    async fn failed_disable_revokes_access_and_admission_cannot_replay_saved_consent() {
        let directory = tempfile::tempdir().unwrap();
        let mut manager = Manager::new(Store::for_test(directory.path().into()));
        let mut server = server();
        server.browser_enabled = true;
        server.clipboard_enabled = true;
        let id = server.id;
        manager
            .save(Config {
                servers: vec![server.clone()],
                tunnels: vec![],
            })
            .await
            .unwrap();
        let owner = manager
            .sessions
            .session(&server, manager.connection_revision(server.id));
        let old = owner.permissions.snapshot().desired;
        assert!(
            owner
                .permissions
                .acknowledge(crate::agent::control::Applied {
                    permissions: old,
                    error: None
                })
        );
        let clipboard_ticket = owner.permissions.clipboard_ticket().unwrap();
        let browser_ticket = owner.permissions.browser_ticket().unwrap();
        let vault = directory.path().join("profiles.stronghold");
        let backup = directory.path().join("original.stronghold");
        std::fs::rename(&vault, &backup).unwrap();
        std::fs::create_dir(&vault).unwrap();
        let error = manager.remember_clipboard(id, false).await.unwrap_err();
        assert!(error.contains("Access is disabled for this connection"));
        assert!(
            manager.server(id).unwrap().clipboard_enabled,
            "failed persistence must be reported truthfully"
        );
        assert!(
            owner
                .permissions
                .clipboard_at(clipboard_ticket, || ())
                .is_none()
        );
        assert!(
            owner
                .permissions
                .browser_at(browser_ticket, || ())
                .is_some()
        );
        assert!(
            !owner
                .permissions
                .acknowledge(crate::agent::control::Applied {
                    permissions: old,
                    error: None
                })
        );
        manager.prepare_integration(id).unwrap();
        assert!(!owner.permissions.snapshot().desired.clipboard);
        std::fs::remove_dir(&vault).unwrap();
        std::fs::rename(&backup, &vault).unwrap();
        manager.remember_integration(id, true, false).await.unwrap();
        assert!(
            !owner.permissions.snapshot().desired.clipboard,
            "another preference cannot regrant a failed revocation"
        );
        manager.remember_clipboard(id, false).await.unwrap();
        assert!(
            !Manager::new(Store::for_test(directory.path().into()))
                .server(id)
                .unwrap()
                .clipboard_enabled
        );
        manager.remember_clipboard(id, true).await.unwrap();
        assert!(
            owner.permissions.snapshot().desired.clipboard,
            "only an explicit enable grants access again"
        );
        assert!(
            !owner.permissions.clipboard_enabled(),
            "enabling still requires acknowledgement"
        );
    }
    #[tokio::test]
    async fn integration_preferences_are_independent_and_browser_only_resumes() {
        let directory = tempfile::tempdir().unwrap();
        let mut manager = Manager::new(Store::for_test(directory.path().into()));
        let server = server();
        let id = server.id;
        manager
            .save(Config {
                servers: vec![server],
                tunnels: vec![],
            })
            .await
            .unwrap();
        manager.remember_integration(id, true, true).await.unwrap();
        manager.remember_clipboard(id, true).await.unwrap();
        manager.remember_clipboard(id, false).await.unwrap();
        manager.shutdown().await;
        let mut reopened = Manager::new(Store::for_test(directory.path().into()));
        assert_eq!(reopened.config.integration_servers(), vec![id]);
        assert!(!reopened.server(id).unwrap().clipboard_enabled);
        assert!(reopened.server(id).unwrap().browser_enabled);
        reopened
            .remember_integration(id, true, false)
            .await
            .unwrap();
        let reopened = Manager::new(Store::for_test(directory.path().into()));
        assert!(reopened.config.integration_servers().is_empty());
    }
}
