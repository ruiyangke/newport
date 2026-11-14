//! Read-only saved-forward projection for a container port chip.
//! Request dispatch, connection ownership, and browser opening live elsewhere.
use newport_domain::{
    containers::forwardable,
    forwarding::{host, local_endpoint},
    model::{Status, Tunnel},
};
use newport_protocol::{desktop::configuration, monitor::containers::ContainerPort};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq)]
pub struct ForwardedPort {
    pub tunnel: Tunnel,
    /// The exact local member of a saved range that reaches the published port.
    pub local_port: u16,
    pub status: Status,
    pub error: Option<String>,
}

/// Retain parsed forwards across host ticks and project individual port chips
/// without deserializing the saved configuration for each chip.
#[derive(Default)]
pub struct PortForwards {
    source: Option<Source>,
    forwards: Vec<CachedForward>,
    #[cfg(test)]
    revision: usize,
}

struct Source {
    available: bool,
    server: String,
    tunnels: Vec<configuration::Tunnel>,
    runtime: Vec<configuration::ConnectionRow>,
    load_error: Option<String>,
}

struct CachedForward {
    tunnel: Tunnel,
    address: String,
    state: (Status, Option<String>),
}

impl PortForwards {
    /// Compare borrowed inputs first. Changes outside these fields, including
    /// server statistics and unrelated configuration, do not rebuild the cache.
    pub fn update(&mut self, snapshot: Option<&configuration::Snapshot>, server: &str) {
        let tunnels = snapshot.map_or(&[][..], |value| &value.config.tunnels);
        let runtime = snapshot.map_or(&[][..], |value| &value.runtime.tunnels);
        let load_error = snapshot.and_then(|value| value.load_error.as_ref());
        if self.source.as_ref().is_some_and(|source| {
            source.available == snapshot.is_some()
                && source.server == server
                && source.tunnels == tunnels
                && source.runtime == runtime
                && source.load_error.as_ref() == load_error
        }) {
            return;
        }
        self.source = Some(Source {
            available: snapshot.is_some(),
            server: server.to_owned(),
            tunnels: tunnels.to_vec(),
            runtime: runtime.to_vec(),
            load_error: load_error.cloned(),
        });
        self.forwards = self
            .source
            .as_ref()
            .and_then(Source::parse)
            .unwrap_or_default();
        #[cfg(test)]
        {
            self.revision += 1;
        }
    }

    /// Project the first saved local forward reaching this TCP binding. A UDP
    /// or private-interface chip never inherits an HTTP action from a tunnel.
    pub fn get(&self, port: &ContainerPort) -> Option<ForwardedPort> {
        if !forwardable(port) {
            return None;
        }
        let remote_port = port.host_port?;
        let address = host(
            port.ip
                .as_deref()
                .filter(|address| !address.is_empty())
                .unwrap_or("0.0.0.0"),
        );
        let saved = self.forwards.iter().find(|saved| {
            saved.address == address
                && (saved.tunnel.remote_port
                    ..=saved
                        .tunnel
                        .remote_port_end
                        .unwrap_or(saved.tunnel.remote_port))
                    .contains(&remote_port)
        })?;
        if saved.tunnel.id.is_nil() {
            return None;
        }
        let local_port = local_endpoint(&saved.tunnel, Some(remote_port))
            .ok()?
            .port();
        let (status, error) = &saved.state;
        Some(ForwardedPort {
            tunnel: saved.tunnel.clone(),
            local_port,
            status: *status,
            error: error.clone(),
        })
    }
}

impl Source {
    fn parse(&self) -> Option<Vec<CachedForward>> {
        if !self.available || self.load_error.is_some() {
            return None;
        }
        let server = Uuid::parse_str(&self.server)
            .ok()
            .filter(|id| !id.is_nil())?;
        Some(
            self.tunnels
                .iter()
                .filter(|tunnel| {
                    tunnel.server_id == server
                        && tunnel.direction == configuration::Direction::Local
                })
                .map(|tunnel| {
                    let state = runtime_state(
                        self.runtime
                            .iter()
                            .find(|row| row.id == tunnel.id)
                            .map(|row| &row.value),
                    );
                    let tunnel = Tunnel::from(tunnel);
                    CachedForward {
                        address: host(&tunnel.remote_host),
                        tunnel,
                        state,
                    }
                })
                .collect(),
        )
    }
}

