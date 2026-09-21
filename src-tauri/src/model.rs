use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use uuid::Uuid;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum AuthMethod {
    #[default]
    PublicKey,
    Password,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Server {
    pub id: Uuid,
    pub name: String,
    pub ssh_user: String,
    pub ssh_host: String,
    pub ssh_port: u16,
    pub identity_file: Option<String>,
    #[serde(default)]
    pub agent_source: Option<String>,
    #[serde(default)]
    pub agent_key_fingerprint: Option<String>,
    #[serde(default)]
    pub auth_method: AuthMethod,
    #[serde(default)]
    pub clipboard_enabled: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Tunnel {
    pub id: Uuid,
    pub name: String,
    pub server_id: Uuid,
    pub local_port: u16,
    pub local_port_end: Option<u16>,
    pub remote_host: String,
    pub remote_port: u16,
    pub remote_port_end: Option<u16>,
    #[serde(default)]
    pub auto_connect: bool,
    #[serde(default = "yes")]
    pub auto_reconnect: bool,
}
fn yes() -> bool {
    true
}
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub servers: Vec<Server>,
    pub tunnels: Vec<Tunnel>,
}
#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionState {
    pub status: Status,
    pub error_message: Option<String>,
    pub reconnect_attempt: u32,
}
#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
    Error,
}
impl ConnectionState {
    pub fn new(status: Status) -> Self {
        Self {
            status,
            ..Self::default()
        }
    }
    pub fn error(message: impl ToString) -> Self {
        Self {
            status: Status::Error,
            error_message: Some(message.to_string()),
            reconnect_attempt: 0,
        }
    }
}
fn host_valid(s: &str) -> bool {
    (!s.contains(':') || s.parse::<std::net::Ipv6Addr>().is_ok())
        && !s.is_empty()
        && s.len() <= 253
        && !s.starts_with('-')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || ".-_:".contains(c))
}
impl Server {
    pub fn same_destination(&self, other: &Self) -> bool {
        self.ssh_host == other.ssh_host
            && self.ssh_port == other.ssh_port
            && self.ssh_user == other.ssh_user
    }
    pub fn same_connection(&self, other: &Self) -> bool {
        self.same_destination(other)
            && self.identity_file == other.identity_file
            && self.agent_source == other.agent_source
            && self.agent_key_fingerprint == other.agent_key_fingerprint
            && self.auth_method == other.auth_method
    }

    pub fn validate(&self) -> Result<(), String> {
        if self
            .agent_source
            .as_deref()
            .is_some_and(|s| !["system", "onePassword"].contains(&s))
        {
            return Err("Unknown SSH agent".into());
        }
        if let Some(fingerprint) = &self.agent_key_fingerprint {
            if !fingerprint.starts_with("SHA256:")
                || fingerprint.len() != 50
                || !fingerprint[7..]
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
            {
                return Err("Invalid SSH key fingerprint".into());
            }
            if self.identity_file.as_ref().is_some_and(|s| !s.is_empty()) {
                return Err("Choose an agent key or a key file, not both".into());
            }
        }
        if !host_valid(&self.ssh_host) {
            return Err("Enter a hostname or an unbracketed IP address.".into());
        }
        if self.ssh_user.is_empty()
            || self.ssh_user.starts_with('-')
            || !self
                .ssh_user
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c))
        {
            return Err("Enter a valid SSH username.".into());
        }
        if self.ssh_port == 0 {
            return Err("SSH port must be between 1 and 65535.".into());
        }
        Ok(())
    }
}
impl Tunnel {
    pub fn pairs(&self) -> Result<Vec<(u16, u16)>, String> {
        let le = self.local_port_end.unwrap_or(self.local_port);
        let re = self.remote_port_end.unwrap_or(self.remote_port);
        if self.local_port == 0
            || self.remote_port == 0
            || le < self.local_port
            || re < self.remote_port
        {
            return Err("Ports must be 1–65535 and ranges must run from low to high.".into());
        }
        if le - self.local_port != re - self.remote_port {
            return Err("Local and remote ranges must contain the same number of ports.".into());
        }
        if le - self.local_port >= 256 {
            return Err("Use at most 256 ports per tunnel.".into());
        }
        if !host_valid(&self.remote_host) {
            return Err("Enter a valid remote hostname or unbracketed IP address.".into());
        }
        Ok((self.local_port..=le).zip(self.remote_port..=re).collect())
    }
}
impl Config {
    /// Clipboard consent is managed by its dedicated command, never a stale editor snapshot.
    pub fn upsert_server(&mut self, mut server: Server) {
        if let Some(existing) = self.servers.iter_mut().find(|s| s.id == server.id) {
            server.clipboard_enabled =
                existing.clipboard_enabled && existing.same_destination(&server);
            *existing = server;
        } else {
            server.clipboard_enabled = false;
            self.servers.push(server);
        }
    }
    pub fn clipboard_servers(&self) -> Vec<Uuid> {
        self.servers
            .iter()
            .filter(|server| server.clipboard_enabled)
            .map(|server| server.id)
            .collect()
    }

