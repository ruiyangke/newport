//! Typed desktop connection contracts.
pub mod discovery;

/// Codes for why an SSH connection failed, which the window words.
pub mod code {
    pub const SSH_UNREACHABLE: &str = "SSH_UNREACHABLE";
    pub const SSH_PROXY_FAILED: &str = "SSH_PROXY_FAILED";
    pub const SSH_INVALID_PROXY: &str = "SSH_INVALID_PROXY";
    pub const SSH_INVALID_JUMP: &str = "SSH_INVALID_JUMP";
    pub const SSH_TIMED_OUT: &str = "SSH_TIMED_OUT";
    pub const SSH_LOST: &str = "SSH_LOST";
    pub const SSH_AGENT_UNAVAILABLE: &str = "SSH_AGENT_UNAVAILABLE";
    pub const SSH_APPROVAL_REQUIRED: &str = "SSH_APPROVAL_REQUIRED";
    pub const SSH_AUTH_REFUSED: &str = "SSH_AUTH_REFUSED";
    pub const SSH_CREDENTIALS: &str = "SSH_CREDENTIALS";
    pub const SSH_HOST_KEY_CHANGED: &str = "SSH_HOST_KEY_CHANGED";
    pub const SSH_HOST_KEY_REJECTED: &str = "SSH_HOST_KEY_REJECTED";
    pub const SSH_SESSIONS_DISABLED: &str = "SSH_SESSIONS_DISABLED";
    pub const SSH_CHANNEL_REFUSED: &str = "SSH_CHANNEL_REFUSED";
    pub const SSH_REQUEST_DENIED: &str = "SSH_REQUEST_DENIED";
    pub const SSH_BUSY: &str = "SSH_BUSY";
    pub const SSH_CLOSED: &str = "SSH_CLOSED";
}
#[derive(Clone, Debug)]
pub struct SetupInspection {
    pub os: String,
    pub architecture: String,
    pub cores: Option<u64>,
    pub memory_kib: Option<u64>,
    pub free_kib: Option<u64>,
    pub installed: bool,
    pub total: u64,
}

#[derive(Clone, Debug)]
pub struct OptionalNotice {
    pub notice: Option<String>,
}

#[derive(Clone, Debug)]
pub struct AgentKey {
    pub source: String,
    pub comment: String,
    pub fingerprint: String,
    pub algorithm: String,
}

#[derive(Clone, Debug)]
pub struct AgentKeyList {
    pub keys: Vec<crate::desktop::connection::AgentKey>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct ConfigHost {
    pub id: uuid::Uuid,
    pub alias: String,
    pub host: String,
    pub user: String,
    pub port: u16,
    pub identity_file: Option<String>,
    pub proxy: Option<crate::desktop::configuration::Proxy>,
    pub through: Vec<String>,
    pub importable: bool,
    pub suggested: bool,
    pub note: Option<String>,
}

#[derive(Clone, Debug)]
pub struct SshResolveArgs {
    pub host: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SshResolved {
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<String>,
    pub proxy_command: Option<String>,
    pub proxy_jump: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ConfigHosts {
    pub path: String,
    pub hosts: Vec<crate::desktop::connection::ConfigHost>,
}

#[derive(Clone, Debug)]
pub struct PortsObservation {
    pub data: Option<discovery::Discovery>,
    pub error: Option<String>,
    pub fetching: bool,
    pub stale: bool,
    pub revision: u64,
    pub updated_at: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct UninstallOutput {
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct SetupInspectArgs {
    pub expected_server: crate::desktop::configuration::Server,
}

#[derive(Clone, Debug)]
pub struct SetupInstallArgs {
    pub expected_server: crate::desktop::configuration::Server,
}

#[derive(Clone, Debug)]
pub struct SetupFinishArgs {
    pub expected_server: crate::desktop::configuration::Server,
    pub browser: bool,
    pub clipboard: bool,
}

#[derive(Clone, Debug)]
pub struct SetupSkipArgs {
    pub expected_server: crate::desktop::configuration::Server,
}

#[derive(Clone, Debug)]
pub struct TestConnectionArgs {
    pub server: uuid::Uuid,
}

#[derive(Clone, Debug)]
pub struct ConnectionsQueryArgs {
    pub server: uuid::Uuid,
    /// Defaults to false when absent on the wire.
    pub refresh: bool,
}

#[derive(Clone, Debug)]
pub struct ReinstallAgentArgs {
    pub server: uuid::Uuid,
}

#[derive(Clone, Debug)]
pub struct UninstallAgentArgs {
    pub expected_server: crate::desktop::configuration::Server,
}

#[derive(Clone, Debug)]
pub struct InspectAgentArgs {
    pub expected_server: crate::desktop::configuration::Server,
}

#[derive(Clone, Debug)]
pub enum Operation {
    SetupInspect(Box<crate::desktop::connection::SetupInspectArgs>),
    SetupInstall(Box<crate::desktop::connection::SetupInstallArgs>),
    SetupFinish(Box<crate::desktop::connection::SetupFinishArgs>),
    SetupSkip(Box<crate::desktop::connection::SetupSkipArgs>),
    SshAgentKeys(Box<crate::common::Present>),
    SshConfigHosts(Box<crate::common::Present>),
    TestConnection(Box<crate::desktop::connection::TestConnectionArgs>),
    ConnectionsQuery(Box<crate::desktop::connection::ConnectionsQueryArgs>),
    ReinstallAgent(Box<crate::desktop::connection::ReinstallAgentArgs>),
    UninstallAgent(Box<crate::desktop::connection::UninstallAgentArgs>),
    InspectAgent(Box<crate::desktop::connection::InspectAgentArgs>),
    SshResolve(Box<crate::desktop::connection::SshResolveArgs>),
}

#[derive(Clone, Debug)]
pub enum Reply {
    SetupInspect(Box<crate::desktop::connection::SetupInspection>),
    SetupInstall(Box<crate::common::Present>),
    SetupFinish(Box<crate::desktop::connection::OptionalNotice>),
    SetupSkip(Box<crate::common::Present>),
    SshAgentKeys(Box<crate::desktop::connection::AgentKeyList>),
    SshConfigHosts(Box<crate::desktop::connection::ConfigHosts>),
    TestConnection(Box<crate::common::Present>),
    ConnectionsQuery(Box<crate::desktop::connection::PortsObservation>),
    ReinstallAgent(Box<crate::common::Present>),
    UninstallAgent(Box<crate::desktop::connection::UninstallOutput>),
    InspectAgent(Box<crate::desktop::configuration::InstallationStatus>),
    SshResolve(Box<crate::desktop::connection::SshResolved>),
}