fn runtime_state(runtime: Option<&configuration::ConnectionState>) -> (Status, Option<String>) {
    let Some(runtime) = runtime else {
        return (Status::Disconnected, None);
    };
    let status = match runtime.status {
        configuration::ConnectionStatus::Disconnected => Status::Disconnected,
        configuration::ConnectionStatus::Connecting => Status::Connecting,
        configuration::ConnectionStatus::Connected => Status::Connected,
        configuration::ConnectionStatus::Reconnecting => Status::Reconnecting,
        configuration::ConnectionStatus::Error => Status::Error,
    };
    (status, runtime.error_message.clone())
}

/// Convenience projection for one-off callers. Retain `PortForwards` when
/// projecting multiple chips or updating from a recurring host tick.
pub fn port_forward(
    snapshot: &configuration::Snapshot,
    server: &str,
    port: &ContainerPort,
) -> Option<ForwardedPort> {
    let mut forwards = PortForwards::default();
    forwards.update(Some(snapshot), server);
    forwards.get(port)
}

#[cfg(test)]
mod tests {
    use super::*;
    use newport_domain::{forwarding::discovered_tunnel, model::Direction};
    use newport_protocol::monitor::containers::PortProtocol;

    fn port(ip: &str, host: u16) -> ContainerPort {
        ContainerPort {
            ip: Some(ip.into()),
            host_port: Some(host),
            container_port: 80,
            protocol: PortProtocol::Tcp,
        }
    }
    fn snapshot(tunnels: &[Tunnel]) -> configuration::Snapshot {
        configuration::Snapshot {
            instance_id: Uuid::new_v4(),
            revision: 1,
            config: configuration::Config {
                servers: Vec::new(),
                tunnels: tunnels.iter().map(configuration::Tunnel::from).collect(),
            },
            runtime: configuration::Runtime {
                agent_transport: Vec::new(),
                agent_runtime: Vec::new(),
                agent_permissions: Vec::new(),
                agent_status: Vec::new(),
                agent_installation: Vec::new(),
                tunnels: Vec::new(),
                tunnel_health: Vec::new(),
                clipboard: Vec::new(),
                clipboard_messages: Vec::new(),
                connection_revisions: Vec::new(),
                awaiting_agent: Vec::new(),
            },
            load_error: None,
        }
    }
    fn runtime(
        snapshot: &mut configuration::Snapshot,
        id: Uuid,
        status: configuration::ConnectionStatus,
        error: Option<&str>,
    ) {
        let rows = &mut snapshot.runtime.tunnels;
        rows.retain(|row| row.id != id);
        rows.push(configuration::ConnectionRow {
            id,
            value: configuration::ConnectionState {
                status,
                error_message: error.map(Into::into),
                reconnect_attempt: 0,
            },
        });
    }
    use configuration::ConnectionStatus as Connection;

    #[test]
    fn saved_forward_projects_exact_range_member_and_runtime_without_mutating_snapshot() {
        let server = Uuid::new_v4();
        let mut tunnel = discovered_tunnel(&[], server, "localhost", 8000).unwrap();
        tunnel.local_port = 9000;
        tunnel.local_port_end = Some(9005);
        tunnel.remote_port_end = Some(8005);
        let mut value = snapshot(std::slice::from_ref(&tunnel));
        runtime(&mut value, tunnel.id, Connection::Connected, None);
        let before = value.clone();
        let projection = port_forward(&value, &server.to_string(), &port("0.0.0.0", 8003)).unwrap();
        assert_eq!(projection.tunnel, tunnel);
        assert_eq!(projection.local_port, 9003);
        assert_eq!(projection.status, Status::Connected);
        assert_eq!(projection.error, None);
        assert_eq!(value, before);
        assert!(port_forward(&value, &server.to_string(), &port("0.0.0.0", 8006)).is_none());
        runtime(
            &mut value,
            tunnel.id,
            Connection::Error,
            Some("destination unavailable"),
        );
        let projection =
            port_forward(&value, &server.to_string(), &port("127.0.0.1", 8005)).unwrap();
        assert_eq!(projection.local_port, 9005);
        assert_eq!(projection.status, Status::Error);
        assert_eq!(projection.error.as_deref(), Some("destination unavailable"));
    }

