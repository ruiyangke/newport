//! Destination matching and local-port planning.
use serde_json::Value;

/// Check an editor's original value against the authoritative configuration.
/// The backend must repeat this check against the revision committed by save.
/// Runtime connection changes are deliberately not part of the comparison.
pub fn validate_update(
    tunnels: &[crate::model::Tunnel],
    expected: &crate::model::Tunnel,
    replacement: &crate::model::Tunnel,
) -> Result<(), String> {
    if expected.id.is_nil()
        || expected.id != replacement.id
        || expected.server_id != replacement.server_id
    {
        return Err("An edited forward must keep its original identity and server.".into());
    }
    let current = tunnels
        .iter()
        .find(|tunnel| tunnel.id == expected.id)
        .ok_or("This forward was deleted while the editor was open. Close the editor and create a new forward if needed.")?;
    if current != expected {
        return Err("This forward changed while the editor was open. Close and reopen it to review the latest configuration.".into());
    }
    Ok(())
}

/// Resolve the Mac endpoint for one destination in a saved local forward.
/// Omitting the destination preserves the saved-forward action's first port.
/// Connection state is checked separately by the runtime owner before opening.
pub fn local_endpoint(
    tunnel: &crate::model::Tunnel,
    remote_port: Option<u16>,
) -> Result<std::net::SocketAddrV4, String> {
    if tunnel.direction != crate::model::Direction::Local {
        return Err(
            "This forward exposes a Mac port on the server; open it from the server instead."
                .into(),
        );
    }
    let remote_port = remote_port.unwrap_or(tunnel.remote_port);
    let local_port = tunnel
        .pairs()?
        .into_iter()
        .find_map(|(local, remote)| (remote == remote_port).then_some(local))
        .ok_or("The requested port is outside this forward's destination range.")?;
    Ok(std::net::SocketAddrV4::new(
        std::net::Ipv4Addr::LOCALHOST,
        local_port,
    ))
}

pub fn host(address: &str) -> String {
    let value = address.trim_end_matches('.').to_lowercase();
    match value.as_str() {
        "0.0.0.0" | "*" | "localhost" => "127.0.0.1".into(),
        _ => value
            .parse::<std::net::Ipv6Addr>()
            .map(|ip| {
                if ip.is_unspecified() {
                    "::1".into()
                } else {
                    ip.to_string()
                }
            })
            .unwrap_or(value),
    }
}

pub fn reaches(tunnel: &Value, server: &str, port: &Value) -> bool {
    let Some(number) = port["port"].as_u64() else {
        return false;
    };
    let Some(first) = tunnel["remotePort"].as_u64() else {
        return false;
    };
    tunnel["serverId"] == server
        && tunnel["direction"].as_str().unwrap_or("local") == "local"
        && host(tunnel["remoteHost"].as_str().unwrap_or(""))
            == host(port["address"].as_str().unwrap_or(""))
        && number >= first
        && number <= tunnel["remotePortEnd"].as_u64().unwrap_or(first)
}

/// Resolve a discovery against the latest configuration before saving.
/// Rebase on a competing commit; preserve the chosen ID across preparation and
/// final validation when creating a new definition.
pub fn discovered_tunnel(
    tunnels: &[crate::model::Tunnel],
    server_id: uuid::Uuid,
    address: &str,
    port: u16,
) -> Result<crate::model::Tunnel, String> {
    use crate::model::{Direction, Tunnel};
    if port == 0 || address.trim().is_empty() {
        return Err("Invalid discovered destination".into());
    }
    let remote_host = host(address);
    if let Some(existing) = tunnels.iter().find(|t| {
        t.server_id == server_id
            && t.direction == Direction::Local
            && host(&t.remote_host) == remote_host
            && (t.remote_port..=t.remote_port_end.unwrap_or(t.remote_port)).contains(&port)
    }) {
        return Ok(existing.clone());
    }
    let local_port = (port..=u16::MAX)
        .find(|candidate| {
            !tunnels.iter().any(|t| {
                t.direction == Direction::Local
                    && (t.local_port..=t.local_port_end.unwrap_or(t.local_port)).contains(candidate)
            })
        })
        .ok_or("No available local port in the saved configuration")?;
    Ok(Tunnel {
        id: uuid::Uuid::new_v4(),
        server_id,
        name: format!("Port {port}"),
        local_port,
        local_port_end: None,
        remote_host,
        remote_port: port,
        remote_port_end: None,
        auto_connect: false,
        auto_reconnect: true,
        direction: Direction::Local,
        project: None,
    })
}

