//! Listener discovery and process/container attribution, independent of SSH transport.
use crate::{model::Server, ssh};
use serde::Serialize;
use std::collections::BTreeMap;

pub async fn discover(server: &Server) -> Result<Vec<DiscoveredPort>, String> {
    let output = ssh::execute(server, DISCOVER_PORTS_COMMAND, None).await?;
    Ok(parse_ports(&output))
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredPort {
    pub port: u16,
    pub address: String,
    pub process_name: Option<String>,
    pub pid: Option<u32>,
    pub container_name: Option<String>,
    pub executable: Option<String>,
    pub working_directory: Option<String>,
    pub command: Option<String>,
    pub arguments: Vec<String>,
    pub user: Option<String>,
    pub application_name: Option<String>,
}
const DISCOVER_PORTS_COMMAND: &str = include_str!("discover-ports.sh");

pub fn parse_ports(output: &str) -> Vec<DiscoveredPort> {
    let (output, details) = output
        .split_once("__PORTHOP_PROCESS_DETAILS__\n")
        .unwrap_or((output, ""));
    let (listeners, docker) = output
        .split_once("__PORTHOP_DOCKER_PORTS__\n")
        .unwrap_or((output, ""));
    let mut ports: BTreeMap<(u16, String), DiscoveredPort> = BTreeMap::new();
    for line in listeners.lines().filter(|line| line.contains("LISTEN")) {
        let fields: Vec<_> = line.split_whitespace().collect();
        // Both ss -tlnp and netstat -tlnp put the local address in column four.
        let Some((address, port)) = fields.get(3).and_then(|field| field.rsplit_once(':')) else {
            continue;
        };
        let Some(port) = port.parse::<u16>().ok().filter(|p| *p > 0) else {
            continue;
        };
        let process_name = line
            .split("users:((\"")
            .nth(1)
            .and_then(|s| s.split('"').next())
            .map(str::to_owned)
            .or_else(|| {
                fields
                    .last()
                    .and_then(|s| s.split_once('/'))
                    .map(|(_, name)| name.to_owned())
            });
        let pid = line
            .split("pid=")
            .nth(1)
            .map(|s| {
                s.chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect::<String>()
            })
            .and_then(|s| s.parse().ok())
            .or_else(|| {
                fields
                    .last()
                    .and_then(|s| s.split_once('/'))
                    .and_then(|(pid, _)| pid.parse().ok())
            });
        let address = address.trim_matches(['[', ']']).to_owned();
        let entry = ports
            .entry((port, address.clone()))
            .or_insert(DiscoveredPort {
                port,
                address,
                process_name: None,
                pid: None,
                container_name: None,
                executable: None,
                working_directory: None,
                command: None,
                arguments: Vec::new(),
                user: None,
                application_name: None,
            });
        if entry.process_name.is_none() && process_name.is_some() {
            entry.process_name = process_name;
            entry.pid = pid;
        }
    }
    for line in docker.lines() {
        let Ok(row) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let (Some(name), Some(bindings)) = (row["name"].as_str(), row["ports"].as_str()) else {
            continue;
        };
        for mapping in bindings.split(',').map(str::trim) {
            let Some((host, target)) = mapping.split_once("->") else {
                continue;
            };
            if !target.ends_with("/tcp") {
                continue;
            }
            let Some((address, published)) = host.rsplit_once(':') else {
                continue;
            };
            let (start, end) = published.split_once('-').unwrap_or((published, published));
            let (Ok(start), Ok(end)) = (start.parse::<u16>(), end.parse::<u16>()) else {
                continue;
            };
            if start == 0 || end < start {
                continue;
            }
            let address = address.trim_matches(['[', ']']).to_owned();
            for port in start..=end {
                let entry = ports
                    .entry((port, address.clone()))
                    .or_insert(DiscoveredPort {
                        port,
                        address: address.clone(),
                        process_name: None,
                        pid: None,
                        container_name: None,
                        executable: None,
                        working_directory: None,
                        command: None,
                        arguments: Vec::new(),
                        user: None,
                        application_name: None,
                    });
                entry.container_name = Some(name.to_owned());
            }
        }
    }
    for line in details.lines() {
        let fields: Vec<_> = line.split('|').collect();
        if fields.len() != 5 {
            continue;
        }
        let Ok(pid) = fields[0].parse::<u32>() else {
            continue;
        };
        let decoded: Option<Vec<_>> = fields[1..].iter().map(|s| decode_hex(s)).collect();
        let Some(decoded) = decoded else {
            continue;
        };
        for port in ports.values_mut().filter(|p| p.pid == Some(pid)) {
            port.executable = nonempty(decoded[0].trim_end_matches('\n'));
            port.working_directory = nonempty(decoded[1].trim_end_matches('\n'));
            port.user = nonempty(decoded[3].trim());
            let argv: Vec<_> = decoded[2].split('\0').filter(|s| !s.is_empty()).collect();
            port.command = nonempty(&argv.join(" "));
            port.arguments = argv.iter().map(|arg| (*arg).to_owned()).collect();
            let executable = port
                .executable
                .as_deref()
                .and_then(|p| p.rsplit('/').next());
            let is_node = matches!(executable, Some("node" | "nodejs"));
            // Only identify Vite from its executable argument, not arbitrary flags/text.
            let vite = is_node
                && argv
                    .get(1)
                    .is_some_and(|p| p.ends_with("/vite") || p.ends_with("/vite/bin/vite.js"));
            let runtime = if vite { Some("Vite") } else { executable };
            let project = if is_node {
                port.working_directory
                    .as_deref()
                    .and_then(|p| p.rsplit('/').next())
                    .filter(|s| !s.is_empty())
            } else {
                None
            };
            port.application_name = runtime.map(|name| match project {
                Some(project) => format!("{name} · {project}"),
                None => name.to_owned(),
            });
        }
    }
    ports.into_values().collect()
}
fn nonempty(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_owned())
}
fn decode_hex(value: &str) -> Option<String> {
    if value.len() > 16384 || !value.len().is_multiple_of(2) || !value.is_ascii() {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..value.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&value[i..i + 2], 16).ok())
        .collect();
    Some(String::from_utf8_lossy(&bytes?).into_owned())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn detail(pid: u32, values: [&str; 4]) -> String {
        format!(
            "{}|{}",
            pid,
            values
                .map(|v| v
                    .as_bytes()
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>())
                .join("|")
        )
    }
    #[test]
    fn enriches_node_without_guessing_from_thread_name() {
        let data = format!("LISTEN 0 511 0.0.0.0:5174 *:* users:((\"MainThread\",pid=126945,fd=25))\n__PORTHOP_PROCESS_DETAILS__\n{}\n", detail(126945, ["/nix/store/node/bin/node\n", "/home/ruiyang/Projects/webcontainers-demo\n", "node\0/home/ruiyang/Projects/webcontainers-demo/node_modules/.bin/vite\0--host\x000.0.0.0\0", "ruiyang\n"]));
        let port = &parse_ports(&data)[0];
        assert_eq!(
            port.application_name.as_deref(),
            Some("Vite · webcontainers-demo")
        );
        assert_eq!(port.user.as_deref(), Some("ruiyang"));
        assert_eq!(port.process_name.as_deref(), Some("MainThread"));
        assert_eq!(port.executable.as_deref(), Some("/nix/store/node/bin/node"));
    }
    #[test]
    fn incomplete_or_invalid_metadata_preserves_listener() {
        let base = "LISTEN 0 511 0.0.0.0:5174 *:* users:((\"MainThread\",pid=9,fd=25))\n__PORTHOP_PROCESS_DETAILS__\n";
        for details in ["9||||", "9|zz|||", "9|é|||", "10|6e6f6465|||"] {
            let ports = parse_ports(&format!("{base}{details}\n"));
            assert_eq!(ports.len(), 1);
            assert_eq!(ports[0].application_name, None);
            assert_eq!(ports[0].pid, Some(9));
        }
    }
    #[test]
    fn preserves_spaces_and_does_not_misidentify_command_arguments() {
        let data = format!("LISTEN 0 511 0.0.0.0:5174 *:* users:((\"MainThread\",pid=9,fd=25))\n__PORTHOP_PROCESS_DETAILS__\n{}", detail(9, ["/usr/bin/node\n", "/srv/my project\n", "node\0server.js\0--description=vite\0", "alice\n"]));
        let port = &parse_ports(&data)[0];
        assert_eq!(port.application_name.as_deref(), Some("node · my project"));
        assert_eq!(port.working_directory.as_deref(), Some("/srv/my project"));
    }
    #[test]
    fn discovers_ss_ipv6_and_deduplicates() {
        let p=parse_ports("LISTEN 0 128 0.0.0.0:22 0.0.0.0:* users:((\"sshd\",pid=45,fd=3))\nLISTEN 0 128 [::]:22 [::]:*\nLISTEN 0 128 [::1]:8080 [::]:*");
        assert_eq!(p.len(), 3);
        assert_eq!(p[0].process_name.as_deref(), Some("sshd"));
        assert_eq!(p[0].pid, Some(45));
        assert_eq!(p[2].address, "::1");
    }
    #[test]
    fn discovers_netstat() {
        let p = parse_ports("tcp 0 0 127.0.0.1:5432 0.0.0.0:* LISTEN 12/postgres");
        assert_eq!(p[0].port, 5432);
        assert_eq!(p[0].process_name.as_deref(), Some("postgres"));
    }
    #[test]
    fn keeps_richer_details_and_distinct_bindings() {
        let ports = parse_ports("LISTEN 0 128 127.0.0.1:8080 *:*\nLISTEN 0 128 127.0.0.1:8080 *:* users:((\"node\",pid=20,fd=3))\nLISTEN 0 128 10.0.0.2:8080 *:* users:((\"other\",pid=21,fd=3))");
        assert_eq!(ports.len(), 2);
        let local = ports.iter().find(|p| p.address == "127.0.0.1").unwrap();
        assert_eq!(local.process_name.as_deref(), Some("node"));
        assert_eq!(local.pid, Some(20));
    }
    #[test]
    fn identifies_docker_bindings_without_guessing_processes() {
        let ports = parse_ports(
            r#"LISTEN 0 128 0.0.0.0:8080 *:*
__PORTHOP_DOCKER_PORTS__
{"name":"web","ports":"0.0.0.0:8080->80/tcp, [::]:8080->80/tcp, 80/tcp, 0.0.0.0:5353->53/udp"}
{"name":"workers","ports":"127.0.0.1:9000-9001->9000-9001/tcp"}
not json
"#,
        );
        assert_eq!(ports.len(), 4);
        assert_eq!(ports[0].container_name.as_deref(), Some("web"));
        assert_eq!(ports[0].process_name, None);
        assert_eq!(ports[1].address, "::");
        assert_eq!(ports[3].port, 9001);
    }
    #[test]
    fn missing_docker_access_keeps_listeners() {
        let ports = parse_ports("LISTEN 0 128 0.0.0.0:22 *:*\n__PORTHOP_DOCKER_PORTS__\n");
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].process_name, None);
        assert_eq!(ports[0].container_name, None);
    }
}