    #[test]
    fn ipv6_matching_preserves_family_server_and_direction() {
        let server = Uuid::new_v4();
        let ipv4 = discovered_tunnel(&[], server, "localhost", 8080).unwrap();
        let mut ipv6 = discovered_tunnel(std::slice::from_ref(&ipv4), server, "::", 8080).unwrap();
        ipv6.local_port = 9080;
        let mut reverse = ipv6.clone();
        reverse.id = Uuid::new_v4();
        reverse.direction = Direction::Remote;
        let mut foreign = ipv6.clone();
        foreign.id = Uuid::new_v4();
        foreign.server_id = Uuid::new_v4();
        let value = snapshot(&[ipv4, reverse, foreign, ipv6.clone()]);
        let projection =
            port_forward(&value, &server.to_string(), &port("0:0:0:0:0:0:0:0", 8080)).unwrap();
        assert_eq!(projection.tunnel.id, ipv6.id);
        assert_eq!(projection.local_port, 9080);
        assert_eq!(projection.status, Status::Disconnected);
        assert!(port_forward(&value, &Uuid::new_v4().to_string(), &port("::1", 8080)).is_none());
        assert!(port_forward(&value, "not-a-server", &port("::1", 8080)).is_none());
    }

    #[test]
    fn connected_saved_tunnels_never_make_udp_internal_or_private_bindings_actionable() {
        let server = Uuid::new_v4();
        let tunnel = discovered_tunnel(&[], server, "0.0.0.0", 8080).unwrap();
        let mut value = snapshot(std::slice::from_ref(&tunnel));
        runtime(&mut value, tunnel.id, Connection::Connected, None);
        let mut udp = port("0.0.0.0", 8080);
        udp.protocol = PortProtocol::Udp;
        let mut internal = port("0.0.0.0", 8080);
        internal.host_port = None;
        for rejected in [udp, internal, port("10.0.0.2", 8080)] {
            assert!(port_forward(&value, &server.to_string(), &rejected).is_none());
        }
    }

    #[test]
    fn projection_fails_closed_for_invalid_ranges_and_unreadable_configurations() {
        let server = Uuid::new_v4();
        let tunnel = discovered_tunnel(&[], server, "127.0.0.1", 8080).unwrap();
        let binding = port("127.0.0.1", 8080);
        let mut invalid = tunnel.clone();
        invalid.local_port_end = Some(8081);
        assert!(port_forward(&snapshot(&[invalid]), &server.to_string(), &binding).is_none());
        let mut nil = tunnel.clone();
        nil.id = Uuid::nil();
        assert!(port_forward(&snapshot(&[nil]), &server.to_string(), &binding).is_none());
        let mut value = snapshot(std::slice::from_ref(&tunnel));
        value.load_error = Some("configuration could not be read".into());
        assert!(port_forward(&value, &server.to_string(), &binding).is_none());
    }

