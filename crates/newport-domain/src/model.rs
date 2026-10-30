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
    #[serde(default)]
    pub browser_enabled: bool,
    #[serde(default)]
    pub agent_install_approved: bool,
    #[serde(default)]
    pub setup_complete: bool,
    /// How the server is reached; `None` connects to it directly.
    #[serde(default)]
    pub proxy: Option<Proxy>,
}
/// What a server's SSH connection goes through.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum Proxy {
    /// A command run on this computer, as ssh's `ProxyCommand`; SSH runs over
    /// its input and output. `%h`, `%p`, `%r` and `%n` stand for the server's
    /// host, port, username and name.
    Command(String),
    /// Another saved server, as ssh's `ProxyJump`. A chain is a jump host
    /// with its own jump host.
    Jump(Uuid),
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
    #[serde(default)]
    pub direction: Direction,
    /// The project this forward was saved under; the UI owns its meaning.
    #[serde(default)]
    pub project: Option<String>,
}
/// Local forwards a Mac port to the server (ssh -L). Remote exposes a Mac port
/// on the server (ssh -R): `remote_host` is the server bind address and
/// `local_port` the target on this Mac's loopback.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Direction {
    #[default]
    Local,
    Remote,
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
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum DestinationStatus {
    Checking,
    Reachable,
    Unavailable,
    Blocked,
    Unknown,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DestinationHealth {
    pub local_port: u16,
    pub remote_port: u16,
    pub status: DestinationStatus,
    pub message: Option<String>,
    pub checked_at: Option<u64>,
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
/// A host name or an unbracketed IP address, with nothing a shell or an SSH
/// option would read as more than a name.
pub fn host_valid(s: &str) -> bool {
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
            && self.proxy == other.proxy
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
        match &self.proxy {
            Some(Proxy::Command(command)) => proxy_command_valid(command)?,
            Some(Proxy::Jump(id)) if *id == self.id => {
                return Err("A server can’t be its own jump host.".into());
            }
            _ => {}
        }
        Ok(())
    }
}
/// The tokens a proxy command may use are the ones Newport expands.
fn proxy_command_valid(command: &str) -> Result<(), String> {
    if command.trim().is_empty() {
        return Err("Enter a proxy command.".into());
    }
    if command.contains(['\n', '\r', '\0']) {
        return Err("Write the proxy command on one line.".into());
    }
    let mut chars = command.chars();
    while let Some(c) = chars.next() {
        if c == '%' && !matches!(chars.next(), Some('h' | 'p' | 'r' | 'n' | '%')) {
            return Err("The proxy command can use only %h, %p, %r, %n and %% after a %.".into());
        }
    }
    Ok(())
}
fn forwarding_host(host: &str) -> String {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let host = host
        .parse::<std::net::IpAddr>()
        .map(|ip| ip.to_string())
        .unwrap_or(host);
    match host.as_str() {
        "localhost" | "0.0.0.0" => "127.0.0.1".into(),
        "::" => "::1".into(),
        _ => host,
    }
}
impl Tunnel {
    /// Validate new/edited forwards without invalidating older saved profiles.
    pub fn validate_unique(&self, saved: &[Tunnel]) -> Result<(), String> {
        self.pairs()?;
        // Only the same direction competes: a remote forward binds nothing on this Mac.
        for other in saved
            .iter()
            .filter(|t| t.id != self.id && t.direction == self.direction)
        {
            let local_overlap = self.local_port <= other.local_port_end.unwrap_or(other.local_port)
                && other.local_port <= self.local_port_end.unwrap_or(self.local_port);
            if self.direction == Direction::Local && local_overlap {
                return Err(format!(
                    "Local port range overlaps the saved tunnel '{}'. Edit that tunnel or choose another local port.",
                    other.name
                ));
            }
            let remote_overlap = self.remote_port
                <= other.remote_port_end.unwrap_or(other.remote_port)
                && other.remote_port <= self.remote_port_end.unwrap_or(self.remote_port);
            if self.server_id == other.server_id
                && forwarding_host(&self.remote_host) == forwarding_host(&other.remote_host)
                && remote_overlap
            {
                return Err(match self.direction {
                    Direction::Local => format!(
                        "This remote port is already forwarded by '{}'. Edit the existing tunnel instead.",
                        other.name
                    ),
                    Direction::Remote => format!(
                        "This server port is already exposed by '{}'. Edit that forward or choose another server port.",
                        other.name
                    ),
                });
            }
        }
        Ok(())
    }

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
            server.browser_enabled = existing.browser_enabled && existing.same_destination(&server);
            *existing = server;
        } else {
            server.clipboard_enabled = false;
            server.browser_enabled = false;
            self.servers.push(server);
        }
    }
    pub fn integration_servers(&self) -> Vec<Uuid> {
        self.servers
            .iter()
            .filter(|server| server.clipboard_enabled || server.browser_enabled)
            .map(|server| server.id)
            .collect()
    }

    /// The saved servers `server` connects through, nearest first. Fails when
    /// one is missing or the chain leads back to a server already in it.
    pub fn jump_hosts(&self, server: &Server) -> Result<Vec<&Server>, String> {
        let mut chain: Vec<&Server> = Vec::new();
        let mut proxy = &server.proxy;
        while let Some(Proxy::Jump(id)) = proxy {
            if *id == server.id || chain.iter().any(|jump| jump.id == *id) {
                return Err(format!(
                    "The jump hosts of ‘{}’ lead back to it. Choose another jump host.",
                    server.name
                ));
            }
            let jump = self
                .servers
                .iter()
                .find(|saved| saved.id == *id)
                .ok_or_else(|| format!("The jump host of ‘{}’ isn’t saved.", server.name))?;
            chain.push(jump);
            proxy = &jump.proxy;
        }
        Ok(chain)
    }
    /// `id` and every saved server that connects through it.
    pub fn through(&self, id: Uuid) -> Vec<Uuid> {
        let mut found = vec![id];
        let mut index = 0;
        while let Some(&jump) = found.get(index) {
            for server in &self.servers {
                if server.proxy == Some(Proxy::Jump(jump)) && !found.contains(&server.id) {
                    found.push(server.id);
                }
            }
            index += 1;
        }
        found
    }

    pub fn validate(&self) -> Result<(), String> {
        let mut ids = HashSet::new();
        for s in &self.servers {
            s.validate()?;
            if !ids.insert(s.id) {
                return Err("Duplicate server ID.".into());
            }
        }
        for s in &self.servers {
            self.jump_hosts(s)?;
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
    fn prevents_duplicate_destinations_and_local_overlap_but_allows_edits() {
        let original = tunnel();
        assert!(
            original
                .validate_unique(std::slice::from_ref(&original))
                .is_ok()
        );
        let mut candidate = original.clone();
        candidate.id = Uuid::new_v4();
        candidate.local_port = 9000;
        candidate.remote_host = "LOCALHOST.".into();
        assert!(
            candidate
                .validate_unique(std::slice::from_ref(&original))
                .unwrap_err()
                .contains("already forwarded")
        );
        candidate.server_id = Uuid::new_v4();
        assert!(
            candidate
                .validate_unique(std::slice::from_ref(&original))
                .is_ok()
        );
        candidate.local_port = original.local_port;
        assert!(
            candidate
                .validate_unique(std::slice::from_ref(&original))
                .unwrap_err()
                .contains("Local port")
        );
    }
    #[test]
    fn prevents_partial_range_duplicates() {
        let mut original = tunnel();
        original.local_port_end = Some(8002);
        original.remote_port_end = Some(82);
        let mut candidate = original.clone();
        candidate.id = Uuid::new_v4();
        candidate.local_port = 9000;
        candidate.local_port_end = None;
        candidate.remote_port = 81;
        candidate.remote_port_end = None;
        assert!(
            candidate
                .validate_unique(std::slice::from_ref(&original))
                .is_err()
        );
        candidate.remote_port = 83;
        assert!(
            candidate
                .validate_unique(std::slice::from_ref(&original))
                .is_ok()
        );
        assert_eq!(forwarding_host("0:0:0:0:0:0:0:0"), forwarding_host("::1"));
        assert_ne!(forwarding_host("::1"), forwarding_host("127.0.0.1"));
    }
    #[test]
    fn tunnels_default_to_local_without_a_project() {
        let t = tunnel();
        assert_eq!(t.direction, Direction::Local);
        assert_eq!(t.project, None);
        let json = serde_json::to_value(&t).unwrap();
        assert_eq!(json["direction"], "local");
        let remote: Tunnel = serde_json::from_value(serde_json::json!({"id":Uuid::new_v4(),"serverId":t.server_id,"name":"r","localPort":3000,"remotePort":9000,"remoteHost":"127.0.0.1","direction":"remote","project":"/srv/app"})).unwrap();
        assert_eq!(remote.direction, Direction::Remote);
        assert_eq!(remote.project.as_deref(), Some("/srv/app"));
    }
    #[test]
    fn directions_only_conflict_with_their_own_kind() {
        let local = tunnel();
        let mut remote = local.clone();
        remote.id = Uuid::new_v4();
        remote.direction = Direction::Remote;
        // Same Mac port and same server port, opposite directions: both valid.
        assert!(remote.validate_unique(std::slice::from_ref(&local)).is_ok());
        assert!(local.validate_unique(std::slice::from_ref(&remote)).is_ok());
        let mut other = remote.clone();
        other.id = Uuid::new_v4();
        other.local_port = 9999;
        other.remote_host = "localhost".into();
        assert!(
            other
                .validate_unique(std::slice::from_ref(&remote))
                .unwrap_err()
                .contains("already exposed")
        );
        // Two remote forwards may target the same Mac port.
        other.remote_port = 81;
        other.local_port = remote.local_port;
        assert!(other.validate_unique(std::slice::from_ref(&remote)).is_ok());
        other.server_id = Uuid::new_v4();
        other.remote_port = remote.remote_port;
        assert!(other.validate_unique(std::slice::from_ref(&remote)).is_ok());
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
        let server: Server = serde_json::from_value(serde_json::json!({"id":Uuid::new_v4(),"name":"Saved","sshHost":"host","sshUser":"user","sshPort":22,"clipboardEnabled":true,"browserEnabled":true})).unwrap();
        let mut config = Config {
            servers: vec![server.clone()],
            tunnels: vec![],
        };
        let mut renamed = server.clone();
        renamed.name = "Renamed".into();
        renamed.browser_enabled = false;
        renamed.clipboard_enabled = false; // a stale editor must not reset consent
        assert!(renamed.same_connection(&server));
        config.upsert_server(renamed.clone());
        assert!(config.servers[0].clipboard_enabled);
        assert!(config.servers[0].browser_enabled);
        let mut auth_change = renamed.clone();
        auth_change.identity_file = Some("/new/key".into());
        assert!(!auth_change.same_connection(&server));
        config.upsert_server(auth_change);
        assert!(config.servers[0].clipboard_enabled);
        assert!(config.servers[0].browser_enabled);
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
            assert!(!config.servers[0].browser_enabled);
        }
        config.servers.clear();
        config.upsert_server(server);
        assert!(!config.servers[0].clipboard_enabled);
        assert!(!config.servers[0].browser_enabled); // only the explicit toggle opts in
    }
    #[test]
    fn swift_server_missing_auth_defaults_to_key() {
        let s:Server=serde_json::from_value(serde_json::json!({"id":Uuid::new_v4(),"name":"","sshUser":"user","sshHost":"host","sshPort":22})).unwrap();
        assert_eq!(s.auth_method, AuthMethod::PublicKey);
        assert!(!s.clipboard_enabled);
        assert!(!s.browser_enabled);
        assert!(!s.agent_install_approved);
        assert!(!s.setup_complete);
        let mut approved = s;
        approved.agent_install_approved = true;
        approved.setup_complete = true;
        let saved = serde_json::to_value(&approved).unwrap();
        assert_eq!(saved["agentInstallApproved"], true);
        assert_eq!(saved["setupComplete"], true);
        assert_eq!(serde_json::from_value::<Server>(saved).unwrap(), approved);
    }
    fn server(name: &str) -> Server {
        serde_json::from_value(serde_json::json!({"id":Uuid::new_v4(),"name":name,"sshUser":"user","sshHost":"host","sshPort":22})).unwrap()
    }
    #[test]
    fn a_saved_server_without_a_proxy_connects_directly() {
        let saved = serde_json::json!({"id":Uuid::new_v4(),"name":"Old","sshUser":"user","sshHost":"host","sshPort":22,"authMethod":"publicKey","identityFile":null});
        let server: Server = serde_json::from_value(saved).unwrap();
        assert_eq!(server.proxy, None);
        assert_eq!(
            serde_json::to_value(&server).unwrap()["proxy"],
            serde_json::Value::Null
        );
    }
    #[test]
    fn proxies_round_trip_in_their_saved_form() {
        let jump = Uuid::new_v4();
        for (proxy, json) in [
            (
                Proxy::Command("nc -X connect -x proxy:8080 %h %p".into()),
                serde_json::json!({"command":"nc -X connect -x proxy:8080 %h %p"}),
            ),
            (Proxy::Jump(jump), serde_json::json!({"jump":jump})),
        ] {
            let mut s = server("Proxied");
            s.proxy = Some(proxy);
            let saved = serde_json::to_value(&s).unwrap();
            assert_eq!(saved["proxy"], json);
            assert_eq!(serde_json::from_value::<Server>(saved).unwrap(), s);
        }
    }
    #[test]
    fn a_proxy_command_is_one_line_with_known_tokens() {
        let mut s = server("Proxied");
        for (command, valid) in [
            ("ssh -W %h:%p bastion", true),
            ("connect %r@%n 100%%", true),
            ("", false),
            ("   ", false),
            ("nc %h %p\nProxyCommand=bad", false),
            ("nc %h %p\r", false),
            ("nc %C", false),
            ("nc %h %", false),
        ] {
            s.proxy = Some(Proxy::Command(command.into()));
            assert_eq!(s.validate().is_ok(), valid, "{command:?}");
        }
    }
    #[test]
    fn a_server_cant_jump_through_itself() {
        let mut s = server("Loop");
        s.proxy = Some(Proxy::Jump(s.id));
        assert!(s.validate().unwrap_err().contains("its own jump host"));
        let config = Config {
            servers: vec![s],
            tunnels: vec![],
        };
        assert!(config.validate().is_err());
    }
    #[test]
    fn jump_hosts_must_be_saved_and_never_lead_back() {
        let bastion = server("Bastion");
        let mut middle = server("Middle");
        middle.proxy = Some(Proxy::Jump(bastion.id));
        let mut inner = server("Inner");
        inner.proxy = Some(Proxy::Jump(middle.id));
        let mut config = Config {
            servers: vec![inner.clone(), middle.clone(), bastion.clone()],
            tunnels: vec![],
        };
        config.validate().unwrap();
        let chain: Vec<_> = config
            .jump_hosts(&inner)
            .unwrap()
            .iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(chain, [middle.id, bastion.id]);
        assert_eq!(
            config.through(bastion.id),
            [bastion.id, middle.id, inner.id]
        );
        assert_eq!(config.through(inner.id), [inner.id]);

        config.servers[2].proxy = Some(Proxy::Jump(inner.id));
        assert!(config.validate().unwrap_err().contains("lead back"));

        config.servers.truncate(2);
        assert!(config.validate().unwrap_err().contains("isn’t saved"));
    }
}
