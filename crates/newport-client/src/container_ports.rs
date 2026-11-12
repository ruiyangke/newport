//! Read-only saved-forward projection for a container port chip.
//! Request dispatch, connection ownership, and browser opening live elsewhere.
use newport_domain::{
    containers::{ContainerPort, forwardable},
    forwarding::{host, local_endpoint},
    model::{Direction, Status, Tunnel},
};
use serde::Deserialize;
use serde_json::Value;
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
    tunnels: Option<Value>,
    runtime: Option<Value>,
    load_error: Option<Value>,
}

struct CachedForward {
    tunnel: Tunnel,
    address: String,
    state: Option<(Status, Option<String>)>,
}

impl PortForwards {
    /// Compare borrowed inputs first. Changes outside these fields, including
    /// server statistics and unrelated configuration, do not rebuild the cache.
    pub fn update(&mut self, snapshot: Option<&Value>, server: &str) {
        let tunnels = snapshot.and_then(|value| value.get("config")?.get("tunnels"));
        let runtime = snapshot.and_then(|value| value.get("runtime")?.get("tunnels"));
        let load_error = snapshot.and_then(|value| value.get("loadError"));
        if self.source.as_ref().is_some_and(|source| {
            source.available == snapshot.is_some()
                && source.server == server
                && source.tunnels.as_ref() == tunnels
                && source.runtime.as_ref() == runtime
                && source.load_error.as_ref() == load_error
        }) {
            return;
        }
        self.source = Some(Source {
            available: snapshot.is_some(),
            server: server.to_owned(),
            tunnels: tunnels.cloned(),
            runtime: runtime.cloned(),
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
        let (status, error) = saved.state.as_ref()?;
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
        if !self.available
            || self
                .load_error
                .as_ref()
                .is_some_and(|value| !value.is_null())
        {
            return None;
        }
        let server = Uuid::parse_str(&self.server)
            .ok()
            .filter(|id| !id.is_nil())?;
        let tunnels = Vec::<Tunnel>::deserialize(self.tunnels.as_ref()?).ok()?;
        let runtime = self.runtime.as_ref()?.as_object()?;
        Some(
            tunnels
                .into_iter()
                .filter(|tunnel| tunnel.server_id == server && tunnel.direction == Direction::Local)
                .map(|tunnel| {
                    let address = host(&tunnel.remote_host);
                    let state =
                        runtime_state(runtime.get(&tunnel.id.to_string()).unwrap_or(&Value::Null));
                    CachedForward {
                        tunnel,
                        address,
                        state,
                    }
                })
                .collect(),
        )
    }
}

fn runtime_state(runtime: &Value) -> Option<(Status, Option<String>)> {
    let status = match runtime.get("status").and_then(Value::as_str) {
        None if runtime.is_null() => Status::Disconnected,
        Some("disconnected") => Status::Disconnected,
        Some("connecting") => Status::Connecting,
        Some("connected") => Status::Connected,
        Some("reconnecting") => Status::Reconnecting,
        Some("error") => Status::Error,
        _ => return None,
    };
    let error = match runtime.get("errorMessage") {
        None | Some(Value::Null) => None,
        Some(Value::String(message)) => Some(message.clone()),
        _ => return None,
    };
    Some((status, error))
}

/// Convenience projection for one-off callers. Retain `PortForwards` when
/// projecting multiple chips or updating from a recurring host tick.
pub fn port_forward(snapshot: &Value, server: &str, port: &ContainerPort) -> Option<ForwardedPort> {
    let mut forwards = PortForwards::default();
    forwards.update(Some(snapshot), server);
    forwards.get(port)
}

#[cfg(test)]
mod tests {
    use super::*;
    use newport_domain::{containers::PortProtocol, forwarding::discovered_tunnel};
    use serde_json::json;

    fn port(ip: &str, host: u16) -> ContainerPort {
        ContainerPort {
            ip: Some(ip.into()),
            host_port: Some(host),
            container_port: 80,
            protocol: PortProtocol::Tcp,
        }
    }
    fn snapshot(tunnels: &[Tunnel]) -> Value {
        json!({"config":{"tunnels":tunnels},"runtime":{"tunnels":{}}})
    }
    fn runtime(snapshot: &mut Value, id: Uuid, status: &str, error: Option<&str>) {
        snapshot["runtime"]["tunnels"][id.to_string()] =
            json!({"status":status,"errorMessage":error,"reconnectAttempt":0});
    }

    #[test]
    fn saved_forward_projects_exact_range_member_and_runtime_without_mutating_snapshot() {
        let server = Uuid::new_v4();
        let mut tunnel = discovered_tunnel(&[], server, "localhost", 8000).unwrap();
        tunnel.local_port = 9000;
        tunnel.local_port_end = Some(9005);
        tunnel.remote_port_end = Some(8005);
        let mut value = snapshot(std::slice::from_ref(&tunnel));
        runtime(&mut value, tunnel.id, "connected", None);
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
            "error",
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
        runtime(&mut value, tunnel.id, "connected", None);
        let mut udp = port("0.0.0.0", 8080);
        udp.protocol = PortProtocol::Udp;
        let mut internal = port("0.0.0.0", 8080);
        internal.host_port = None;
        for rejected in [udp, internal, port("10.0.0.2", 8080)] {
            assert!(port_forward(&value, &server.to_string(), &rejected).is_none());
        }
    }

    #[test]
    fn projection_fails_closed_for_malformed_ranges_configurations_and_runtime() {
        let server = Uuid::new_v4();
        let tunnel = discovered_tunnel(&[], server, "127.0.0.1", 8080).unwrap();
        let binding = port("127.0.0.1", 8080);
        let mut invalid = tunnel.clone();
        invalid.local_port_end = Some(8081);
        assert!(port_forward(&snapshot(&[invalid]), &server.to_string(), &binding).is_none());
        let mut value = snapshot(std::slice::from_ref(&tunnel));
        value["config"]["tunnels"][0]["id"] = json!("bad-id");
        assert!(port_forward(&value, &server.to_string(), &binding).is_none());
        let mut value = snapshot(std::slice::from_ref(&tunnel));
        value["loadError"] = json!("configuration could not be read");
        assert!(port_forward(&value, &server.to_string(), &binding).is_none());
        let mut value = snapshot(std::slice::from_ref(&tunnel));
        runtime(&mut value, tunnel.id, "unknown", None);
        assert!(port_forward(&value, &server.to_string(), &binding).is_none());
        runtime(&mut value, tunnel.id, "connected", None);
        value["runtime"]["tunnels"][tunnel.id.to_string()]["errorMessage"] = json!(42);
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
        runtime(&mut value, tunnel.id, "connected", None);
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
        value["runtime"]["servers"] = json!({"new-server": {"status":"connected"}});
        value["config"]["servers"] = json!([{"name":"Renamed server"}]);
        value["config"]["preferences"] = json!({"theme":"dark"});
        value["statistics"] = json!({"bytes":9000});
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
        runtime(&mut value, tunnel.id, "error", Some("connection refused"));
        forwards.update(Some(&value), &server.to_string());
        assert_eq!(forwards.revision, 2);
        let projected = forwards.get(&binding).unwrap();
        assert_eq!(projected.status, Status::Error);
        assert_eq!(projected.error.as_deref(), Some("connection refused"));
        value["config"]["tunnels"][0]["localPort"] = json!(9090);
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
    fn cache_drops_projections_on_outage_or_malformed_inputs_and_recovers() {
        let server = Uuid::new_v4();
        let tunnel = discovered_tunnel(&[], server, "localhost", 8080).unwrap();
        let mut valid = snapshot(std::slice::from_ref(&tunnel));
        runtime(&mut valid, tunnel.id, "connected", None);
        let binding = port("127.0.0.1", 8080);
        let mut outage = valid.clone();
        outage["loadError"] = json!("configuration unavailable");
        let mut malformed_config = valid.clone();
        malformed_config["config"]["tunnels"][0]["id"] = json!("malformed");
        let mut malformed_runtime = valid.clone();
        malformed_runtime["runtime"]["tunnels"] = json!([]);
        let mut malformed_status = valid.clone();
        runtime(&mut malformed_status, tunnel.id, "unknown", None);
        let mut missing_runtime = valid.clone();
        missing_runtime.as_object_mut().unwrap().remove("runtime");
        let mut forwards = PortForwards::default();
        for invalid in [
            None,
            Some(outage),
            Some(malformed_config),
            Some(malformed_runtime),
            Some(malformed_status),
            Some(missing_runtime),
        ] {
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