    #[test]
    fn cache_reuses_parsed_forwards_across_ticks_chips_and_unrelated_changes() {
        let server = Uuid::new_v4();
        let mut tunnel = discovered_tunnel(&[], server, "localhost", 8000).unwrap();
        tunnel.local_port = 9000;
        tunnel.local_port_end = Some(9005);
        tunnel.remote_port_end = Some(8005);
        let mut value = snapshot(std::slice::from_ref(&tunnel));
        runtime(&mut value, tunnel.id, Connection::Connected, None);
        let mut forwards = PortForwards::default();
        assert!(forwards.get(&port("127.0.0.1", 8003)).is_none());
        forwards.update(Some(&value), &server.to_string());
        assert_eq!(forwards.revision, 1);
        for _ in 0..20 {
            forwards.update(Some(&value), &server.to_string());
            for offset in 0..=5 {
                let projected = forwards.get(&port("0.0.0.0", 8000 + offset)).unwrap();
                assert_eq!(projected.local_port, 9000 + offset);
                assert_eq!(projected.status, Status::Connected);
            }
        }
        value.revision += 1;
        value
            .runtime
            .connection_revisions
            .push(configuration::RevisionRow {
                id: Uuid::new_v4(),
                value: 3,
            });
        value.config.servers.push(configuration::Server {
            id: Uuid::new_v4(),
            name: "Renamed server".into(),
            ssh_user: "user".into(),
            ssh_host: "host".into(),
            ssh_port: 22,
            identity_file: None,
            agent_source: None,
            agent_key_fingerprint: None,
            auth_method: configuration::AuthMethod::PublicKey,
            clipboard_enabled: false,
            browser_enabled: false,
            agent_install_approved: false,
            setup_complete: false,
            proxy: None,
        });
        forwards.update(Some(&value), &server.to_string());
        assert_eq!(forwards.revision, 1);
        assert_eq!(
            forwards.get(&port("127.0.0.1", 8003)).unwrap().local_port,
            9003
        );
    }

    #[test]
    fn cache_rebuilds_for_runtime_config_and_server_changes() {
        let server = Uuid::new_v4();
        let tunnel = discovered_tunnel(&[], server, "localhost", 8080).unwrap();
        let mut value = snapshot(std::slice::from_ref(&tunnel));
        let binding = port("127.0.0.1", 8080);
        let mut forwards = PortForwards::default();
        forwards.update(Some(&value), &server.to_string());
        assert_eq!(forwards.get(&binding).unwrap().status, Status::Disconnected);
        runtime(
            &mut value,
            tunnel.id,
            Connection::Error,
            Some("connection refused"),
        );
        forwards.update(Some(&value), &server.to_string());
        assert_eq!(forwards.revision, 2);
        let projected = forwards.get(&binding).unwrap();
        assert_eq!(projected.status, Status::Error);
        assert_eq!(projected.error.as_deref(), Some("connection refused"));
        value.config.tunnels[0].local_port = 9090;
        forwards.update(Some(&value), &server.to_string());
        assert_eq!(forwards.revision, 3);
        assert_eq!(forwards.get(&binding).unwrap().local_port, 9090);
        forwards.update(Some(&value), &Uuid::new_v4().to_string());
        assert_eq!(forwards.revision, 4);
        assert!(forwards.get(&binding).is_none());
        forwards.update(Some(&value), &server.to_string());
        assert_eq!(forwards.revision, 5);
        assert_eq!(forwards.get(&binding).unwrap().local_port, 9090);
    }

    #[test]
    fn cache_drops_projections_on_outage_and_recovers() {
        let server = Uuid::new_v4();
        let tunnel = discovered_tunnel(&[], server, "localhost", 8080).unwrap();
        let mut valid = snapshot(std::slice::from_ref(&tunnel));
        runtime(&mut valid, tunnel.id, Connection::Connected, None);
        let binding = port("127.0.0.1", 8080);
        let mut outage = valid.clone();
        outage.load_error = Some("configuration unavailable".into());
        let mut forwards = PortForwards::default();
        for invalid in [None, Some(outage)] {
            forwards.update(Some(&valid), &server.to_string());
            assert_eq!(forwards.get(&binding).unwrap().status, Status::Connected);
            forwards.update(invalid.as_ref(), &server.to_string());
            assert!(forwards.get(&binding).is_none());
            let revision = forwards.revision;
            forwards.update(invalid.as_ref(), &server.to_string());
            assert_eq!(
                forwards.revision, revision,
                "unchanged bad input must not be reparsed"
            );
        }
        forwards.update(Some(&valid), &server.to_string());
        forwards.update(Some(&valid), "invalid-server");
        assert!(forwards.get(&binding).is_none());
        forwards.update(Some(&valid), &server.to_string());
        assert_eq!(forwards.get(&binding).unwrap().status, Status::Connected);
    }
}