#[cfg(test)]
mod tests {
    use super::{discovered_tunnel, host, local_endpoint, reaches, validate_update};
    use serde_json::json;

    #[test]
    fn edits_require_an_unchanged_saved_target_without_blocking_unrelated_changes() {
        let original = discovered_tunnel(&[], uuid::Uuid::new_v4(), "127.0.0.1", 3000).unwrap();
        let mut edited = original.clone();
        edited.name = "Renamed application".into();
        edited.remote_port = 3001;
        let mut unrelated =
            discovered_tunnel(&[], uuid::Uuid::new_v4(), "127.0.0.1", 4000).unwrap();
        unrelated.name = "Changed elsewhere".into();
        assert!(validate_update(&[original.clone(), unrelated], &original, &edited).is_ok());
        assert!(validate_update(&[], &original, &edited)
            .unwrap_err()
            .contains("deleted"));
        let mut changed = original.clone();
        changed.auto_reconnect = !changed.auto_reconnect;
        assert!(validate_update(&[changed], &original, &edited)
            .unwrap_err()
            .contains("changed"));
        // A deleted UUID reused on a different server is also a stale target.
        let mut replaced = original.clone();
        replaced.server_id = uuid::Uuid::new_v4();
        assert!(validate_update(&[replaced], &original, &edited).is_err());
    }

    #[test]
    fn edit_precondition_cannot_be_used_for_a_different_identity() {
        let original = discovered_tunnel(&[], uuid::Uuid::new_v4(), "127.0.0.1", 3000).unwrap();
        let mut edited = original.clone();
        edited.id = uuid::Uuid::new_v4();
        assert!(validate_update(std::slice::from_ref(&original), &original, &edited).is_err());
        edited.id = original.id;
        edited.server_id = uuid::Uuid::new_v4();
        assert!(validate_update(std::slice::from_ref(&original), &original, &edited).is_err());
        edited.id = uuid::Uuid::nil();
        assert!(validate_update(std::slice::from_ref(&edited), &edited, &edited).is_err());
    }
    #[test]
    fn matching_respects_destination_range_server_and_direction() {
        let mut tunnel = json!({"serverId":"a", "remoteHost":"localhost.", "remotePort":3000,"remotePortEnd":3005});
        let port = json!({"port":3003,"address":"0.0.0.0"});
        assert!(reaches(&tunnel, "a", &port));
        assert!(!reaches(&tunnel, "b", &port));
        assert!(!reaches(
            &tunnel,
            "a",
            &json!({"port":3003,"address":"10.0.0.5"})
        ));
        tunnel["direction"] = json!("remote");
        assert!(!reaches(&tunnel, "a", &port));
        assert_eq!(host("::"), "::1");
        assert_eq!(host("0:0:0:0:0:0:0:1"), "::1");
    }
    #[test]
    fn discovery_reuses_ranges_and_allocates_across_servers() {
        let server = uuid::Uuid::new_v4();
        let other = uuid::Uuid::new_v4();
        let mut first = discovered_tunnel(&[], server, "0.0.0.0", 3000).unwrap();
        first.remote_port_end = Some(3004);
        first.local_port_end = Some(3004);
        assert_eq!(
            discovered_tunnel(&[first.clone()], server, "localhost", 3003)
                .unwrap()
                .id,
            first.id
        );
        let next = discovered_tunnel(&[first], other, "127.0.0.1", 3000).unwrap();
        assert_eq!(next.local_port, 3005);
        assert_eq!(next.server_id, other);
        let last = discovered_tunnel(&[], server, "::", 65535).unwrap();
        assert_eq!(last.remote_host, "::1");
        assert!(discovered_tunnel(&[last], other, "::", 65535).is_err());
        assert!(discovered_tunnel(&[], server, "127.0.0.1", 0).is_err());
    }

