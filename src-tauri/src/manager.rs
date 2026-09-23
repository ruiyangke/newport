use crate::{config::Store, model::*, ssh};
use serde::Serialize;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{sync::watch, task::JoinHandle};
use uuid::Uuid;

#[derive(Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Runtime {
    pub tunnels: HashMap<Uuid, ConnectionState>,
    pub tunnel_health: HashMap<Uuid, Vec<DestinationHealth>>,
    pub clipboard: HashMap<Uuid, ConnectionState>,
    pub clipboard_messages: HashMap<Uuid, String>,
    pub clipboard_path_needed: HashMap<Uuid, bool>,
    pub health: HashMap<Uuid, String>,
    pub connectivity: HashMap<Uuid, crate::connectivity::Health>,
    pub connection_revisions: HashMap<Uuid, u64>,
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
pub type Shared = Arc<tokio::sync::Mutex<Manager>>;
// Acquire before the manager lock. History I/O must not block snapshots or unrelated commands.
pub async fn history_guard(state: &Shared) -> tokio::sync::OwnedMutexGuard<()> {
    let barrier = state.lock().await.history_barrier.clone();
    barrier.lock_owned().await
}
pub struct Manager {
    pub store: Arc<Store>,
    pub config: Config,
    pub runtime: Arc<Mutex<Runtime>>,
    pub load_error: Option<String>,
    pub history_barrier: Arc<tokio::sync::Mutex<()>>,
    tunnels: HashMap<Uuid, Running>,
    clipboard: HashMap<Uuid, Running>,
    next_connection_revision: u64,
    connectivity: Mutex<HashMap<Uuid, crate::connectivity::Connectivity>>,
    version: Mutex<(u64, serde_json::Value)>,
    instance_id: Uuid,
    connection_changed_at: HashMap<Uuid, Instant>,
}
struct Running {
    cancel: watch::Sender<bool>,
    task: JoinHandle<()>,
}
impl Running {
    async fn stop(self) {
        let _ = self.cancel.send(true);
        let _ = self.task.await;
    }
}
impl Manager {
    pub fn new(store: Store) -> Self {
        let (config, load_error) = match store.load() {
            Ok(c) => (c, None),
            Err(e) => (Config::default(), Some(e)),
        };
        Self {
            store: Arc::new(store),
            config,
            load_error,
            runtime: Arc::new(Mutex::new(Runtime::default())),
            tunnels: HashMap::new(),
            clipboard: HashMap::new(),
            history_barrier: Arc::default(),
            next_connection_revision: 0,
            connectivity: Mutex::new(HashMap::new()),
            version: Mutex::new((0, serde_json::Value::Null)),
            instance_id: Uuid::new_v4(),
            connection_changed_at: HashMap::new(),
        }
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
    pub fn advance_connection_revision(&mut self, id: Uuid) {
        self.connection_changed_at.insert(id, Instant::now());
        self.connectivity.lock().unwrap().remove(&id);
        self.next_connection_revision += 1;
        self.runtime
            .lock()
            .unwrap()
            .connection_revisions
            .insert(id, self.next_connection_revision);
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
            self.connectivity
                .lock()
                .unwrap()
                .entry(server.id)
                .or_default()
                .observe(started, result);
        }
    }
    pub fn snapshot(&self) -> Snapshot {
        let mut runtime = self.runtime.lock().unwrap().clone();
        let mut connectivity = self.connectivity.lock().unwrap();
        connectivity.retain(|id, _| self.config.servers.iter().any(|s| s.id == *id));
        runtime.health.clear();
        for server in &self.config.servers {
            let health = connectivity
                .entry(server.id)
                .or_default()
                .health(Instant::now());
            runtime.health.insert(server.id, health.status.into());
            runtime.connectivity.insert(server.id, health);
        }
        let mut snapshot = Snapshot {
            instance_id: self.instance_id,
            revision: 0,
            config: self.config.clone(),
            runtime,
            load_error: self.load_error.clone(),
        };
        // Object equality ignores map order; only public state changes advance the revision.
        let payload = serde_json::to_value(&snapshot).expect("snapshot serializes");
        let mut version = self.version.lock().unwrap();
        if version.1 != payload {
            version.0 += 1;
            version.1 = payload;
        }
        snapshot.revision = version.0;
        snapshot
    }
    pub async fn save(&mut self, config: Config) -> Result<(), String> {
        self.save_with_password(config, None).await
    }
    pub async fn save_with_password(
        &mut self,
        config: Config,
        password: Option<(Uuid, zeroize::Zeroizing<String>)>,
    ) -> Result<(), String> {
        if let Some(e) = &self.load_error {
            return Err(e.clone());
        }
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
        self.config = config;
        Ok(())
    }
    pub fn server(&self, id: Uuid) -> Result<Server, String> {
        self.config
            .servers
            .iter()
            .find(|s| s.id == id)
            .cloned()
            .ok_or("Server no longer exists.".into())
    }
    pub async fn connect(&mut self, id: Uuid) -> Result<(), String> {
        if self.tunnels.get(&id).is_some_and(|r| !r.task.is_finished()) {
            return Ok(());
        }
        let tunnel = self
            .config
            .tunnels
            .iter()
            .find(|t| t.id == id)
            .cloned()
            .ok_or("Tunnel no longer exists.")?;
        tunnel.pairs()?;
        let server = self.server(tunnel.server_id)?;
        // Check overlaps against active tunnels before binding listeners.
        let requested = tunnel.pairs()?;
        for t in &self.config.tunnels {
            if t.id != id
                && self
                    .tunnels
                    .get(&t.id)
                    .is_some_and(|r| !r.task.is_finished())
                && t.pairs()?
                    .iter()
                    .any(|(p, _)| requested.iter().any(|(q, _)| p == q))
            {
                return Err(format!(
                    "Local ports overlap with active tunnel '{}'.",
                    t.name
                ));
            }
        }
        self.set_state(id, false, ConnectionState::new(Status::Connecting));
        let running = spawn(server, Some(tunnel), id, self.runtime.clone())?;
        self.tunnels.insert(id, running);
        Ok(())
    }
    pub async fn disconnect(&mut self, id: Uuid) {
        if let Some(r) = self.tunnels.remove(&id) {
            r.stop().await;
        }
        self.set_state(id, false, ConnectionState::default());
    }
    pub async fn start_integration(
        &mut self,
        id: Uuid,
        app: tauri::AppHandle,
    ) -> Result<(), String> {
        if self
            .clipboard
            .get(&id)
            .is_some_and(|r| !r.task.is_finished())
        {
            return Ok(());
        }
        let server = self.server(id)?;
        if !server.clipboard_enabled && !server.browser_enabled {
            return Ok(());
        }
        let client = crate::clipboard::client_identity(&self.store.directory, id)?;
        self.set_state(id, true, ConnectionState::new(Status::Connecting));
        self.runtime.lock().unwrap().clipboard_messages.remove(&id);
        let runtime = self.runtime.clone();
        let (cancel, rx) = watch::channel(false);
        let task = tokio::spawn(async move {
            let result = crate::clipboard::run(
                server,
                client,
                app,
                rx,
                |message, path_needed| {
                    runtime
                        .lock()
                        .unwrap()
                        .clipboard_messages
                        .insert(id, message);
                    runtime
                        .lock()
                        .unwrap()
                        .clipboard_path_needed
                        .insert(id, path_needed);
                    set_state(&runtime, id, true, ConnectionState::new(Status::Connected));
                },
                || {
                    set_state(
                        &runtime,
                        id,
                        true,
                        ConnectionState::new(Status::Reconnecting),
                    );
                },
            )
            .await;
            set_state(
                &runtime,
                id,
                true,
                match result {
                    Ok(()) => ConnectionState::default(),
                    Err(error) => ConnectionState::error(error),
                },
            );
        });
        let running = Running { cancel, task };
        self.clipboard.insert(id, running);
        Ok(())
    }
    pub async fn stop_integration(&mut self, id: Uuid) {
        if let Some(r) = self.clipboard.remove(&id) {
            r.stop().await;
        } else {
            self.set_state(id, true, ConnectionState::default());
        }
        self.runtime.lock().unwrap().clipboard_messages.remove(&id);
    }
    pub fn active_tunnels(&self, server: Uuid) -> Vec<Uuid> {
        self.config
            .tunnels
            .iter()
            .filter(|t| {
                t.server_id == server
                    && self
                        .tunnels
                        .get(&t.id)
                        .is_some_and(|r| !r.task.is_finished())
            })
            .map(|t| t.id)
            .collect()
    }
    pub async fn remember_clipboard(&mut self, id: Uuid, enabled: bool) -> Result<(), String> {
        self.remember_integration(id, false, enabled).await
    }
    pub async fn remember_integration(
        &mut self,
        id: Uuid,
        browser: bool,
        enabled: bool,
    ) -> Result<(), String> {
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
            return Ok(());
        }
        *preference = enabled;
        self.save(config).await
    }
    pub async fn shutdown(&mut self) {
        let ids: Vec<_> = self.tunnels.keys().copied().collect();
        for id in ids {
            self.disconnect(id).await;
        }
        let ids: Vec<_> = self.clipboard.keys().copied().collect();
        for id in ids {
            self.stop_integration(id).await;
        }
    }
    fn set_state(&self, id: Uuid, clipboard: bool, state: ConnectionState) {
        set_state(&self.runtime, id, clipboard, state);
    }
}
fn set_state(runtime: &Arc<Mutex<Runtime>>, id: Uuid, clipboard: bool, state: ConnectionState) {
    let mut r = runtime.lock().unwrap();
    if clipboard {
        r.clipboard.insert(id, state);
    } else {
        if state.status != Status::Connected {
            r.tunnel_health.remove(&id);
        }
        r.tunnels.insert(id, state);
    }
}
fn spawn(
    server: Server,
    tunnel: Option<Tunnel>,
    id: Uuid,
    runtime: Arc<Mutex<Runtime>>,
) -> Result<Running, String> {
    let (cancel, mut rx) = watch::channel(false);
    let task = tokio::spawn(async move {
        let clipboard = false;
        let mut attempt = 0;
        loop {
            if *rx.borrow() {
                break;
            }
            let connected = tokio::select! {
                _ = rx.changed() => break,
                result = ssh::Forwarding::start(&server, tunnel.as_ref()) => result,
            };
            let (message, retryable) = match connected {
                Ok(forwarding) => {
                    set_state(
                        &runtime,
                        id,
                        clipboard,
                        ConnectionState::new(Status::Connected),
                    );
                    let started = Instant::now();
                    let failure = loop {
                        tokio::select! {
                            _ = rx.changed() => break None,
                            _ = tokio::time::sleep(Duration::from_millis(250)) => {
                                if let Some(error) = forwarding.error() { break Some(error); }
                                runtime.lock().unwrap().tunnel_health.insert(id, forwarding.destination_health());
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
            if !retryable || !tunnel.as_ref().is_some_and(|t| t.auto_reconnect) || attempt >= 10 {
                set_state(
                    &runtime,
                    id,
                    clipboard,
                    ConnectionState::error(if attempt >= 10 {
                        format!("Reconnect limit reached. {message}")
                    } else {
                        message
                    }),
                );
                break;
            }
            attempt += 1;
            set_state(
                &runtime,
                id,
                clipboard,
                ConnectionState {
                    status: Status::Reconnecting,
                    error_message: Some(message),
                    reconnect_attempt: attempt,
                },
            );
            let delay = (2u64.pow(attempt.min(7))).min(120);
            tokio::select! { _ = rx.changed() => break, _ = tokio::time::sleep(Duration::from_secs(delay)) => {} }
        }
    });
    Ok(Running { cancel, task })
}
pub async fn connection_events(
    state: Shared,
    mut events: tokio::sync::broadcast::Receiver<crate::connectivity::Observation>,
) {
    loop {
        match events.recv().await {
            Ok(event) => {
                let m = state.lock().await;
                if m.connection_changed_at
                    .get(&event.server.id)
                    .is_some_and(|at| *at > event.started)
                {
                    continue;
                }
                if m.matches_connection(&event.server, m.connection_revision(event.server.id)) {
                    m.connectivity
                        .lock()
                        .unwrap()
                        .entry(event.server.id)
                        .or_default()
                        .observe_at(
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

pub async fn background(state: Shared, app: tauri::AppHandle) {
    {
        let mut m = state.lock().await;
        let clipboard_ids = m.config.integration_servers();
        log::info!(
            "Restoring clipboard sharing for {} saved profiles",
            clipboard_ids.len()
        );
        for id in clipboard_ids {
            if let Err(error) = m.start_integration(id, app.clone()).await {
                m.set_state(id, true, ConnectionState::error(error));
            }
        }
        let ids: Vec<_> = m
            .config
            .tunnels
            .iter()
            .filter(|t| t.auto_connect)
            .map(|t| t.id)
            .collect();
        for id in ids {
            if let Err(e) = m.connect(id).await {
                m.set_state(id, false, ConnectionState::error(e));
            }
        }
    }
    let mut timer = tokio::time::interval(Duration::from_secs(30));
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        timer.tick().await;
        let servers = {
            let m = state.lock().await;
            m.config
                .servers
                .iter()
                .map(|s| (s.clone(), m.connection_revision(s.id)))
                .collect::<Vec<_>>()
        };
        // Independent probes prevent a slow server from delaying every other server.
        let mut probes = tokio::task::JoinSet::new();
        for (server, revision) in servers {
            let state = state.clone();
            probes.spawn(async move {
                let started = Instant::now();
                let result = ssh::execute(&server, "printf ok", None).await.map(|_| ());
                state
                    .lock()
                    .await
                    .observe_connection(&server, revision, started, result);
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
    #[ignore = "Run with scripts/test-ssh.sh and its ephemeral SSH fixture"]
    async fn real_ssh_forward_reconnect_and_cancel() {
        let dir = tempfile::tempdir().unwrap();
        let server = Server {
            id: Uuid::new_v4(),
            name: "Fixture".into(),
            ssh_user: "fixture".into(),
            ssh_host: "127.0.0.1".into(),
            ssh_port: std::env::var("PORTHOP_TEST_SSH_PORT")
                .unwrap()
                .parse()
                .unwrap(),
            identity_file: Some(std::env::var("PORTHOP_TEST_IDENTITY").unwrap()),
            agent_source: None,
            agent_key_fingerprint: None,
            auth_method: AuthMethod::PublicKey,
            clipboard_enabled: false,
            browser_enabled: false,
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
        m.connect(tunnel.id).await.unwrap();
        wait_status(&m, tunnel.id, Status::Connected).await;
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        stream.write_all(b"porthop roundtrip").await.unwrap();
        let mut received = [0; 17];
        tokio::time::timeout(Duration::from_secs(3), stream.read_exact(&mut received))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&received, b"porthop roundtrip");
        drop(stream);
        // A second profile cannot claim a live local port.
        let mut overlap = tunnel.clone();
        overlap.id = Uuid::new_v4();
        m.config.tunnels.push(overlap.clone());
        assert!(m.connect(overlap.id).await.unwrap_err().contains("overlap"));
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
        m.connect(tunnel.id).await.unwrap();
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
        let state = Arc::new(tokio::sync::Mutex::new(Manager::new(Store::for_test(
            directory.path().into(),
        ))));
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
        assert!(manager
            .remember_clipboard(Uuid::new_v4(), true)
            .await
            .is_err());
        std::fs::remove_dir(&vault).unwrap();
        std::fs::rename(&backup, &vault).unwrap();
        let reopened = Manager::new(Store::for_test(directory.path().into()));
        assert!(reopened.config.integration_servers().is_empty());
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
