//! Presentation of backend tunnel health.
use serde_json::Value;

pub fn status(runtime: &Value, id: &str) -> String {
    status_for(runtime, id, None)
}

/// A discovered row represents one destination; a saved range represents all.
/// Transport failures apply to the whole forward regardless of destination.
pub fn status_for(runtime: &Value, id: &str, remote_port: Option<u64>) -> String {
    let state = &runtime["tunnels"][id];
    let status = state["status"].as_str().unwrap_or("disconnected");
    if let Some(error) = state["errorMessage"].as_str().filter(|s| !s.is_empty()) {
        return format!("{status} · {error}");
    }
    if status == "connected" {
        if let Some(health) = runtime["tunnelHealth"][id].as_array() {
            let health: Vec<_> = health
                .iter()
                .filter(|h| remote_port.is_none_or(|port| h["remotePort"].as_u64() == Some(port)))
                .collect();
            if let Some(issue) = health
                .iter()
                .find(|h| matches!(h["status"].as_str(), Some("unavailable" | "blocked")))
            {
                // The row's status already says the destination is down: say why.
                return issue["message"]
                    .as_str()
                    .filter(|message| !message.is_empty())
                    .map(str::to_owned)
                    .unwrap_or_else(|| {
                        format!("Destination {}", issue["status"].as_str().unwrap())
                    });
            }
            if !health.is_empty() && health.iter().all(|h| h["status"] == "reachable") {
                return "Connected · destination reachable".into();
            }
        }
        return "Connected · destination not confirmed".into();
    }
    status.into()
}

#[cfg(test)]
mod tests {
    use super::{status, status_for};
    use serde_json::json;
    #[test]
    fn connected_transport_does_not_claim_destination_is_up() {
        let mut runtime = json!({"tunnels":{"t":{"status":"connected"}},"tunnelHealth":{"t":[{"remotePort":8080,"status":"unavailable","message":"Connection refused"}]}});
        assert_eq!(status(&runtime, "t"), "Connection refused");
        runtime["tunnelHealth"]["t"][0]["message"] = json!(null);
        assert_eq!(status(&runtime, "t"), "Destination unavailable");
        assert!(
            status(&json!({"tunnels":{"t":{"status":"connected"}}}), "t").contains("not confirmed")
        );
    }

    #[test]
    fn status_scopes_range_health_to_the_displayed_destination() {
        let runtime = json!({
            "tunnels":{"t":{"status":"connected"}},
            "tunnelHealth":{"t":[
                {"remotePort":8079,"status":"unavailable","message":"Connection refused"},
                {"remotePort":8080,"status":"reachable"},
                {"remotePort":8081,"status":"checking"}
            ]}
        });
        assert_eq!(
            status_for(&runtime, "t", Some(8080)),
            "Connected · destination reachable"
        );
        assert_eq!(status_for(&runtime, "t", Some(8079)), "Connection refused");
        assert!(status_for(&runtime, "t", Some(8081)).contains("not confirmed"));
        assert!(status_for(&runtime, "t", Some(8082)).contains("not confirmed"));
        assert_eq!(status(&runtime, "t"), "Connection refused");
    }

    #[test]
    fn transport_failures_are_not_scoped_and_disconnected_health_is_stale() {
        let mut runtime = json!({
            "tunnels":{"t":{"status":"reconnecting","errorMessage":"SSH connection lost"}},
            "tunnelHealth":{"t":[{"remotePort":8080,"status":"reachable"}]}
        });
        assert_eq!(
            status_for(&runtime, "t", Some(8080)),
            "reconnecting · SSH connection lost"
        );
        runtime["tunnels"]["t"] = json!({"status":"error"});
        assert_eq!(status_for(&runtime, "t", Some(8080)), "error");
        runtime["tunnels"]["t"] = json!({"status":"disconnected"});
        assert_eq!(status_for(&runtime, "t", Some(8080)), "disconnected");
    }
}

/// Display the listening endpoint first, followed by the destination.
pub fn route(tunnel: &Value) -> String {
    let range = |start: &str, end: &str| {
        let first = tunnel[start].as_u64().unwrap_or_default();
        match tunnel[end].as_u64().filter(|last| *last != first) {
            Some(last) => format!("{first}–{last}"),
            None => first.to_string(),
        }
    };
    let host = tunnel["remoteHost"].as_str().unwrap_or("");
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    let local = format!("localhost:{}", range("localPort", "localPortEnd"));
    let remote = format!("{host}:{}", range("remotePort", "remotePortEnd"));
    if tunnel["direction"] == "remote" {
        format!("{remote} → {local}")
    } else {
        format!("{local} → {remote}")
    }
}
#[cfg(test)]
mod route_tests {
    use super::route;
    use serde_json::json;
    #[test]
    fn route_preserves_direction_ranges_and_ipv6() {
        let mut tunnel = json!({"localPort":8000,"localPortEnd":8002,"remotePort":9000,"remotePortEnd":9002,"remoteHost":"::1"});
        assert_eq!(route(&tunnel), "localhost:8000–8002 → [::1]:9000–9002");
        tunnel["direction"] = json!("remote");
        assert_eq!(route(&tunnel), "[::1]:9000–9002 → localhost:8000–8002");
    }
}