    #[test]
    fn local_endpoint_resolves_the_requested_destination_and_defaults_to_start() {
        let mut tunnel = discovered_tunnel(&[], uuid::Uuid::new_v4(), "127.0.0.1", 3000).unwrap();
        tunnel.local_port = 9000;
        tunnel.local_port_end = Some(9004);
        tunnel.remote_port_end = Some(3004);
        for (remote, local) in [
            (None, 9000),
            (Some(3000), 9000),
            (Some(3003), 9003),
            (Some(3004), 9004),
        ] {
            let endpoint = local_endpoint(&tunnel, remote).unwrap();
            assert_eq!(*endpoint.ip(), std::net::Ipv4Addr::LOCALHOST);
            assert_eq!(endpoint.port(), local);
        }
        for port in [0, 2999, 3005, u16::MAX] {
            assert!(local_endpoint(&tunnel, Some(port)).is_err());
        }
        tunnel.local_port = 65531;
        tunnel.local_port_end = Some(u16::MAX);
        assert_eq!(
            local_endpoint(&tunnel, Some(3004)).unwrap().port(),
            u16::MAX
        );
    }

    #[test]
    fn local_endpoint_rejects_reverse_and_invalid_saved_ranges() {
        let mut tunnel = discovered_tunnel(&[], uuid::Uuid::new_v4(), "127.0.0.1", 3000).unwrap();
        assert_eq!(local_endpoint(&tunnel, None).unwrap().port(), 3000);
        assert!(local_endpoint(&tunnel, Some(3001)).is_err());
        tunnel.direction = crate::model::Direction::Remote;
        assert!(local_endpoint(&tunnel, None).is_err());
        tunnel.direction = crate::model::Direction::Local;
        tunnel.local_port_end = Some(3004);
        assert!(local_endpoint(&tunnel, Some(3000)).is_err());
        tunnel.remote_port_end = Some(2999);
        assert!(local_endpoint(&tunnel, None).is_err());
        tunnel.local_port = 0;
        assert!(local_endpoint(&tunnel, None).is_err());
    }
}

/// Parse an editor port or inclusive range without accepting zero or reversed ranges.
pub fn parse_port_range(value: &str) -> Result<(u16, Option<u16>), String> {
    let mut parts = value.trim().split('-');
    let parse = |part: &str| part.trim().parse::<u16>().ok().filter(|p| *p != 0);
    let start = parts
        .next()
        .and_then(parse)
        .ok_or("Enter a port between 1 and 65535")?;
    let end = parts
        .next()
        .map(|p| parse(p).ok_or("Invalid range end"))
        .transpose()?;
    if parts.next().is_some() || end.is_some_and(|end| end < start) {
        return Err("Enter an ascending port range, such as 8000-8010".into());
    }
    Ok((start, end.filter(|end| *end != start)))
}

#[cfg(test)]
mod range_tests {
    use super::parse_port_range;
    #[test]
    fn validates_full_port_range_input() {
        assert_eq!(
            parse_port_range(" 8000 - 8010 ").unwrap(),
            (8000, Some(8010))
        );
        assert_eq!(parse_port_range("65535").unwrap(), (65535, None));
        for invalid in ["0", "65536", "8-7", "1-2-3", "1-", "-1", ""] {
            assert!(parse_port_range(invalid).is_err(), "{invalid}");
        }
    }
}
