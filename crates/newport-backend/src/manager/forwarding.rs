//! Forward lifetimes and local-port admission, independent of profile storage.
//! The registry lock covers only publication/admission; no SSH or cleanup awaits.
use super::*;

pub(super) struct Forwarding {
    pub(super) state: Mutex<State>,
    runtime: Arc<Mutex<Runtime>>,
    /// Where each tunnel finds its server's link.
    sessions: Arc<crate::server_session::Registry>,
}
#[derive(Default)]
pub(super) struct State {
    config: Config,
    revisions: HashMap<Uuid, u64>,
    load_error: Option<String>,
    pub(super) running: HashMap<Uuid, Running>,
    stopping: HashMap<Uuid, Arc<TunnelCleanup>>,
    shutting_down: bool,
}
impl Forwarding {
    pub(super) fn new(
        runtime: Arc<Mutex<Runtime>>,
        sessions: Arc<crate::server_session::Registry>,
    ) -> Self {
        Self {
            state: Mutex::default(),
            runtime,
            sessions,
        }
    }

    pub(super) fn publish(
        &self,
        config: &Config,
        revisions: HashMap<Uuid, u64>,
        load_error: Option<String>,
    ) {
        let mut state = self.state.lock().unwrap();
        if state.config == *config && state.revisions == revisions && state.load_error == load_error
        {
            return;
        }
        let changed: Vec<_> = state
            .running
            .keys()
            .copied()
            .filter(|id| {
                let before = state.config.tunnels.iter().find(|t| t.id == *id);
                let after = config.tunnels.iter().find(|t| t.id == *id);
                if before != after {
                    return true;
                }
                let Some(tunnel) = after else {
                    return true;
                };
                let before_server = state
                    .config
                    .servers
                    .iter()
                    .find(|s| s.id == tunnel.server_id);
                let after_server = config.servers.iter().find(|s| s.id == tunnel.server_id);
                !matches!((before_server, after_server), (Some(a), Some(b)) if a.same_connection(b))
                    || state.revisions.get(&tunnel.server_id).copied().unwrap_or(0)
                        != revisions.get(&tunnel.server_id).copied().unwrap_or(0)
            })
            .map(|id| {
                (
                    id,
                    state
                        .running
                        .get(&id)
                        .is_some_and(|r| !r.task.is_finished()),
                )
            })
            .collect();
        state.config = config.clone();
        state.revisions = revisions;
        state.load_error = load_error;
        for (id, _) in &changed {
            state.disconnect(*id, &self.runtime);
        }
        // Reserve replacement listeners only after every obsolete owner retires.
        // A new task waits for all conflicting cleanup generations before SSH.
        for (id, was_active) in changed {
            if was_active
                && state.config.tunnels.iter().any(|t| t.id == id)
                && let Err(error) = state.connect(id, &self.runtime, &self.sessions, false)
            {
                set_tunnel_state(&self.runtime, id, ConnectionState::error(error));
            }
        }
        let mut runtime = self.runtime.lock().unwrap();
        runtime
            .tunnels
            .retain(|id, _| config.tunnels.iter().any(|t| t.id == *id));
        runtime
            .tunnel_health
            .retain(|id, _| config.tunnels.iter().any(|t| t.id == *id));
    }

