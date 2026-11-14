//! Typed desktop configuration contracts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AuthMethod {
    PublicKey,
    Password,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Local,
    Remote,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feature {
    Clipboard,
    Browser,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Proxy {
    Command(Box<String>),
    Jump(Box<uuid::Uuid>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Server {
    pub id: uuid::Uuid,
    pub name: String,
    pub ssh_user: String,
    pub ssh_host: String,
    pub ssh_port: u16,
    pub identity_file: Option<String>,
    pub agent_source: Option<String>,
    pub agent_key_fingerprint: Option<String>,
    pub auth_method: crate::desktop::configuration::AuthMethod,
    pub clipboard_enabled: bool,
    pub browser_enabled: bool,
    pub agent_install_approved: bool,
    pub setup_complete: bool,
    pub proxy: Option<crate::desktop::configuration::Proxy>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tunnel {
    pub id: uuid::Uuid,
    pub name: String,
    pub server_id: uuid::Uuid,
    pub local_port: u16,
    pub local_port_end: Option<u16>,
    pub remote_host: String,
    pub remote_port: u16,
    pub remote_port_end: Option<u16>,
    pub auto_connect: bool,
    pub auto_reconnect: bool,
    pub direction: crate::desktop::configuration::Direction,
    pub project: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub servers: Vec<crate::desktop::configuration::Server>,
    pub tunnels: Vec<crate::desktop::configuration::Tunnel>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ConnectionSettings {
    pub server_id: uuid::Uuid,
    pub ssh_host: String,
    pub ssh_user: String,
    pub ssh_port: u16,
    pub auth_method: crate::desktop::configuration::AuthMethod,
    pub identity_file: Option<String>,
    pub agent_source: Option<String>,
    pub agent_key_fingerprint: Option<String>,
    pub proxy: Option<crate::desktop::configuration::Proxy>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ConnectionScope {
    pub settings: crate::desktop::configuration::ConnectionSettings,
    pub connection_revision: u64,
    pub backend_instance: uuid::Uuid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportStatus {
    Disconnected,
    Connecting,
    Connected,
    Recovering,
    Failed,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Transport {
    pub status: crate::desktop::configuration::TransportStatus,
    pub generation: u64,
    pub waiting: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeStatus {
    Unknown,
    Starting,
    Ready,
    Failed,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RuntimeInstance {
    pub instance_id: uuid::Uuid,
    pub build_sha256: String,
    pub generation: u64,
    pub retiring: bool,
    pub draining: bool,
    pub connected: bool,
    pub registrations: u64,
    pub worker_leases: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RuntimeSnapshot {
    pub status: crate::desktop::configuration::RuntimeStatus,
    pub generation: u64,
    pub hello: Option<crate::runtime::Hello>,
    pub error: Option<String>,
    pub retryable: bool,
    pub failures: u32,
    pub instances: Vec<crate::desktop::configuration::RuntimeInstance>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PermissionsSnapshot {
    pub desired: crate::runtime::Permissions,
    /// Revision zero means no acknowledgement, with both permissions disabled.
    pub effective: crate::runtime::Permissions,
    pub error: Option<String>,
    pub clipboard_epoch: u64,
    pub browser_epoch: u64,
    pub session_epoch: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildRelation {
    Unknown,
    Missing,
    Bundled,
    Different,
    Older,
    Newer,
    SameRelease,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UpdateOutcome {
    pub at: u64,
    pub from: Option<String>,
    pub to: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct InstallationStatus {
    pub installed: Option<bool>,
    pub update_available: Option<bool>,
    pub build_relation: crate::desktop::configuration::BuildRelation,
    pub installed_hash: Option<String>,
    pub bundled_hash: Option<String>,
    pub installed_version: Option<String>,
    pub bundled_version: Option<String>,
    pub installed_variant: Option<crate::product::Variant>,
    pub bundled_variant: crate::product::Variant,
    pub installed_running: Option<bool>,
    pub bundled_staged: bool,
    pub checked_at: u64,
    pub error: Option<String>,
    pub automatic_update: Option<crate::desktop::configuration::UpdateOutcome>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallStage {
    Preparing,
    Uploading,
    Installing,
    Verifying,
    Activating,
    Ready,
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub struct InstallProgress {
    pub stage: crate::desktop::configuration::InstallStage,
    pub sent: u64,
    pub total: u64,
    pub error: Option<String>,
    pub failed_stage: Option<crate::desktop::configuration::InstallStage>,
    pub bytes_per_second: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConnectionState {
    pub status: crate::desktop::configuration::ConnectionStatus,
    pub error_message: Option<String>,
    pub reconnect_attempt: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DestinationStatus {
    Checking,
    Reachable,
    Unavailable,
    Blocked,
    Unknown,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DestinationHealth {
    pub local_port: u16,
    pub remote_port: u16,
    pub status: crate::desktop::configuration::DestinationStatus,
    pub message: Option<String>,
    pub checked_at: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DestinationHealthList {
    pub values: Vec<crate::desktop::configuration::DestinationHealth>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TransportRow {
    pub id: uuid::Uuid,
    pub value: crate::desktop::configuration::Transport,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RuntimeRow {
    pub id: uuid::Uuid,
    pub value: crate::desktop::configuration::RuntimeSnapshot,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PermissionsRow {
    pub id: uuid::Uuid,
    pub value: crate::desktop::configuration::PermissionsSnapshot,
}

#[derive(Clone, Debug, PartialEq)]
pub struct InstallRow {
    pub id: uuid::Uuid,
    pub value: crate::desktop::configuration::InstallProgress,
}

#[derive(Clone, Debug, PartialEq)]
pub struct InstallationRow {
    pub id: uuid::Uuid,
    pub value: crate::desktop::configuration::InstallationStatus,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConnectionRow {
    pub id: uuid::Uuid,
    pub value: crate::desktop::configuration::ConnectionState,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HealthRow {
    pub id: uuid::Uuid,
    pub value: crate::desktop::configuration::DestinationHealthList,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MessageRow {
    pub id: uuid::Uuid,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RevisionRow {
    pub id: uuid::Uuid,
    pub value: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Runtime {
    pub agent_transport: Vec<crate::desktop::configuration::TransportRow>,
    pub agent_runtime: Vec<crate::desktop::configuration::RuntimeRow>,
    pub agent_permissions: Vec<crate::desktop::configuration::PermissionsRow>,
    pub agent_status: Vec<crate::desktop::configuration::InstallationRow>,
    pub agent_installation: Vec<crate::desktop::configuration::InstallRow>,
    pub tunnels: Vec<crate::desktop::configuration::ConnectionRow>,
    pub tunnel_health: Vec<crate::desktop::configuration::HealthRow>,
    pub clipboard: Vec<crate::desktop::configuration::ConnectionRow>,
    pub clipboard_messages: Vec<crate::desktop::configuration::MessageRow>,
    pub connection_revisions: Vec<crate::desktop::configuration::RevisionRow>,
    pub awaiting_agent: Vec<uuid::Uuid>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub instance_id: uuid::Uuid,
    pub revision: u64,
    pub config: crate::desktop::configuration::Config,
    pub runtime: crate::desktop::configuration::Runtime,
    pub load_error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Tunnels {
    pub tunnels: Vec<crate::desktop::configuration::Tunnel>,
}

#[derive(Clone, Debug)]
pub struct SaveServerArgs {
    pub server: crate::desktop::configuration::Server,
    pub password: Option<crate::common::Secret>,
}

#[derive(Clone, Debug)]
pub struct DeleteServerArgs {
    pub server: uuid::Uuid,
}

#[derive(Clone, Debug)]
pub struct ForwardDiscoveredPortArgs {
    pub server: uuid::Uuid,
    pub address: String,
    pub port: u16,
}

#[derive(Clone, Debug)]
pub struct ForwardTunnelBatchArgs {
    pub server: uuid::Uuid,
    pub create: Vec<crate::desktop::configuration::Tunnel>,
    pub connect: Vec<crate::desktop::configuration::Tunnel>,
    pub expected: Option<crate::desktop::configuration::ConnectionScope>,
}

#[derive(Clone, Debug)]
pub struct SaveTunnelArgs {
    pub tunnel: crate::desktop::configuration::Tunnel,
}

#[derive(Clone, Debug)]
pub struct UpdateTunnelArgs {
    pub tunnel: crate::desktop::configuration::Tunnel,
    pub expected: crate::desktop::configuration::Tunnel,
}

#[derive(Clone, Debug)]
pub struct DeleteTunnelArgs {
    pub tunnel: uuid::Uuid,
}

#[derive(Clone, Debug)]
pub struct DeleteTunnelsArgs {
    pub expected: Vec<crate::desktop::configuration::Tunnel>,
}

#[derive(Clone, Debug)]
pub struct SetTunnelConnectedArgs {
    pub tunnel: uuid::Uuid,
    pub connected: bool,
}

#[derive(Clone, Debug)]
pub struct SetClipboardEnabledArgs {
    pub server: uuid::Uuid,
    pub enabled: bool,
    pub expected: crate::desktop::configuration::ConnectionScope,
}

#[derive(Clone, Debug)]
pub struct SetIntegrationEnabledArgs {
    pub server: uuid::Uuid,
    pub feature: crate::desktop::configuration::Feature,
    pub enabled: bool,
    pub expected: crate::desktop::configuration::ConnectionScope,
}

#[derive(Clone, Debug)]
pub struct OpenTunnelArgs {
    pub tunnel: uuid::Uuid,
    pub remote_port: Option<u16>,
    pub expected: Option<crate::desktop::configuration::Tunnel>,
    pub expected_connection: Option<crate::desktop::configuration::ConnectionScope>,
}

#[derive(Clone, Debug)]
pub struct OpenTerminalLinkArgs {
    pub terminal: uuid::Uuid,
    pub url: String,
}

#[derive(Clone, Debug)]
pub enum Operation {
    SaveServer(Box<crate::desktop::configuration::SaveServerArgs>),
    DeleteServer(Box<crate::desktop::configuration::DeleteServerArgs>),
    ForwardDiscoveredPort(Box<crate::desktop::configuration::ForwardDiscoveredPortArgs>),
    ForwardTunnelBatch(Box<crate::desktop::configuration::ForwardTunnelBatchArgs>),
    SaveTunnel(Box<crate::desktop::configuration::SaveTunnelArgs>),
    UpdateTunnel(Box<crate::desktop::configuration::UpdateTunnelArgs>),
    DeleteTunnel(Box<crate::desktop::configuration::DeleteTunnelArgs>),
    DeleteTunnels(Box<crate::desktop::configuration::DeleteTunnelsArgs>),
    SetTunnelConnected(Box<crate::desktop::configuration::SetTunnelConnectedArgs>),
    SetClipboardEnabled(Box<crate::desktop::configuration::SetClipboardEnabledArgs>),
    SetIntegrationEnabled(Box<crate::desktop::configuration::SetIntegrationEnabledArgs>),
    OpenTunnel(Box<crate::desktop::configuration::OpenTunnelArgs>),
    OpenTerminalLink(Box<crate::desktop::configuration::OpenTerminalLinkArgs>),
    Snapshot(Box<crate::common::Present>),
}

#[derive(Clone, Debug)]
pub enum Reply {
    SaveServer(Box<crate::common::Present>),
    DeleteServer(Box<crate::common::Present>),
    ForwardDiscoveredPort(Box<crate::common::Present>),
    ForwardTunnelBatch(Box<crate::desktop::configuration::Tunnels>),
    SaveTunnel(Box<crate::common::Present>),
    UpdateTunnel(Box<crate::common::Present>),
    DeleteTunnel(Box<crate::common::Present>),
    DeleteTunnels(Box<crate::common::Present>),
    SetTunnelConnected(Box<crate::common::Present>),
    SetClipboardEnabled(Box<crate::common::Present>),
    SetIntegrationEnabled(Box<crate::common::Present>),
    OpenTunnel(Box<crate::common::Present>),
    OpenTerminalLink(Box<crate::common::Present>),
    Snapshot(Box<crate::desktop::configuration::Snapshot>),
}

impl Server {
    /// Every setting whose change needs a new SSH connection.
    pub fn connection_settings(&self) -> ConnectionSettings {
        ConnectionSettings {
            server_id: self.id,
            ssh_host: self.ssh_host.clone(),
            ssh_user: self.ssh_user.clone(),
            ssh_port: self.ssh_port,
            auth_method: self.auth_method,
            identity_file: self.identity_file.clone(),
            agent_source: self.agent_source.clone(),
            agent_key_fingerprint: self.agent_key_fingerprint.clone(),
            proxy: self.proxy.clone(),
        }
    }
}

impl Config {
    /// The saved server with this id.
    pub fn server(&self, id: uuid::Uuid) -> Option<&Server> {
        self.servers.iter().find(|server| server.id == id)
    }

    /// The forwards saved for a server, in saved order.
    pub fn tunnels_of(&self, server: uuid::Uuid) -> impl Iterator<Item = &Tunnel> + Clone {
        self.tunnels
            .iter()
            .filter(move |tunnel| tunnel.server_id == server)
    }
}

impl Runtime {
    /// A server's agent transport, once the backend has tried to reach it.
    pub fn agent_transport(&self, server: uuid::Uuid) -> Option<&Transport> {
        self.agent_transport
            .iter()
            .find(|row| row.id == server)
            .map(|row| &row.value)
    }

    /// A server's agent runtime: its state and the agent's hello.
    pub fn agent_runtime(&self, server: uuid::Uuid) -> Option<&RuntimeSnapshot> {
        self.agent_runtime
            .iter()
            .find(|row| row.id == server)
            .map(|row| &row.value)
    }

    /// A server's desired and acknowledged integration permissions.
    pub fn agent_permissions(&self, server: uuid::Uuid) -> Option<&PermissionsSnapshot> {
        self.agent_permissions
            .iter()
            .find(|row| row.id == server)
            .map(|row| &row.value)
    }

    /// The last check of the agent installed on a server.
    pub fn agent_status(&self, server: uuid::Uuid) -> Option<&InstallationStatus> {
        self.agent_status
            .iter()
            .find(|row| row.id == server)
            .map(|row| &row.value)
    }

    /// The progress of an agent install running on a server.
    pub fn agent_installation(&self, server: uuid::Uuid) -> Option<&InstallProgress> {
        self.agent_installation
            .iter()
            .find(|row| row.id == server)
            .map(|row| &row.value)
    }

    /// A forward's connection.
    pub fn tunnel(&self, tunnel: uuid::Uuid) -> Option<&ConnectionState> {
        self.tunnels
            .iter()
            .find(|row| row.id == tunnel)
            .map(|row| &row.value)
    }

    /// A forward's destination checks.
    pub fn tunnel_health(&self, tunnel: uuid::Uuid) -> &[DestinationHealth] {
        self.tunnel_health
            .iter()
            .find(|row| row.id == tunnel)
            .map_or(&[], |row| &row.value.values)
    }

    /// A server's clipboard connection.
    pub fn clipboard(&self, server: uuid::Uuid) -> Option<&ConnectionState> {
        self.clipboard
            .iter()
            .find(|row| row.id == server)
            .map(|row| &row.value)
    }

    /// The last clipboard message for a server.
    pub fn clipboard_message(&self, server: uuid::Uuid) -> Option<&str> {
        self.clipboard_messages
            .iter()
            .find(|row| row.id == server)
            .map(|row| row.value.as_str())
    }

    /// A server's connection revision. A server the backend has not connected
    /// yet is at revision zero.
    pub fn connection_revision(&self, server: uuid::Uuid) -> u64 {
        self.connection_revisions
            .iter()
            .find(|row| row.id == server)
            .map_or(0, |row| row.value)
    }

    /// Whether a server waits for approval to install its agent.
    pub fn awaiting_agent(&self, server: uuid::Uuid) -> bool {
        self.awaiting_agent.contains(&server)
    }
}

impl Snapshot {
    /// A saved server's connection scope in this snapshot: its connection
    /// settings, its connection revision and this backend instance.
    pub fn connection_scope(&self, server: &Server) -> ConnectionScope {
        ConnectionScope {
            settings: server.connection_settings(),
            connection_revision: self.runtime.connection_revision(server.id),
            backend_instance: self.instance_id,
        }
    }
}