    pub fn validate(&self) -> Result<(), String> {
        let mut ids = HashSet::new();
        for s in &self.servers {
            s.validate()?;
            if !ids.insert(s.id) {
                return Err("Duplicate server ID.".into());
            }
        }
        let mut tunnels = HashSet::new();
        for t in &self.tunnels {
            t.pairs()?;
            if !ids.contains(&t.server_id) {
                return Err(format!("Tunnel '{}' references a missing server.", t.name));
            }
            if !tunnels.insert(t.id) {
                return Err("Duplicate tunnel ID.".into());
            }
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn tunnel() -> Tunnel {
        serde_json::from_value(serde_json::json!({"id":Uuid::new_v4(),"serverId":Uuid::new_v4(),"name":"test","localPort":8000,"remotePort":80,"remoteHost":"127.0.0.1"})).unwrap()
    }
    #[test]
    fn ranges_are_equal_and_bounded() {
        let mut t = tunnel();
        assert_eq!(t.pairs().unwrap(), vec![(8000, 80)]);
        t.local_port_end = Some(8002);
        assert!(t.pairs().is_err());
        t.remote_port_end = Some(82);
        assert_eq!(t.pairs().unwrap().len(), 3);
        t.local_port_end = Some(7999);
        assert!(t.pairs().is_err());
        t.local_port = 0;
        assert!(t.pairs().is_err());
    }
    #[test]
    fn rejects_forwarding_injection() {
        let mut t = tunnel();
        t.remote_host = "host:80:other".into();
        assert!(t.pairs().is_err());
        t.remote_host = "host\nProxyCommand=bad".into();
        assert!(t.pairs().is_err());
    }
    #[test]
    fn server_edits_preserve_consent_only_for_the_same_destination() {
        let server: Server = serde_json::from_value(serde_json::json!({"id":Uuid::new_v4(),"name":"Saved","sshHost":"host","sshUser":"user","sshPort":22,"clipboardEnabled":true})).unwrap();
        let mut config = Config {
            servers: vec![server.clone()],
            tunnels: vec![],
        };
        let mut renamed = server.clone();
        renamed.name = "Renamed".into();
        renamed.clipboard_enabled = false; // a stale editor must not reset consent
        assert!(renamed.same_connection(&server));
        config.upsert_server(renamed.clone());
        assert!(config.servers[0].clipboard_enabled);
        let mut auth_change = renamed.clone();
        auth_change.identity_file = Some("/new/key".into());
        assert!(!auth_change.same_connection(&server));
        config.upsert_server(auth_change);
        assert!(config.servers[0].clipboard_enabled);
        for field in ["host", "user", "port"] {
            let mut changed = server.clone();
            match field {
                "host" => changed.ssh_host = "elsewhere".into(),
                "user" => changed.ssh_user = "other".into(),
                _ => changed.ssh_port = 2222,
            }
            config.servers = vec![server.clone()];
            config.upsert_server(changed);
            assert!(!config.servers[0].clipboard_enabled);
        }
        config.servers.clear();
        config.upsert_server(server);
        assert!(!config.servers[0].clipboard_enabled); // only the explicit toggle opts in
    }
    #[test]
    fn swift_server_missing_auth_defaults_to_key() {
        let s:Server=serde_json::from_value(serde_json::json!({"id":Uuid::new_v4(),"name":"","sshUser":"user","sshHost":"host","sshPort":22})).unwrap();
        assert_eq!(s.auth_method, AuthMethod::PublicKey);
        assert!(!s.clipboard_enabled);
    }
}