    pub(super) fn connect(&self, id: Uuid) -> Result<(), String> {
        self.state
            .lock()
            .unwrap()
            .connect(id, &self.runtime, &self.sessions, true)
    }
    pub(super) fn auto_connect(&self) {
        let mut state = self.state.lock().unwrap();
        let ids: Vec<_> = state
            .config
            .tunnels
            .iter()
            .filter(|t| t.auto_connect)
            .map(|t| t.id)
            .collect();
        for id in ids {
            if let Err(error) = state.connect(id, &self.runtime, &self.sessions, false) {
                set_tunnel_state(&self.runtime, id, ConnectionState::error(error));
            }
        }
    }
    pub(super) fn open_endpoint(
        &self,
        instance: Uuid,
        id: Uuid,
        remote_port: Option<u16>,
        expected: Option<&Tunnel>,
        expected_connection: Option<&serde_json::Value>,
    ) -> Result<std::net::SocketAddrV4, String> {
        let state = self.state.lock().unwrap();
        let tunnel = state
            .config
            .tunnels
            .iter()
            .find(|t| t.id == id)
            .ok_or("Tunnel no longer exists")?;
        if expected.is_some_and(|expected| expected != tunnel) {
            return Err("The forward changed before it could be opened.".into());
        }
        state.validate_connection(instance, tunnel.server_id, expected_connection)?;
        let endpoint = newport_domain::forwarding::local_endpoint(tunnel, remote_port)?;
        if !self
            .runtime
            .lock()
            .unwrap()
            .tunnels
            .get(&id)
            .is_some_and(|s| s.status == Status::Connected)
        {
            return Err("Connect the tunnel before opening it.".into());
        }
        Ok(endpoint)
    }
    pub(super) fn connect_discovered(
        &self,
        server: Uuid,
        address: &str,
        port: u16,
    ) -> Result<bool, String> {
        let mut state = self.state.lock().unwrap();
        if !state.config.servers.iter().any(|s| s.id == server) {
            return Err("Server no longer exists.".into());
        }
        let tunnel = newport_domain::forwarding::discovered_tunnel(
            &state.config.tunnels,
            server,
            address,
            port,
        )?;
        if !state.config.tunnels.iter().any(|t| t.id == tunnel.id) {
            return Ok(false);
        }
        state.connect(tunnel.id, &self.runtime, &self.sessions, true)?;
        Ok(true)
    }
    pub(super) fn connect_batch(
        &self,
        instance: Uuid,
        server: Uuid,
        tunnels: &[Tunnel],
        expected: Option<&serde_json::Value>,
    ) -> Result<Vec<Tunnel>, String> {
        let mut state = self.state.lock().unwrap();
        if !state.config.servers.iter().any(|s| s.id == server) {
            return Err("Server no longer exists.".into());
        }
        state.validate_connection(instance, server, expected)?;
        if expected.is_some() {
            for tunnel in tunnels {
                newport_domain::forwarding::validate_update(&state.config.tunnels, tunnel, tunnel)?;
            }
        }
        let resolved = newport_domain::forwarding_batch::resolve_batch(
            server,
            &[],
            tunnels,
            &state.config.tunnels,
        )?;
        for id in &resolved.connect {
            state.connect(*id, &self.runtime, &self.sessions, true)?;
        }
        Ok(resolved
            .connect
            .iter()
            .filter_map(|id| state.config.tunnels.iter().find(|t| t.id == *id).cloned())
            .collect())
    }
    pub(super) fn disconnect(&self, id: Uuid) -> Option<Arc<TunnelCleanup>> {
        self.state.lock().unwrap().disconnect(id, &self.runtime)
    }
    pub(super) fn cleanup(&self, id: Uuid) -> Option<Arc<TunnelCleanup>> {
        self.state.lock().unwrap().stopping.get(&id).cloned()
    }
    pub(super) fn active(&self, server: Uuid) -> Vec<Uuid> {
        let state = self.state.lock().unwrap();
        state
            .config
            .tunnels
            .iter()
            .filter(|t| {
                t.server_id == server
                    && state
                        .running
                        .get(&t.id)
                        .is_some_and(|r| !r.task.is_finished())
            })
            .map(|t| t.id)
            .collect()
    }
    pub(super) fn shutdown(&self) -> (Vec<Running>, Vec<Arc<TunnelCleanup>>) {
        let mut state = self.state.lock().unwrap();
        state.shutting_down = true;
        let running = std::mem::take(&mut state.running);
        for (id, task) in &running {
            task.retire();
            set_tunnel_state(&self.runtime, *id, ConnectionState::default());
        }
        (
            running.into_values().collect(),
            std::mem::take(&mut state.stopping).into_values().collect(),
        )
    }
}
impl State {
    fn validate_connection(
        &self,
        instance: Uuid,
        server: Uuid,
        expected: Option<&serde_json::Value>,
    ) -> Result<(), String> {
        let Some(expected) = expected else {
            return Ok(());
        };
        if self.load_error.is_some() {
            return Err("Server configuration is unavailable.".into());
        }
        let profile = self
            .config
            .servers
            .iter()
            .find(|s| s.id == server)
            .ok_or("Server no longer exists.")?;
        let scope = newport_domain::connection_scope::connection(
            &serde_json::to_value(profile).map_err(|e| e.to_string())?,
            &serde_json::json!({"instanceId":instance, "runtime":{"connectionRevisions":{server.to_string():self.revisions.get(&server).copied().unwrap_or(0)}}}),
        );
        if scope != *expected {
            return Err(
                "The server connection changed before the forwarding action could finish.".into(),
            );
        }
        Ok(())
    }
    /// `asked` when the person connects the tunnel.
    fn connect(
        &mut self,
        id: Uuid,
        runtime: &Arc<Mutex<Runtime>>,
        sessions: &crate::server_session::Registry,
        asked: bool,
    ) -> Result<(), String> {
        if self.shutting_down {
            return Err("Newport is shutting down.".into());
        }
        if self.load_error.is_some() {
            return Err("Server configuration is unavailable.".into());
        }
        if self.running.get(&id).is_some_and(|r| !r.task.is_finished()) {
            return Ok(());
        }
        let tunnel = self
            .config
            .tunnels
            .iter()
            .find(|t| t.id == id)
            .cloned()
            .ok_or("Tunnel no longer exists.")?;
        let requested = tunnel.pairs()?;
        let server = self
            .config
            .servers
            .iter()
            .find(|s| s.id == tunnel.server_id)
            .cloned()
            .ok_or("Server no longer exists.")?;
        for other in &self.config.tunnels {
            if other.id != id
                && tunnel.direction == Direction::Local
                && other.direction == Direction::Local
                && self
                    .running
                    .get(&other.id)
                    .is_some_and(|r| !r.task.is_finished())
                && other
                    .pairs()?
                    .iter()
                    .any(|(p, _)| requested.iter().any(|(q, _)| p == q))
            {
                return Err(format!(
                    "Local ports overlap with active tunnel '{}'.",
                    other.name
                ));
            }
        }
        self.stopping
            .retain(|_, cleanup| !cleanup.task.is_finished());
        let previous = self
            .stopping
            .iter()
            .filter(|(old, cleanup)| {
                **old == id
                    || (tunnel.direction == Direction::Local
                        && requested
                            .iter()
                            .any(|(port, _)| cleanup.local_ports.contains(port)))
            })
            .map(|(_, cleanup)| cleanup.clone())
            .collect();
        let owner = sessions.session(
            &server,
            self.revisions.get(&server.id).copied().unwrap_or(0),
        );
        set_tunnel_state(runtime, id, ConnectionState::new(Status::Connecting));
        self.running.insert(
            id,
            spawn(owner, tunnel, id, runtime.clone(), previous, asked)?,
        );
        Ok(())
    }
    fn disconnect(
        &mut self,
        id: Uuid,
        runtime: &Arc<Mutex<Runtime>>,
    ) -> Option<Arc<TunnelCleanup>> {
        self.stopping
            .retain(|_, cleanup| !cleanup.task.is_finished());
        if let Some(running) = self.running.remove(&id) {
            self.stopping.insert(id, TunnelCleanup::start(running));
        }
        if self.config.tunnels.iter().any(|t| t.id == id) {
            set_tunnel_state(runtime, id, ConnectionState::default());
        } else {
            let mut runtime = runtime.lock().unwrap();
            runtime.tunnels.remove(&id);
            runtime.tunnel_health.remove(&id);
        }
        self.stopping.get(&id).cloned()
    }
}
