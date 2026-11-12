//! Desktop operations and typed results, independent of application and persistence models.
use crate::common::Limits;
use std::{fmt, num::NonZeroU64, time::Duration};
mod records;
pub use records::*;
mod service;
mod state;
mod validate;
pub use state::{Allocation, Exchange, IdSequence, Phase, RequestContext, request_allocation};
mod flow;
pub use flow::{OpaqueFlow, OpaqueKind, TerminalFlow};
pub mod account;
pub mod applications;
pub mod configuration;
pub mod connection;
pub mod containers;
pub mod events;
pub mod files;
pub mod git;
pub mod host;
pub mod metrics;
pub mod preferences;
pub mod services;
pub mod stream;
pub mod stub;
pub mod terminal;
#[cfg(feature = "protobuf")]
pub mod wire;

pub const VERSION: u32 = 1;
#[cfg(feature = "protobuf")]
pub const CONTRACT_ID: &str = crate::encoding::protobuf::contracts::DESKTOP;

#[cfg(feature = "protobuf")]
pub fn require_contract_id(actual: Option<&str>) -> Result<(), crate::common::Error> {
    if actual != Some(CONTRACT_ID) {
        return Err(crate::common::Error {
            code: "incompatible".into(),
            message: "Newport app and backend use different protocols. Update and restart Newport."
                .into(),
            retry: crate::common::Retry::No,
        });
    }
    Ok(())
}
pub const MAX_REQUEST: usize = 1024 * 1024;
pub const MAX_SAVE_REQUEST: usize = 17 * 1024 * 1024;
pub const MAX_RESPONSE: usize = 32 * 1024 * 1024;
pub const MAX_DOCUMENT: usize = 16 * 1024 * 1024;
pub const MAX_TEXT: usize = 64 * 1024;
pub const MAX_PATH: usize = 32 * 1024;
pub const MAX_ITEMS: usize = 65536;
pub const MAX_DATA: usize = 64 * 1024;
pub const OPAQUE_CHUNK: usize = crate::worker::MAX_CHUNK;
pub const STDERR_LIMIT: usize = 64 * 1024;
pub const OUTPUT_CREDIT: u64 = 1024 * 1024;
pub const INPUT_WINDOW: u64 = 256 * 1024;
pub const HANDSHAKE_LIMITS: Limits = Limits {
    message: 4096,
    metadata: 4096,
};
pub const CONTROL_LIMITS: Limits = Limits {
    message: 16 * 1024,
    metadata: 16 * 1024,
};
pub const REQUEST_LIMITS: Limits = Limits {
    message: MAX_REQUEST,
    metadata: MAX_REQUEST,
};
pub const SAVE_LIMITS: Limits = Limits {
    message: MAX_SAVE_REQUEST,
    metadata: MAX_REQUEST,
};
pub const RESPONSE_LIMITS: Limits = Limits {
    message: MAX_RESPONSE,
    metadata: MAX_RESPONSE,
};
pub const TERMINAL_CLIENT_LIMITS: Limits = Limits {
    message: MAX_DATA + 1024,
    metadata: 1024,
};
pub const TERMINAL_SERVER_LIMITS: Limits = Limits {
    message: MAX_DOCUMENT + 1024,
    metadata: 16 * 1024,
};
pub const OPAQUE_LIMITS: Limits = Limits {
    message: STDERR_LIMIT + OPAQUE_CHUNK + 1024,
    metadata: 16 * 1024,
};
pub const CONNECTION_SLOTS: usize = 256;
pub const OPERATION_SLOTS: usize = 32;
pub const REVOCATION_SLOTS: usize = 4;
pub const LARGE_REQUEST_SLOTS: usize = 2;
pub const REFUSAL_SLOTS: usize = 8;
pub const ADMISSION_TIMEOUT: Duration = Duration::from_secs(10);
pub const REFUSAL_TIMEOUT: Duration = Duration::from_secs(5);
pub const RESPONSE_WRITE_TIMEOUT: Duration = Duration::from_secs(30);
pub const OPENED_WRITE_TIMEOUT: Duration = Duration::from_secs(10);
pub const CLOSURE_TIMEOUT: Duration = Duration::from_secs(5);
pub const CONSENT_HEARTBEAT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RequestId(NonZeroU64);
impl RequestId {
    pub fn new(value: u64) -> Result<Self, Invalid> {
        NonZeroU64::new(value)
            .map(Self)
            .ok_or(Invalid("zero request identifier"))
    }
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Invalid(pub &'static str);
impl fmt::Display for Invalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Invalid desktop {}", self.0)
    }
}
impl std::error::Error for Invalid {}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission {
    Ordinary,
    IntegrationOff,
    Stream,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisconnectPolicy {
    Cancel,
    Complete,
    Stream,
}
fn text(value: &str) -> Result<(), Invalid> {
    if value.len() > MAX_TEXT || value.contains('\0') {
        Err(Invalid("text"))
    } else {
        Ok(())
    }
}
fn count(value: usize) -> Result<(), Invalid> {
    bulk(value, MAX_ITEMS)
}
fn bulk(value: usize, limit: usize) -> Result<(), Invalid> {
    if value > limit {
        Err(Invalid("size"))
    } else {
        Ok(())
    }
}
fn unique(values: impl IntoIterator<Item = uuid::Uuid>) -> Result<(), Invalid> {
    let mut seen = std::collections::BTreeSet::new();
    for id in values {
        if !seen.insert(id) {
            return Err(Invalid("duplicate identity"));
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    AccountStatus,
    AccountSignIn,
    AccountCancelSignIn,
    AccountSignOut,
    AccountOpenPortal,
    AccountOpenBilling,
    AccountRefresh,
    AccountStartTrial,
    AccountRemoteLicense,
    ConnectionSetupInspect,
    ConnectionSetupInstall,
    ConnectionSetupFinish,
    ConnectionSetupSkip,
    ConnectionSshAgentKeys,
    ConnectionSshConfigHosts,
    ConnectionTestConnection,
    ConnectionConnectionsQuery,
    ConnectionReinstallAgent,
    ConnectionUninstallAgent,
    ConnectionInspectAgent,
    ConnectionSshResolve,
    ConfigurationSaveServer,
    ConfigurationDeleteServer,
    ConfigurationForwardDiscoveredPort,
    ConfigurationForwardTunnelBatch,
    ConfigurationSaveTunnel,
    ConfigurationUpdateTunnel,
    ConfigurationDeleteTunnel,
    ConfigurationDeleteTunnels,
    ConfigurationSetTunnelConnected,
    ConfigurationSetClipboardEnabled,
    ConfigurationSetIntegrationEnabled,
    ConfigurationOpenTunnel,
    ConfigurationOpenTerminalLink,
    ConfigurationSnapshot,
    FilesList,
    FilesWatch,
    FilesPreview,
    FilesReadText,
    FilesProjectIndex,
    FilesWriteText,
    FilesCancel,
    FilesProgress,
    FilesUpload,
    FilesDownload,
    MetricsCollect,
    MetricsHistory,
    MetricsCache,
    MetricsClearCache,
    ServicesSnapshot,
    ServicesLogs,
    ServicesUnitFile,
    ServicesAction,
    ContainersSnapshot,
    ContainersInspect,
    ContainersLogs,
    ContainersAction,
    ContainersProjectAction,
    ContainersRemove,
    ContainersDisk,
    ContainersPrune,
    ContainersComposeProject,
    ContainersComposeRun,
    ContainersComposeJob,
    ContainersComposeCancel,
    ContainersComposeJobs,
    PreferencesSetWindow,
    PreferencesProjectsList,
    PreferencesProjectsSave,
    PreferencesProjectsRemove,
    PreferencesSettings,
    PreferencesSettingsSave,
    PreferencesInstalled,
    PreferencesInstalledSave,
    GitRequest,
    GitRefreshRepository,
    GitPendingOperations,
    GitAcknowledgeOperation,
    GitReviewOperation,
    ApplicationsStatus,
    ApplicationsCatalogue,
    ApplicationsDisplay,
    ApplicationsLaunch,
    ApplicationsShow,
    ApplicationsStates,
    ApplicationsStop,
    HostActivate,
    HostOpenRemoteApp,
    HostStartupSettings,
    HostSetLaunchAtLogin,
    HostUpdateStatus,
    HostCheckForUpdates,
    HostInstallUpdate,
    EventsPoll,
    TerminalClose,
    TerminalList,
    TerminalRename,
    TerminalEnd,
    StreamTerminalOpen,
    StreamCommandOpen,
    StreamRemoteAppsOpen,
    StreamLspOpen,
}
impl account::Operation {
    pub fn kind(&self) -> Kind {
        match self {
            Self::Status(_) => Kind::AccountStatus,
            Self::SignIn(_) => Kind::AccountSignIn,
            Self::CancelSignIn(_) => Kind::AccountCancelSignIn,
            Self::SignOut(_) => Kind::AccountSignOut,
            Self::OpenPortal(_) => Kind::AccountOpenPortal,
            Self::OpenBilling(_) => Kind::AccountOpenBilling,
            Self::Refresh(_) => Kind::AccountRefresh,
            Self::StartTrial(_) => Kind::AccountStartTrial,
            Self::RemoteLicense(_) => Kind::AccountRemoteLicense,
        }
    }
    pub fn server_id(&self) -> Option<uuid::Uuid> {
        match self {
            Self::Status(_) => None,
            Self::SignIn(_) => None,
            Self::CancelSignIn(_) => None,
            Self::SignOut(_) => None,
            Self::OpenPortal(_) => None,
            Self::OpenBilling(_) => None,
            Self::Refresh(_) => None,
            Self::StartTrial(_) => None,
            Self::RemoteLicense(_) => None,
        }
    }
    pub fn admission(&self) -> Admission {
        match self {
            Self::Status(_) => Admission::Ordinary,
            Self::SignIn(_) => Admission::Ordinary,
            Self::CancelSignIn(_) => Admission::Ordinary,
            Self::SignOut(_) => Admission::Ordinary,
            Self::OpenPortal(_) => Admission::Ordinary,
            Self::OpenBilling(_) => Admission::Ordinary,
            Self::Refresh(_) => Admission::Ordinary,
            Self::StartTrial(_) => Admission::Ordinary,
            Self::RemoteLicense(_) => Admission::Ordinary,
        }
    }
    pub fn disconnect_policy(&self) -> DisconnectPolicy {
        match self {
            Self::Status(_) => DisconnectPolicy::Complete,
            Self::SignIn(_) => DisconnectPolicy::Complete,
            Self::CancelSignIn(_) => DisconnectPolicy::Complete,
            Self::SignOut(_) => DisconnectPolicy::Complete,
            Self::OpenPortal(_) => DisconnectPolicy::Complete,
            Self::OpenBilling(_) => DisconnectPolicy::Complete,
            Self::Refresh(_) => DisconnectPolicy::Complete,
            Self::StartTrial(_) => DisconnectPolicy::Complete,
            Self::RemoteLicense(_) => DisconnectPolicy::Complete,
        }
    }
    pub fn limits(&self) -> Limits {
        match self {
            Self::Status(_) => REQUEST_LIMITS,
            Self::SignIn(_) => REQUEST_LIMITS,
            Self::CancelSignIn(_) => REQUEST_LIMITS,
            Self::SignOut(_) => REQUEST_LIMITS,
            Self::OpenPortal(_) => REQUEST_LIMITS,
            Self::OpenBilling(_) => REQUEST_LIMITS,
            Self::Refresh(_) => REQUEST_LIMITS,
            Self::StartTrial(_) => REQUEST_LIMITS,
            Self::RemoteLicense(_) => REQUEST_LIMITS,
        }
    }
}
impl account::Reply {
    pub fn kind(&self) -> Kind {
        match self {
            Self::Status(_) => Kind::AccountStatus,
            Self::SignIn(_) => Kind::AccountSignIn,
            Self::CancelSignIn(_) => Kind::AccountCancelSignIn,
            Self::SignOut(_) => Kind::AccountSignOut,
            Self::OpenPortal(_) => Kind::AccountOpenPortal,
            Self::OpenBilling(_) => Kind::AccountOpenBilling,
            Self::Refresh(_) => Kind::AccountRefresh,
            Self::StartTrial(_) => Kind::AccountStartTrial,
            Self::RemoteLicense(_) => Kind::AccountRemoteLicense,
        }
    }
    pub fn validate_for(&self, operation: &account::Operation) -> Result<(), Invalid> {
        self.validate()?;
        match (self, operation) {
            (Self::Status(_), account::Operation::Status(_)) => Ok(()),
            (Self::SignIn(_), account::Operation::SignIn(_)) => Ok(()),
            (Self::CancelSignIn(_), account::Operation::CancelSignIn(_)) => Ok(()),
            (Self::SignOut(_), account::Operation::SignOut(_)) => Ok(()),
            (Self::OpenPortal(_), account::Operation::OpenPortal(_)) => Ok(()),
            (Self::OpenBilling(_), account::Operation::OpenBilling(_)) => Ok(()),
            (Self::Refresh(_), account::Operation::Refresh(_)) => Ok(()),
            (Self::StartTrial(_), account::Operation::StartTrial(_)) => Ok(()),
            (Self::RemoteLicense(_), account::Operation::RemoteLicense(_)) => Ok(()),
            _ => Err(Invalid("response operation")),
        }
    }
}
impl connection::Operation {
    pub fn kind(&self) -> Kind {
        match self {
            Self::SetupInspect(_) => Kind::ConnectionSetupInspect,
            Self::SetupInstall(_) => Kind::ConnectionSetupInstall,
            Self::SetupFinish(_) => Kind::ConnectionSetupFinish,
            Self::SetupSkip(_) => Kind::ConnectionSetupSkip,
            Self::SshAgentKeys(_) => Kind::ConnectionSshAgentKeys,
            Self::SshConfigHosts(_) => Kind::ConnectionSshConfigHosts,
            Self::TestConnection(_) => Kind::ConnectionTestConnection,
            Self::ConnectionsQuery(_) => Kind::ConnectionConnectionsQuery,
            Self::ReinstallAgent(_) => Kind::ConnectionReinstallAgent,
            Self::UninstallAgent(_) => Kind::ConnectionUninstallAgent,
            Self::InspectAgent(_) => Kind::ConnectionInspectAgent,
            Self::SshResolve(_) => Kind::ConnectionSshResolve,
        }
    }
    pub fn server_id(&self) -> Option<uuid::Uuid> {
        match self {
            Self::SetupInspect(value) => Some(value.expected_server.id),
            Self::SetupInstall(value) => Some(value.expected_server.id),
            Self::SetupFinish(value) => Some(value.expected_server.id),
            Self::SetupSkip(value) => Some(value.expected_server.id),
            Self::SshAgentKeys(_) => None,
            Self::SshConfigHosts(_) => None,
            Self::TestConnection(value) => Some(value.server),
            Self::ConnectionsQuery(value) => Some(value.server),
            Self::ReinstallAgent(value) => Some(value.server),
            Self::UninstallAgent(value) => Some(value.expected_server.id),
            Self::InspectAgent(value) => Some(value.expected_server.id),
            Self::SshResolve(_) => None,
        }
    }
    pub fn admission(&self) -> Admission {
        match self {
            Self::SetupInspect(_) => Admission::Ordinary,
            Self::SetupInstall(_) => Admission::Ordinary,
            Self::SetupFinish(_) => Admission::Ordinary,
            Self::SetupSkip(_) => Admission::Ordinary,
            Self::SshAgentKeys(_) => Admission::Ordinary,
            Self::SshConfigHosts(_) => Admission::Ordinary,
            Self::TestConnection(_) => Admission::Ordinary,
            Self::ConnectionsQuery(_) => Admission::Ordinary,
            Self::ReinstallAgent(_) => Admission::Ordinary,
            Self::UninstallAgent(_) => Admission::Ordinary,
            Self::InspectAgent(_) => Admission::Ordinary,
            Self::SshResolve(_) => Admission::Ordinary,
        }
    }
    pub fn disconnect_policy(&self) -> DisconnectPolicy {
        match self {
            Self::SetupInspect(_) => DisconnectPolicy::Cancel,
            Self::SetupInstall(_) => DisconnectPolicy::Complete,
            Self::SetupFinish(_) => DisconnectPolicy::Complete,
            Self::SetupSkip(_) => DisconnectPolicy::Complete,
            Self::SshAgentKeys(_) => DisconnectPolicy::Complete,
            Self::SshConfigHosts(_) => DisconnectPolicy::Complete,
            Self::TestConnection(_) => DisconnectPolicy::Cancel,
            Self::ConnectionsQuery(_) => DisconnectPolicy::Complete,
            Self::ReinstallAgent(_) => DisconnectPolicy::Complete,
            Self::UninstallAgent(_) => DisconnectPolicy::Complete,
            Self::InspectAgent(_) => DisconnectPolicy::Complete,
            Self::SshResolve(_) => DisconnectPolicy::Complete,
        }
    }
    pub fn limits(&self) -> Limits {
        match self {
            Self::SetupInspect(_) => REQUEST_LIMITS,
            Self::SetupInstall(_) => REQUEST_LIMITS,
            Self::SetupFinish(_) => REQUEST_LIMITS,
            Self::SetupSkip(_) => REQUEST_LIMITS,
            Self::SshAgentKeys(_) => REQUEST_LIMITS,
            Self::SshConfigHosts(_) => REQUEST_LIMITS,
            Self::TestConnection(_) => REQUEST_LIMITS,
            Self::ConnectionsQuery(_) => REQUEST_LIMITS,
            Self::ReinstallAgent(_) => REQUEST_LIMITS,
            Self::UninstallAgent(_) => REQUEST_LIMITS,
            Self::InspectAgent(_) => REQUEST_LIMITS,
            Self::SshResolve(_) => REQUEST_LIMITS,
        }
    }
}
impl connection::Reply {
    pub fn kind(&self) -> Kind {
        match self {
            Self::SetupInspect(_) => Kind::ConnectionSetupInspect,
            Self::SetupInstall(_) => Kind::ConnectionSetupInstall,
            Self::SetupFinish(_) => Kind::ConnectionSetupFinish,
            Self::SetupSkip(_) => Kind::ConnectionSetupSkip,
            Self::SshAgentKeys(_) => Kind::ConnectionSshAgentKeys,
            Self::SshConfigHosts(_) => Kind::ConnectionSshConfigHosts,
            Self::TestConnection(_) => Kind::ConnectionTestConnection,
            Self::ConnectionsQuery(_) => Kind::ConnectionConnectionsQuery,
            Self::ReinstallAgent(_) => Kind::ConnectionReinstallAgent,
            Self::UninstallAgent(_) => Kind::ConnectionUninstallAgent,
            Self::InspectAgent(_) => Kind::ConnectionInspectAgent,
            Self::SshResolve(_) => Kind::ConnectionSshResolve,
        }
    }
    pub fn validate_for(&self, operation: &connection::Operation) -> Result<(), Invalid> {
        self.validate()?;
        match (self, operation) {
            (Self::SetupInspect(_), connection::Operation::SetupInspect(_)) => Ok(()),
            (Self::SetupInstall(_), connection::Operation::SetupInstall(_)) => Ok(()),
            (Self::SetupFinish(_), connection::Operation::SetupFinish(_)) => Ok(()),
            (Self::SetupSkip(_), connection::Operation::SetupSkip(_)) => Ok(()),
            (Self::SshAgentKeys(_), connection::Operation::SshAgentKeys(_)) => Ok(()),
            (Self::SshConfigHosts(_), connection::Operation::SshConfigHosts(_)) => Ok(()),
            (Self::TestConnection(_), connection::Operation::TestConnection(_)) => Ok(()),
            (Self::ConnectionsQuery(_), connection::Operation::ConnectionsQuery(_)) => Ok(()),
            (Self::ReinstallAgent(_), connection::Operation::ReinstallAgent(_)) => Ok(()),
            (Self::UninstallAgent(_), connection::Operation::UninstallAgent(_)) => Ok(()),
            (Self::InspectAgent(_), connection::Operation::InspectAgent(_)) => Ok(()),
            (Self::SshResolve(_), connection::Operation::SshResolve(_)) => Ok(()),
            _ => Err(Invalid("response operation")),
        }
    }
}
impl configuration::Operation {
    pub fn kind(&self) -> Kind {
        match self {
            Self::SaveServer(_) => Kind::ConfigurationSaveServer,
            Self::DeleteServer(_) => Kind::ConfigurationDeleteServer,
            Self::ForwardDiscoveredPort(_) => Kind::ConfigurationForwardDiscoveredPort,
            Self::ForwardTunnelBatch(_) => Kind::ConfigurationForwardTunnelBatch,
            Self::SaveTunnel(_) => Kind::ConfigurationSaveTunnel,
            Self::UpdateTunnel(_) => Kind::ConfigurationUpdateTunnel,
            Self::DeleteTunnel(_) => Kind::ConfigurationDeleteTunnel,
            Self::DeleteTunnels(_) => Kind::ConfigurationDeleteTunnels,
            Self::SetTunnelConnected(_) => Kind::ConfigurationSetTunnelConnected,
            Self::SetClipboardEnabled(_) => Kind::ConfigurationSetClipboardEnabled,
            Self::SetIntegrationEnabled(_) => Kind::ConfigurationSetIntegrationEnabled,
            Self::OpenTunnel(_) => Kind::ConfigurationOpenTunnel,
            Self::OpenTerminalLink(_) => Kind::ConfigurationOpenTerminalLink,
            Self::Snapshot(_) => Kind::ConfigurationSnapshot,
        }
    }
    pub fn server_id(&self) -> Option<uuid::Uuid> {
        match self {
            Self::SaveServer(value) => Some(value.server.id),
            Self::DeleteServer(value) => Some(value.server),
            Self::ForwardDiscoveredPort(value) => Some(value.server),
            Self::ForwardTunnelBatch(value) => Some(value.server),
            Self::SaveTunnel(value) => Some(value.tunnel.server_id),
            Self::UpdateTunnel(value) => Some(value.tunnel.server_id),
            Self::DeleteTunnel(_) => None,
            Self::DeleteTunnels(_) => None,
            Self::SetTunnelConnected(_) => None,
            Self::SetClipboardEnabled(value) => Some(value.server),
            Self::SetIntegrationEnabled(value) => Some(value.server),
            Self::OpenTunnel(_) => None,
            Self::OpenTerminalLink(_) => None,
            Self::Snapshot(_) => None,
        }
    }
    pub fn admission(&self) -> Admission {
        match self {
            Self::SaveServer(_) => Admission::Ordinary,
            Self::DeleteServer(_) => Admission::Ordinary,
            Self::ForwardDiscoveredPort(_) => Admission::Ordinary,
            Self::ForwardTunnelBatch(_) => Admission::Ordinary,
            Self::SaveTunnel(_) => Admission::Ordinary,
            Self::UpdateTunnel(_) => Admission::Ordinary,
            Self::DeleteTunnel(_) => Admission::Ordinary,
            Self::DeleteTunnels(_) => Admission::Ordinary,
            Self::SetTunnelConnected(_) => Admission::Ordinary,
            Self::SetClipboardEnabled(value) => {
                if !value.enabled {
                    Admission::IntegrationOff
                } else {
                    Admission::Ordinary
                }
            }
            Self::SetIntegrationEnabled(value) => {
                if !value.enabled {
                    Admission::IntegrationOff
                } else {
                    Admission::Ordinary
                }
            }
            Self::OpenTunnel(_) => Admission::Ordinary,
            Self::OpenTerminalLink(_) => Admission::Ordinary,
            Self::Snapshot(_) => Admission::Ordinary,
        }
    }
    pub fn disconnect_policy(&self) -> DisconnectPolicy {
        match self {
            Self::SaveServer(_) => DisconnectPolicy::Complete,
            Self::DeleteServer(_) => DisconnectPolicy::Complete,
            Self::ForwardDiscoveredPort(_) => DisconnectPolicy::Complete,
            Self::ForwardTunnelBatch(_) => DisconnectPolicy::Complete,
            Self::SaveTunnel(_) => DisconnectPolicy::Complete,
            Self::UpdateTunnel(_) => DisconnectPolicy::Complete,
            Self::DeleteTunnel(_) => DisconnectPolicy::Complete,
            Self::DeleteTunnels(_) => DisconnectPolicy::Complete,
            Self::SetTunnelConnected(_) => DisconnectPolicy::Complete,
            Self::SetClipboardEnabled(_) => DisconnectPolicy::Complete,
            Self::SetIntegrationEnabled(_) => DisconnectPolicy::Complete,
            Self::OpenTunnel(_) => DisconnectPolicy::Complete,
            Self::OpenTerminalLink(_) => DisconnectPolicy::Complete,
            Self::Snapshot(_) => DisconnectPolicy::Complete,
        }
    }
    pub fn limits(&self) -> Limits {
        match self {
            Self::SaveServer(_) => REQUEST_LIMITS,
            Self::DeleteServer(_) => REQUEST_LIMITS,
            Self::ForwardDiscoveredPort(_) => REQUEST_LIMITS,
            Self::ForwardTunnelBatch(_) => REQUEST_LIMITS,
            Self::SaveTunnel(_) => REQUEST_LIMITS,
            Self::UpdateTunnel(_) => REQUEST_LIMITS,
            Self::DeleteTunnel(_) => REQUEST_LIMITS,
            Self::DeleteTunnels(_) => REQUEST_LIMITS,
            Self::SetTunnelConnected(_) => REQUEST_LIMITS,
            Self::SetClipboardEnabled(_) => REQUEST_LIMITS,
            Self::SetIntegrationEnabled(_) => REQUEST_LIMITS,
            Self::OpenTunnel(_) => REQUEST_LIMITS,
            Self::OpenTerminalLink(_) => REQUEST_LIMITS,
            Self::Snapshot(_) => REQUEST_LIMITS,
        }
    }
}
impl configuration::Reply {
    pub fn kind(&self) -> Kind {
        match self {
            Self::SaveServer(_) => Kind::ConfigurationSaveServer,
            Self::DeleteServer(_) => Kind::ConfigurationDeleteServer,
            Self::ForwardDiscoveredPort(_) => Kind::ConfigurationForwardDiscoveredPort,
            Self::ForwardTunnelBatch(_) => Kind::ConfigurationForwardTunnelBatch,
            Self::SaveTunnel(_) => Kind::ConfigurationSaveTunnel,
            Self::UpdateTunnel(_) => Kind::ConfigurationUpdateTunnel,
            Self::DeleteTunnel(_) => Kind::ConfigurationDeleteTunnel,
            Self::DeleteTunnels(_) => Kind::ConfigurationDeleteTunnels,
            Self::SetTunnelConnected(_) => Kind::ConfigurationSetTunnelConnected,
            Self::SetClipboardEnabled(_) => Kind::ConfigurationSetClipboardEnabled,
            Self::SetIntegrationEnabled(_) => Kind::ConfigurationSetIntegrationEnabled,
            Self::OpenTunnel(_) => Kind::ConfigurationOpenTunnel,
            Self::OpenTerminalLink(_) => Kind::ConfigurationOpenTerminalLink,
            Self::Snapshot(_) => Kind::ConfigurationSnapshot,
        }
    }
    pub fn validate_for(&self, operation: &configuration::Operation) -> Result<(), Invalid> {
        self.validate()?;
        match (self, operation) {
            (Self::SaveServer(_), configuration::Operation::SaveServer(_)) => Ok(()),
            (Self::DeleteServer(_), configuration::Operation::DeleteServer(_)) => Ok(()),
            (
                Self::ForwardDiscoveredPort(_),
                configuration::Operation::ForwardDiscoveredPort(_),
            ) => Ok(()),
            (Self::ForwardTunnelBatch(_), configuration::Operation::ForwardTunnelBatch(_)) => {
                Ok(())
            }
            (Self::SaveTunnel(_), configuration::Operation::SaveTunnel(_)) => Ok(()),
            (Self::UpdateTunnel(_), configuration::Operation::UpdateTunnel(_)) => Ok(()),
            (Self::DeleteTunnel(_), configuration::Operation::DeleteTunnel(_)) => Ok(()),
            (Self::DeleteTunnels(_), configuration::Operation::DeleteTunnels(_)) => Ok(()),
            (Self::SetTunnelConnected(_), configuration::Operation::SetTunnelConnected(_)) => {
                Ok(())
            }
            (Self::SetClipboardEnabled(_), configuration::Operation::SetClipboardEnabled(_)) => {
                Ok(())
            }
            (
                Self::SetIntegrationEnabled(_),
                configuration::Operation::SetIntegrationEnabled(_),
            ) => Ok(()),
            (Self::OpenTunnel(_), configuration::Operation::OpenTunnel(_)) => Ok(()),
            (Self::OpenTerminalLink(_), configuration::Operation::OpenTerminalLink(_)) => Ok(()),
            (Self::Snapshot(_), configuration::Operation::Snapshot(_)) => Ok(()),
            _ => Err(Invalid("response operation")),
        }
    }
}
impl files::Operation {
    pub fn kind(&self) -> Kind {
        match self {
            Self::List(_) => Kind::FilesList,
            Self::Watch(_) => Kind::FilesWatch,
            Self::Preview(_) => Kind::FilesPreview,
            Self::ReadText(_) => Kind::FilesReadText,
            Self::ProjectIndex(_) => Kind::FilesProjectIndex,
            Self::WriteText(_) => Kind::FilesWriteText,
            Self::Cancel(_) => Kind::FilesCancel,
            Self::Progress(_) => Kind::FilesProgress,
            Self::Upload(_) => Kind::FilesUpload,
            Self::Download(_) => Kind::FilesDownload,
        }
    }
    pub fn server_id(&self) -> Option<uuid::Uuid> {
        match self {
            Self::List(value) => Some(value.server),
            Self::Watch(value) => Some(value.server),
            Self::Preview(value) => Some(value.server),
            Self::ReadText(value) => Some(value.server),
            Self::ProjectIndex(value) => Some(value.server),
            Self::WriteText(value) => Some(value.server),
            Self::Cancel(_) => None,
            Self::Progress(_) => None,
            Self::Upload(value) => Some(value.server),
            Self::Download(value) => Some(value.server),
        }
    }
    pub fn admission(&self) -> Admission {
        match self {
            Self::List(_) => Admission::Ordinary,
            Self::Watch(_) => Admission::Ordinary,
            Self::Preview(_) => Admission::Ordinary,
            Self::ReadText(_) => Admission::Ordinary,
            Self::ProjectIndex(_) => Admission::Ordinary,
            Self::WriteText(_) => Admission::Ordinary,
            Self::Cancel(_) => Admission::Ordinary,
            Self::Progress(_) => Admission::Ordinary,
            Self::Upload(_) => Admission::Ordinary,
            Self::Download(_) => Admission::Ordinary,
        }
    }
    pub fn disconnect_policy(&self) -> DisconnectPolicy {
        match self {
            Self::List(_) => DisconnectPolicy::Cancel,
            Self::Watch(_) => DisconnectPolicy::Cancel,
            Self::Preview(_) => DisconnectPolicy::Cancel,
            Self::ReadText(_) => DisconnectPolicy::Cancel,
            Self::ProjectIndex(_) => DisconnectPolicy::Cancel,
            Self::WriteText(_) => DisconnectPolicy::Cancel,
            Self::Cancel(_) => DisconnectPolicy::Complete,
            Self::Progress(_) => DisconnectPolicy::Complete,
            Self::Upload(_) => DisconnectPolicy::Cancel,
            Self::Download(_) => DisconnectPolicy::Cancel,
        }
    }
    pub fn limits(&self) -> Limits {
        match self {
            Self::List(_) => REQUEST_LIMITS,
            Self::Watch(_) => REQUEST_LIMITS,
            Self::Preview(_) => REQUEST_LIMITS,
            Self::ReadText(_) => REQUEST_LIMITS,
            Self::ProjectIndex(_) => REQUEST_LIMITS,
            Self::WriteText(_) => SAVE_LIMITS,
            Self::Cancel(_) => REQUEST_LIMITS,
            Self::Progress(_) => REQUEST_LIMITS,
            Self::Upload(_) => REQUEST_LIMITS,
            Self::Download(_) => REQUEST_LIMITS,
        }
    }
}
impl files::Reply {
    pub fn kind(&self) -> Kind {
        match self {
            Self::List(_) => Kind::FilesList,
            Self::Watch(_) => Kind::FilesWatch,
            Self::Preview(_) => Kind::FilesPreview,
            Self::ReadText(_) => Kind::FilesReadText,
            Self::ProjectIndex(_) => Kind::FilesProjectIndex,
            Self::WriteText(_) => Kind::FilesWriteText,
            Self::Cancel(_) => Kind::FilesCancel,
            Self::Progress(_) => Kind::FilesProgress,
            Self::Upload(_) => Kind::FilesUpload,
            Self::Download(_) => Kind::FilesDownload,
        }
    }
    pub fn validate_for(&self, operation: &files::Operation) -> Result<(), Invalid> {
        self.validate()?;
        match (self, operation) {
            (Self::List(_), files::Operation::List(_)) => Ok(()),
            (Self::Watch(_), files::Operation::Watch(_)) => Ok(()),
            (Self::Preview(_), files::Operation::Preview(_)) => Ok(()),
            (Self::ReadText(_), files::Operation::ReadText(_)) => Ok(()),
            (Self::ProjectIndex(_), files::Operation::ProjectIndex(_)) => Ok(()),
            (Self::WriteText(_), files::Operation::WriteText(_)) => Ok(()),
            (Self::Cancel(_), files::Operation::Cancel(_)) => Ok(()),
            (Self::Progress(_), files::Operation::Progress(_)) => Ok(()),
            (Self::Upload(_), files::Operation::Upload(_)) => Ok(()),
            (Self::Download(_), files::Operation::Download(_)) => Ok(()),
            _ => Err(Invalid("response operation")),
        }
    }
}
impl metrics::Operation {
    pub fn kind(&self) -> Kind {
        match self {
            Self::Collect(_) => Kind::MetricsCollect,
            Self::History(_) => Kind::MetricsHistory,
            Self::Cache(_) => Kind::MetricsCache,
            Self::ClearCache(_) => Kind::MetricsClearCache,
        }
    }
    pub fn server_id(&self) -> Option<uuid::Uuid> {
        match self {
            Self::Collect(value) => Some(value.server),
            Self::History(value) => Some(value.server),
            Self::Cache(_) => None,
            Self::ClearCache(_) => None,
        }
    }
    pub fn admission(&self) -> Admission {
        match self {
            Self::Collect(_) => Admission::Ordinary,
            Self::History(_) => Admission::Ordinary,
            Self::Cache(_) => Admission::Ordinary,
            Self::ClearCache(_) => Admission::Ordinary,
        }
    }
    pub fn disconnect_policy(&self) -> DisconnectPolicy {
        match self {
            Self::Collect(_) => DisconnectPolicy::Cancel,
            Self::History(_) => DisconnectPolicy::Complete,
            Self::Cache(_) => DisconnectPolicy::Complete,
            Self::ClearCache(_) => DisconnectPolicy::Complete,
        }
    }
    pub fn limits(&self) -> Limits {
        match self {
            Self::Collect(_) => REQUEST_LIMITS,
            Self::History(_) => REQUEST_LIMITS,
            Self::Cache(_) => REQUEST_LIMITS,
            Self::ClearCache(_) => REQUEST_LIMITS,
        }
    }
}
impl metrics::Reply {
    pub fn kind(&self) -> Kind {
        match self {
            Self::Collect(_) => Kind::MetricsCollect,
            Self::History(_) => Kind::MetricsHistory,
            Self::Cache(_) => Kind::MetricsCache,
            Self::ClearCache(_) => Kind::MetricsClearCache,
        }
    }
    pub fn validate_for(&self, operation: &metrics::Operation) -> Result<(), Invalid> {
        self.validate()?;
        match (self, operation) {
            (Self::Collect(_), metrics::Operation::Collect(_)) => Ok(()),
            (Self::History(_), metrics::Operation::History(_)) => Ok(()),
            (Self::Cache(_), metrics::Operation::Cache(_)) => Ok(()),
            (Self::ClearCache(_), metrics::Operation::ClearCache(_)) => Ok(()),
            _ => Err(Invalid("response operation")),
        }
    }
}
impl services::Operation {
    pub fn kind(&self) -> Kind {
        match self {
            Self::Snapshot(_) => Kind::ServicesSnapshot,
            Self::Logs(_) => Kind::ServicesLogs,
            Self::UnitFile(_) => Kind::ServicesUnitFile,
            Self::Action(_) => Kind::ServicesAction,
        }
    }
    pub fn server_id(&self) -> Option<uuid::Uuid> {
        match self {
            Self::Snapshot(value) => Some(value.server),
            Self::Logs(value) => Some(value.server),
            Self::UnitFile(value) => Some(value.server),
            Self::Action(value) => Some(value.server),
        }
    }
    pub fn admission(&self) -> Admission {
        match self {
            Self::Snapshot(_) => Admission::Ordinary,
            Self::Logs(_) => Admission::Ordinary,
            Self::UnitFile(_) => Admission::Ordinary,
            Self::Action(_) => Admission::Ordinary,
        }
    }
    pub fn disconnect_policy(&self) -> DisconnectPolicy {
        match self {
            Self::Snapshot(_) => DisconnectPolicy::Cancel,
            Self::Logs(_) => DisconnectPolicy::Cancel,
            Self::UnitFile(_) => DisconnectPolicy::Cancel,
            Self::Action(_) => DisconnectPolicy::Complete,
        }
    }
    pub fn limits(&self) -> Limits {
        match self {
            Self::Snapshot(_) => REQUEST_LIMITS,
            Self::Logs(_) => REQUEST_LIMITS,
            Self::UnitFile(_) => REQUEST_LIMITS,
            Self::Action(_) => REQUEST_LIMITS,
        }
    }
}
impl services::Reply {
    pub fn kind(&self) -> Kind {
        match self {
            Self::Snapshot(_) => Kind::ServicesSnapshot,
            Self::Logs(_) => Kind::ServicesLogs,
            Self::UnitFile(_) => Kind::ServicesUnitFile,
            Self::Action(_) => Kind::ServicesAction,
        }
    }
    pub fn validate_for(&self, operation: &services::Operation) -> Result<(), Invalid> {
        self.validate()?;
        match (self, operation) {
            (Self::Snapshot(_), services::Operation::Snapshot(_)) => Ok(()),
            (Self::Logs(_), services::Operation::Logs(_)) => Ok(()),
            (Self::UnitFile(_), services::Operation::UnitFile(_)) => Ok(()),
            (Self::Action(_), services::Operation::Action(_)) => Ok(()),
            _ => Err(Invalid("response operation")),
        }
    }
}
impl containers::Operation {
    pub fn kind(&self) -> Kind {
        match self {
            Self::Snapshot(_) => Kind::ContainersSnapshot,
            Self::Inspect(_) => Kind::ContainersInspect,
            Self::Logs(_) => Kind::ContainersLogs,
            Self::Action(_) => Kind::ContainersAction,
            Self::ProjectAction(_) => Kind::ContainersProjectAction,
            Self::Remove(_) => Kind::ContainersRemove,
            Self::Disk(_) => Kind::ContainersDisk,
            Self::Prune(_) => Kind::ContainersPrune,
            Self::ComposeProject(_) => Kind::ContainersComposeProject,
            Self::ComposeRun(_) => Kind::ContainersComposeRun,
            Self::ComposeJob(_) => Kind::ContainersComposeJob,
            Self::ComposeCancel(_) => Kind::ContainersComposeCancel,
            Self::ComposeJobs(_) => Kind::ContainersComposeJobs,
        }
    }
    pub fn server_id(&self) -> Option<uuid::Uuid> {
        match self {
            Self::Snapshot(value) => Some(value.server),
            Self::Inspect(value) => Some(value.server),
            Self::Logs(value) => Some(value.server),
            Self::Action(value) => Some(value.server),
            Self::ProjectAction(value) => Some(value.server),
            Self::Remove(value) => Some(value.server),
            Self::Disk(value) => Some(value.server),
            Self::Prune(value) => Some(value.server),
            Self::ComposeProject(value) => Some(value.server),
            Self::ComposeRun(value) => Some(value.server),
            Self::ComposeJob(value) => Some(value.server),
            Self::ComposeCancel(value) => Some(value.server),
            Self::ComposeJobs(value) => Some(value.server),
        }
    }
    pub fn admission(&self) -> Admission {
        match self {
            Self::Snapshot(_) => Admission::Ordinary,
            Self::Inspect(_) => Admission::Ordinary,
            Self::Logs(_) => Admission::Ordinary,
            Self::Action(_) => Admission::Ordinary,
            Self::ProjectAction(_) => Admission::Ordinary,
            Self::Remove(_) => Admission::Ordinary,
            Self::Disk(_) => Admission::Ordinary,
            Self::Prune(_) => Admission::Ordinary,
            Self::ComposeProject(_) => Admission::Ordinary,
            Self::ComposeRun(_) => Admission::Ordinary,
            Self::ComposeJob(_) => Admission::Ordinary,
            Self::ComposeCancel(_) => Admission::Ordinary,
            Self::ComposeJobs(_) => Admission::Ordinary,
        }
    }
    pub fn disconnect_policy(&self) -> DisconnectPolicy {
        match self {
            Self::Snapshot(_) => DisconnectPolicy::Cancel,
            Self::Inspect(_) => DisconnectPolicy::Cancel,
            Self::Logs(_) => DisconnectPolicy::Cancel,
            Self::Action(_) => DisconnectPolicy::Complete,
            Self::ProjectAction(_) => DisconnectPolicy::Complete,
            Self::Remove(_) => DisconnectPolicy::Complete,
            Self::Disk(_) => DisconnectPolicy::Cancel,
            Self::Prune(_) => DisconnectPolicy::Complete,
            Self::ComposeProject(_) => DisconnectPolicy::Cancel,
            Self::ComposeRun(_) => DisconnectPolicy::Complete,
            Self::ComposeJob(_) => DisconnectPolicy::Cancel,
            Self::ComposeCancel(_) => DisconnectPolicy::Complete,
            Self::ComposeJobs(_) => DisconnectPolicy::Cancel,
        }
    }
    pub fn limits(&self) -> Limits {
        match self {
            Self::Snapshot(_) => REQUEST_LIMITS,
            Self::Inspect(_) => REQUEST_LIMITS,
            Self::Logs(_) => REQUEST_LIMITS,
            Self::Action(_) => REQUEST_LIMITS,
            Self::ProjectAction(_) => REQUEST_LIMITS,
            Self::Remove(_) => REQUEST_LIMITS,
            Self::Disk(_) => REQUEST_LIMITS,
            Self::Prune(_) => REQUEST_LIMITS,
            Self::ComposeProject(_) => REQUEST_LIMITS,
            Self::ComposeRun(_) => REQUEST_LIMITS,
            Self::ComposeJob(_) => REQUEST_LIMITS,
            Self::ComposeCancel(_) => REQUEST_LIMITS,
            Self::ComposeJobs(_) => REQUEST_LIMITS,
        }
    }
}
impl containers::Reply {
    pub fn kind(&self) -> Kind {
        match self {
            Self::Snapshot(_) => Kind::ContainersSnapshot,
            Self::Inspect(_) => Kind::ContainersInspect,
            Self::Logs(_) => Kind::ContainersLogs,
            Self::Action(_) => Kind::ContainersAction,
            Self::ProjectAction(_) => Kind::ContainersProjectAction,
            Self::Remove(_) => Kind::ContainersRemove,
            Self::Disk(_) => Kind::ContainersDisk,
            Self::Prune(_) => Kind::ContainersPrune,
            Self::ComposeProject(_) => Kind::ContainersComposeProject,
            Self::ComposeRun(_) => Kind::ContainersComposeRun,
            Self::ComposeJob(_) => Kind::ContainersComposeJob,
            Self::ComposeCancel(_) => Kind::ContainersComposeCancel,
            Self::ComposeJobs(_) => Kind::ContainersComposeJobs,
        }
    }
    pub fn validate_for(&self, operation: &containers::Operation) -> Result<(), Invalid> {
        self.validate()?;
        match (self, operation) {
            (Self::Snapshot(_), containers::Operation::Snapshot(_)) => Ok(()),
            (Self::Inspect(_), containers::Operation::Inspect(_)) => Ok(()),
            (Self::Logs(_), containers::Operation::Logs(_)) => Ok(()),
            (Self::Action(_), containers::Operation::Action(_)) => Ok(()),
            (Self::ProjectAction(_), containers::Operation::ProjectAction(_)) => Ok(()),
            (Self::Remove(_), containers::Operation::Remove(_)) => Ok(()),
            (Self::Disk(_), containers::Operation::Disk(_)) => Ok(()),
            (Self::Prune(_), containers::Operation::Prune(_)) => Ok(()),
            (Self::ComposeProject(_), containers::Operation::ComposeProject(_)) => Ok(()),
            (Self::ComposeRun(_), containers::Operation::ComposeRun(_)) => Ok(()),
            (Self::ComposeJob(_), containers::Operation::ComposeJob(_)) => Ok(()),
            (Self::ComposeCancel(_), containers::Operation::ComposeCancel(_)) => Ok(()),
            (Self::ComposeJobs(_), containers::Operation::ComposeJobs(_)) => Ok(()),
            _ => Err(Invalid("response operation")),
        }
    }
}
impl preferences::Operation {
    pub fn kind(&self) -> Kind {
        match self {
            Self::SetWindow(_) => Kind::PreferencesSetWindow,
            Self::ProjectsList(_) => Kind::PreferencesProjectsList,
            Self::ProjectsSave(_) => Kind::PreferencesProjectsSave,
            Self::ProjectsRemove(_) => Kind::PreferencesProjectsRemove,
            Self::Settings(_) => Kind::PreferencesSettings,
            Self::SettingsSave(_) => Kind::PreferencesSettingsSave,
            Self::Installed(_) => Kind::PreferencesInstalled,
            Self::InstalledSave(_) => Kind::PreferencesInstalledSave,
        }
    }
    pub fn server_id(&self) -> Option<uuid::Uuid> {
        match self {
            Self::SetWindow(_) => None,
            Self::ProjectsList(value) => Some(value.server),
            Self::ProjectsSave(value) => Some(value.project.server),
            Self::ProjectsRemove(value) => Some(value.server),
            Self::Settings(_) => None,
            Self::SettingsSave(_) => None,
            Self::Installed(_) => None,
            Self::InstalledSave(value) => Some(value.server),
        }
    }
    pub fn admission(&self) -> Admission {
        match self {
            Self::SetWindow(_) => Admission::Ordinary,
            Self::ProjectsList(_) => Admission::Ordinary,
            Self::ProjectsSave(_) => Admission::Ordinary,
            Self::ProjectsRemove(_) => Admission::Ordinary,
            Self::Settings(_) => Admission::Ordinary,
            Self::SettingsSave(_) => Admission::Ordinary,
            Self::Installed(_) => Admission::Ordinary,
            Self::InstalledSave(_) => Admission::Ordinary,
        }
    }
    pub fn disconnect_policy(&self) -> DisconnectPolicy {
        match self {
            Self::SetWindow(_) => DisconnectPolicy::Complete,
            Self::ProjectsList(_) => DisconnectPolicy::Complete,
            Self::ProjectsSave(_) => DisconnectPolicy::Complete,
            Self::ProjectsRemove(_) => DisconnectPolicy::Complete,
            Self::Settings(_) => DisconnectPolicy::Complete,
            Self::SettingsSave(_) => DisconnectPolicy::Complete,
            Self::Installed(_) => DisconnectPolicy::Complete,
            Self::InstalledSave(_) => DisconnectPolicy::Complete,
        }
    }
    pub fn limits(&self) -> Limits {
        match self {
            Self::SetWindow(_) => REQUEST_LIMITS,
            Self::ProjectsList(_) => REQUEST_LIMITS,
            Self::ProjectsSave(_) => REQUEST_LIMITS,
            Self::ProjectsRemove(_) => REQUEST_LIMITS,
            Self::Settings(_) => REQUEST_LIMITS,
            Self::SettingsSave(_) => REQUEST_LIMITS,
            Self::Installed(_) => REQUEST_LIMITS,
            Self::InstalledSave(_) => REQUEST_LIMITS,
        }
    }
}
impl preferences::Reply {
    pub fn kind(&self) -> Kind {
        match self {
            Self::SetWindow(_) => Kind::PreferencesSetWindow,
            Self::ProjectsList(_) => Kind::PreferencesProjectsList,
            Self::ProjectsSave(_) => Kind::PreferencesProjectsSave,
            Self::ProjectsRemove(_) => Kind::PreferencesProjectsRemove,
            Self::Settings(_) => Kind::PreferencesSettings,
            Self::SettingsSave(_) => Kind::PreferencesSettingsSave,
            Self::Installed(_) => Kind::PreferencesInstalled,
            Self::InstalledSave(_) => Kind::PreferencesInstalledSave,
        }
    }
    pub fn validate_for(&self, operation: &preferences::Operation) -> Result<(), Invalid> {
        self.validate()?;
        match (self, operation) {
            (Self::SetWindow(_), preferences::Operation::SetWindow(_)) => Ok(()),
            (Self::ProjectsList(_), preferences::Operation::ProjectsList(_)) => Ok(()),
            (Self::ProjectsSave(_), preferences::Operation::ProjectsSave(_)) => Ok(()),
            (Self::ProjectsRemove(_), preferences::Operation::ProjectsRemove(_)) => Ok(()),
            (Self::Settings(_), preferences::Operation::Settings(_)) => Ok(()),
            (Self::SettingsSave(_), preferences::Operation::SettingsSave(_)) => Ok(()),
            (Self::Installed(_), preferences::Operation::Installed(_)) => Ok(()),
            (Self::InstalledSave(_), preferences::Operation::InstalledSave(_)) => Ok(()),
            _ => Err(Invalid("response operation")),
        }
    }
}
impl git::Operation {
    pub fn kind(&self) -> Kind {
        match self {
            Self::Request(_) => Kind::GitRequest,
            Self::RefreshRepository(_) => Kind::GitRefreshRepository,
            Self::PendingOperations(_) => Kind::GitPendingOperations,
            Self::AcknowledgeOperation(_) => Kind::GitAcknowledgeOperation,
            Self::ReviewOperation(_) => Kind::GitReviewOperation,
        }
    }
    pub fn server_id(&self) -> Option<uuid::Uuid> {
        match self {
            Self::Request(value) => Some(value.server),
            Self::RefreshRepository(value) => Some(value.server),
            Self::PendingOperations(value) => Some(value.server),
            Self::AcknowledgeOperation(value) => Some(value.server),
            Self::ReviewOperation(value) => Some(value.server),
        }
    }
    pub fn admission(&self) -> Admission {
        match self {
            Self::Request(_) => Admission::Ordinary,
            Self::RefreshRepository(_) => Admission::Ordinary,
            Self::PendingOperations(_) => Admission::Ordinary,
            Self::AcknowledgeOperation(_) => Admission::Ordinary,
            Self::ReviewOperation(_) => Admission::Ordinary,
        }
    }
    pub fn disconnect_policy(&self) -> DisconnectPolicy {
        match self {
            Self::Request(_) => DisconnectPolicy::Complete,
            Self::RefreshRepository(_) => DisconnectPolicy::Complete,
            Self::PendingOperations(_) => DisconnectPolicy::Complete,
            Self::AcknowledgeOperation(_) => DisconnectPolicy::Complete,
            Self::ReviewOperation(_) => DisconnectPolicy::Complete,
        }
    }
    pub fn limits(&self) -> Limits {
        match self {
            Self::Request(_) => REQUEST_LIMITS,
            Self::RefreshRepository(_) => REQUEST_LIMITS,
            Self::PendingOperations(_) => REQUEST_LIMITS,
            Self::AcknowledgeOperation(_) => REQUEST_LIMITS,
            Self::ReviewOperation(_) => REQUEST_LIMITS,
        }
    }
}
impl git::Reply {
    pub fn kind(&self) -> Kind {
        match self {
            Self::Request(_) => Kind::GitRequest,
            Self::RefreshRepository(_) => Kind::GitRefreshRepository,
            Self::PendingOperations(_) => Kind::GitPendingOperations,
            Self::AcknowledgeOperation(_) => Kind::GitAcknowledgeOperation,
            Self::ReviewOperation(_) => Kind::GitReviewOperation,
        }
    }
    pub fn validate_for(&self, operation: &git::Operation) -> Result<(), Invalid> {
        self.validate()?;
        match (self, operation) {
            (Self::Request(result), git::Operation::Request(request)) => {
                result.validate_request(&request.request)?;
                Ok(())
            }
            (Self::RefreshRepository(result), git::Operation::RefreshRepository(request)) => {
                if result.repo_id != request.repo_id {
                    return Err(Invalid("repository identity"));
                }
                Ok(())
            }
            (Self::PendingOperations(_), git::Operation::PendingOperations(_)) => Ok(()),
            (Self::AcknowledgeOperation(_), git::Operation::AcknowledgeOperation(_)) => Ok(()),
            (Self::ReviewOperation(_), git::Operation::ReviewOperation(_)) => Ok(()),
            _ => Err(Invalid("response operation")),
        }
    }
}
impl applications::Operation {
    pub fn kind(&self) -> Kind {
        match self {
            Self::Status(_) => Kind::ApplicationsStatus,
            Self::Catalogue(_) => Kind::ApplicationsCatalogue,
            Self::Display(_) => Kind::ApplicationsDisplay,
            Self::Launch(_) => Kind::ApplicationsLaunch,
            Self::Show(_) => Kind::ApplicationsShow,
            Self::States(_) => Kind::ApplicationsStates,
            Self::Stop(_) => Kind::ApplicationsStop,
        }
    }
    pub fn server_id(&self) -> Option<uuid::Uuid> {
        match self {
            Self::Status(value) => Some(value.server),
            Self::Catalogue(value) => Some(value.server),
            Self::Display(value) => Some(value.server),
            Self::Launch(value) => Some(value.server),
            Self::Show(value) => Some(value.server),
            Self::States(value) => Some(value.server),
            Self::Stop(value) => Some(value.server),
        }
    }
    pub fn admission(&self) -> Admission {
        match self {
            Self::Status(_) => Admission::Ordinary,
            Self::Catalogue(_) => Admission::Ordinary,
            Self::Display(_) => Admission::Ordinary,
            Self::Launch(_) => Admission::Ordinary,
            Self::Show(_) => Admission::Ordinary,
            Self::States(_) => Admission::Ordinary,
            Self::Stop(_) => Admission::Ordinary,
        }
    }
    pub fn disconnect_policy(&self) -> DisconnectPolicy {
        match self {
            Self::Status(_) => DisconnectPolicy::Complete,
            Self::Catalogue(_) => DisconnectPolicy::Complete,
            Self::Display(_) => DisconnectPolicy::Complete,
            Self::Launch(_) => DisconnectPolicy::Complete,
            Self::Show(_) => DisconnectPolicy::Complete,
            Self::States(_) => DisconnectPolicy::Complete,
            Self::Stop(_) => DisconnectPolicy::Complete,
        }
    }
    pub fn limits(&self) -> Limits {
        match self {
            Self::Status(_) => REQUEST_LIMITS,
            Self::Catalogue(_) => REQUEST_LIMITS,
            Self::Display(_) => REQUEST_LIMITS,
            Self::Launch(_) => REQUEST_LIMITS,
            Self::Show(_) => REQUEST_LIMITS,
            Self::States(_) => REQUEST_LIMITS,
            Self::Stop(_) => REQUEST_LIMITS,
        }
    }
}
impl applications::Reply {
    pub fn kind(&self) -> Kind {
        match self {
            Self::Status(_) => Kind::ApplicationsStatus,
            Self::Catalogue(_) => Kind::ApplicationsCatalogue,
            Self::Display(_) => Kind::ApplicationsDisplay,
            Self::Launch(_) => Kind::ApplicationsLaunch,
            Self::Show(_) => Kind::ApplicationsShow,
            Self::States(_) => Kind::ApplicationsStates,
            Self::Stop(_) => Kind::ApplicationsStop,
        }
    }
    pub fn validate_for(&self, operation: &applications::Operation) -> Result<(), Invalid> {
        self.validate()?;
        match (self, operation) {
            (Self::Status(_), applications::Operation::Status(_)) => Ok(()),
            (Self::Catalogue(_), applications::Operation::Catalogue(_)) => Ok(()),
            (Self::Display(_), applications::Operation::Display(_)) => Ok(()),
            (Self::Launch(_), applications::Operation::Launch(_)) => Ok(()),
            (Self::Show(_), applications::Operation::Show(_)) => Ok(()),
            (Self::States(_), applications::Operation::States(_)) => Ok(()),
            (Self::Stop(_), applications::Operation::Stop(_)) => Ok(()),
            _ => Err(Invalid("response operation")),
        }
    }
}
impl host::Operation {
    pub fn kind(&self) -> Kind {
        match self {
            Self::Activate(_) => Kind::HostActivate,
            Self::OpenRemoteApp(_) => Kind::HostOpenRemoteApp,
            Self::StartupSettings(_) => Kind::HostStartupSettings,
            Self::SetLaunchAtLogin(_) => Kind::HostSetLaunchAtLogin,
            Self::UpdateStatus(_) => Kind::HostUpdateStatus,
            Self::CheckForUpdates(_) => Kind::HostCheckForUpdates,
            Self::InstallUpdate(_) => Kind::HostInstallUpdate,
        }
    }
    pub fn server_id(&self) -> Option<uuid::Uuid> {
        match self {
            Self::Activate(_) => None,
            Self::OpenRemoteApp(_) => None,
            Self::StartupSettings(_) => None,
            Self::SetLaunchAtLogin(_) => None,
            Self::UpdateStatus(_) => None,
            Self::CheckForUpdates(_) => None,
            Self::InstallUpdate(_) => None,
        }
    }
    pub fn admission(&self) -> Admission {
        match self {
            Self::Activate(_) => Admission::Ordinary,
            Self::OpenRemoteApp(_) => Admission::Ordinary,
            Self::StartupSettings(_) => Admission::Ordinary,
            Self::SetLaunchAtLogin(_) => Admission::Ordinary,
            Self::UpdateStatus(_) => Admission::Ordinary,
            Self::CheckForUpdates(_) => Admission::Ordinary,
            Self::InstallUpdate(_) => Admission::Ordinary,
        }
    }
    pub fn disconnect_policy(&self) -> DisconnectPolicy {
        match self {
            Self::Activate(_) => DisconnectPolicy::Cancel,
            Self::OpenRemoteApp(_) => DisconnectPolicy::Cancel,
            Self::StartupSettings(_) => DisconnectPolicy::Complete,
            Self::SetLaunchAtLogin(_) => DisconnectPolicy::Complete,
            Self::UpdateStatus(_) => DisconnectPolicy::Complete,
            Self::CheckForUpdates(_) => DisconnectPolicy::Complete,
            Self::InstallUpdate(_) => DisconnectPolicy::Complete,
        }
    }
    pub fn limits(&self) -> Limits {
        match self {
            Self::Activate(_) => REQUEST_LIMITS,
            Self::OpenRemoteApp(_) => REQUEST_LIMITS,
            Self::StartupSettings(_) => REQUEST_LIMITS,
            Self::SetLaunchAtLogin(_) => REQUEST_LIMITS,
            Self::UpdateStatus(_) => REQUEST_LIMITS,
            Self::CheckForUpdates(_) => REQUEST_LIMITS,
            Self::InstallUpdate(_) => REQUEST_LIMITS,
        }
    }
}
impl host::Reply {
    pub fn kind(&self) -> Kind {
        match self {
            Self::Activate(_) => Kind::HostActivate,
            Self::OpenRemoteApp(_) => Kind::HostOpenRemoteApp,
            Self::StartupSettings(_) => Kind::HostStartupSettings,
            Self::SetLaunchAtLogin(_) => Kind::HostSetLaunchAtLogin,
            Self::UpdateStatus(_) => Kind::HostUpdateStatus,
            Self::CheckForUpdates(_) => Kind::HostCheckForUpdates,
            Self::InstallUpdate(_) => Kind::HostInstallUpdate,
        }
    }
    pub fn validate_for(&self, operation: &host::Operation) -> Result<(), Invalid> {
        self.validate()?;
        match (self, operation) {
            (Self::Activate(_), host::Operation::Activate(_)) => Ok(()),
            (Self::OpenRemoteApp(_), host::Operation::OpenRemoteApp(_)) => Ok(()),
            (Self::StartupSettings(_), host::Operation::StartupSettings(_)) => Ok(()),
            (Self::SetLaunchAtLogin(_), host::Operation::SetLaunchAtLogin(_)) => Ok(()),
            (Self::UpdateStatus(_), host::Operation::UpdateStatus(_)) => Ok(()),
            (Self::CheckForUpdates(_), host::Operation::CheckForUpdates(_)) => Ok(()),
            (Self::InstallUpdate(_), host::Operation::InstallUpdate(_)) => Ok(()),
            _ => Err(Invalid("response operation")),
        }
    }
}
impl events::Operation {
    pub fn kind(&self) -> Kind {
        match self {
            Self::Poll(_) => Kind::EventsPoll,
        }
    }
    pub fn server_id(&self) -> Option<uuid::Uuid> {
        match self {
            Self::Poll(_) => None,
        }
    }
    pub fn admission(&self) -> Admission {
        match self {
            Self::Poll(_) => Admission::Ordinary,
        }
    }
    pub fn disconnect_policy(&self) -> DisconnectPolicy {
        match self {
            Self::Poll(_) => DisconnectPolicy::Complete,
        }
    }
    pub fn limits(&self) -> Limits {
        match self {
            Self::Poll(_) => REQUEST_LIMITS,
        }
    }
}
impl events::Reply {
    pub fn kind(&self) -> Kind {
        match self {
            Self::Poll(_) => Kind::EventsPoll,
        }
    }
    pub fn validate_for(&self, operation: &events::Operation) -> Result<(), Invalid> {
        self.validate()?;
        match (self, operation) {
            (Self::Poll(_), events::Operation::Poll(_)) => Ok(()),
        }
    }
}
impl terminal::Operation {
    pub fn kind(&self) -> Kind {
        match self {
            Self::Close(_) => Kind::TerminalClose,
            Self::List(_) => Kind::TerminalList,
            Self::Rename(_) => Kind::TerminalRename,
            Self::End(_) => Kind::TerminalEnd,
        }
    }
    pub fn server_id(&self) -> Option<uuid::Uuid> {
        match self {
            Self::Close(_) => None,
            Self::List(value) => Some(value.server),
            Self::Rename(value) => Some(value.server),
            Self::End(value) => Some(value.server),
        }
    }
    pub fn admission(&self) -> Admission {
        match self {
            Self::Close(_) => Admission::Ordinary,
            Self::List(_) => Admission::Ordinary,
            Self::Rename(_) => Admission::Ordinary,
            Self::End(_) => Admission::Ordinary,
        }
    }
    pub fn disconnect_policy(&self) -> DisconnectPolicy {
        match self {
            Self::Close(_) => DisconnectPolicy::Complete,
            Self::List(_) => DisconnectPolicy::Complete,
            Self::Rename(_) => DisconnectPolicy::Complete,
            Self::End(_) => DisconnectPolicy::Complete,
        }
    }
    pub fn limits(&self) -> Limits {
        match self {
            Self::Close(_) => REQUEST_LIMITS,
            Self::List(_) => REQUEST_LIMITS,
            Self::Rename(_) => REQUEST_LIMITS,
            Self::End(_) => REQUEST_LIMITS,
        }
    }
}
impl terminal::Reply {
    pub fn kind(&self) -> Kind {
        match self {
            Self::Close(_) => Kind::TerminalClose,
            Self::List(_) => Kind::TerminalList,
            Self::Rename(_) => Kind::TerminalRename,
            Self::End(_) => Kind::TerminalEnd,
        }
    }
    pub fn validate_for(&self, operation: &terminal::Operation) -> Result<(), Invalid> {
        self.validate()?;
        match (self, operation) {
            (Self::Close(_), terminal::Operation::Close(_)) => Ok(()),
            (Self::List(_), terminal::Operation::List(_)) => Ok(()),
            (Self::Rename(_), terminal::Operation::Rename(_)) => Ok(()),
            (Self::End(_), terminal::Operation::End(_)) => Ok(()),
            _ => Err(Invalid("response operation")),
        }
    }
}
impl stream::Operation {
    pub fn kind(&self) -> Kind {
        match self {
            Self::TerminalOpen(_) => Kind::StreamTerminalOpen,
            Self::CommandOpen(_) => Kind::StreamCommandOpen,
            Self::RemoteAppsOpen(_) => Kind::StreamRemoteAppsOpen,
            Self::LspOpen(_) => Kind::StreamLspOpen,
        }
    }
    pub fn server_id(&self) -> Option<uuid::Uuid> {
        match self {
            Self::TerminalOpen(value) => Some(value.server),
            Self::CommandOpen(value) => Some(value.server),
            Self::RemoteAppsOpen(value) => Some(value.server),
            Self::LspOpen(value) => Some(value.server),
        }
    }
    pub fn admission(&self) -> Admission {
        match self {
            Self::TerminalOpen(_) => Admission::Stream,
            Self::CommandOpen(_) => Admission::Stream,
            Self::RemoteAppsOpen(_) => Admission::Stream,
            Self::LspOpen(_) => Admission::Stream,
        }
    }
    pub fn disconnect_policy(&self) -> DisconnectPolicy {
        match self {
            Self::TerminalOpen(_) => DisconnectPolicy::Stream,
            Self::CommandOpen(_) => DisconnectPolicy::Stream,
            Self::RemoteAppsOpen(_) => DisconnectPolicy::Stream,
            Self::LspOpen(_) => DisconnectPolicy::Stream,
        }
    }
    pub fn limits(&self) -> Limits {
        match self {
            Self::TerminalOpen(_) => REQUEST_LIMITS,
            Self::CommandOpen(_) => REQUEST_LIMITS,
            Self::RemoteAppsOpen(_) => REQUEST_LIMITS,
            Self::LspOpen(_) => REQUEST_LIMITS,
        }
    }
}
impl Operation {
    pub fn kind(&self) -> Kind {
        match self {
            Self::Account(v) => v.kind(),
            Self::Connection(v) => v.kind(),
            Self::Configuration(v) => v.kind(),
            Self::Files(v) => v.kind(),
            Self::Metrics(v) => v.kind(),
            Self::Services(v) => v.kind(),
            Self::Containers(v) => v.kind(),
            Self::Preferences(v) => v.kind(),
            Self::Git(v) => v.kind(),
            Self::Applications(v) => v.kind(),
            Self::Host(v) => v.kind(),
            Self::Events(v) => v.kind(),
            Self::Terminal(v) => v.kind(),
            Self::Stream(v) => v.kind(),
        }
    }
    pub fn server_id(&self) -> Option<uuid::Uuid> {
        match self {
            Self::Account(v) => v.server_id(),
            Self::Connection(v) => v.server_id(),
            Self::Configuration(v) => v.server_id(),
            Self::Files(v) => v.server_id(),
            Self::Metrics(v) => v.server_id(),
            Self::Services(v) => v.server_id(),
            Self::Containers(v) => v.server_id(),
            Self::Preferences(v) => v.server_id(),
            Self::Git(v) => v.server_id(),
            Self::Applications(v) => v.server_id(),
            Self::Host(v) => v.server_id(),
            Self::Events(v) => v.server_id(),
            Self::Terminal(v) => v.server_id(),
            Self::Stream(v) => v.server_id(),
        }
    }
    pub fn admission(&self) -> Admission {
        match self {
            Self::Account(v) => v.admission(),
            Self::Connection(v) => v.admission(),
            Self::Configuration(v) => v.admission(),
            Self::Files(v) => v.admission(),
            Self::Metrics(v) => v.admission(),
            Self::Services(v) => v.admission(),
            Self::Containers(v) => v.admission(),
            Self::Preferences(v) => v.admission(),
            Self::Git(v) => v.admission(),
            Self::Applications(v) => v.admission(),
            Self::Host(v) => v.admission(),
            Self::Events(v) => v.admission(),
            Self::Terminal(v) => v.admission(),
            Self::Stream(v) => v.admission(),
        }
    }
    pub fn disconnect_policy(&self) -> DisconnectPolicy {
        match self {
            Self::Account(v) => v.disconnect_policy(),
            Self::Connection(v) => v.disconnect_policy(),
            Self::Configuration(v) => v.disconnect_policy(),
            Self::Files(v) => v.disconnect_policy(),
            Self::Metrics(v) => v.disconnect_policy(),
            Self::Services(v) => v.disconnect_policy(),
            Self::Containers(v) => v.disconnect_policy(),
            Self::Preferences(v) => v.disconnect_policy(),
            Self::Git(v) => v.disconnect_policy(),
            Self::Applications(v) => v.disconnect_policy(),
            Self::Host(v) => v.disconnect_policy(),
            Self::Events(v) => v.disconnect_policy(),
            Self::Terminal(v) => v.disconnect_policy(),
            Self::Stream(v) => v.disconnect_policy(),
        }
    }
    pub fn limits(&self) -> Limits {
        match self {
            Self::Account(v) => v.limits(),
            Self::Connection(v) => v.limits(),
            Self::Configuration(v) => v.limits(),
            Self::Files(v) => v.limits(),
            Self::Metrics(v) => v.limits(),
            Self::Services(v) => v.limits(),
            Self::Containers(v) => v.limits(),
            Self::Preferences(v) => v.limits(),
            Self::Git(v) => v.limits(),
            Self::Applications(v) => v.limits(),
            Self::Host(v) => v.limits(),
            Self::Events(v) => v.limits(),
            Self::Terminal(v) => v.limits(),
            Self::Stream(v) => v.limits(),
        }
    }
}
impl Reply {
    pub fn kind(&self) -> Kind {
        match self {
            Self::Account(v) => v.kind(),
            Self::Connection(v) => v.kind(),
            Self::Configuration(v) => v.kind(),
            Self::Files(v) => v.kind(),
            Self::Metrics(v) => v.kind(),
            Self::Services(v) => v.kind(),
            Self::Containers(v) => v.kind(),
            Self::Preferences(v) => v.kind(),
            Self::Git(v) => v.kind(),
            Self::Applications(v) => v.kind(),
            Self::Host(v) => v.kind(),
            Self::Events(v) => v.kind(),
            Self::Terminal(v) => v.kind(),
        }
    }
    pub fn validate_for(&self, operation: &Operation) -> Result<(), Invalid> {
        match (self, operation) {
            (Self::Account(v), Operation::Account(r)) => v.validate_for(r),
            (Self::Connection(v), Operation::Connection(r)) => v.validate_for(r),
            (Self::Configuration(v), Operation::Configuration(r)) => v.validate_for(r),
            (Self::Files(v), Operation::Files(r)) => v.validate_for(r),
            (Self::Metrics(v), Operation::Metrics(r)) => v.validate_for(r),
            (Self::Services(v), Operation::Services(r)) => v.validate_for(r),
            (Self::Containers(v), Operation::Containers(r)) => v.validate_for(r),
            (Self::Preferences(v), Operation::Preferences(r)) => v.validate_for(r),
            (Self::Git(v), Operation::Git(r)) => v.validate_for(r),
            (Self::Applications(v), Operation::Applications(r)) => v.validate_for(r),
            (Self::Host(v), Operation::Host(r)) => v.validate_for(r),
            (Self::Events(v), Operation::Events(r)) => v.validate_for(r),
            (Self::Terminal(v), Operation::Terminal(r)) => v.validate_for(r),
            _ => Err(Invalid("response family")),
        }
    }
}
impl Response {
    pub fn validate_for(&self, request: &Request) -> Result<(), Invalid> {
        self.validate()?;
        if self.id != request.id {
            return Err(Invalid("response correlation"));
        }
        match &self.outcome {
            Outcome::Completed(reply) => reply.validate_for(&request.operation),
            Outcome::Failed(_) => Ok(()),
            Outcome::Opened(opened) => match &request.operation {
                Operation::Stream(value) => opened.validate_for(value),
                _ => Err(Invalid("unexpected opened stream")),
            },
        }
    }
}
impl Progress {
    pub fn validate_for(&self, request: &Request) -> Result<(), Invalid> {
        self.validate()?;
        if self.id != request.id {
            return Err(Invalid("progress correlation"));
        }
        match &self.state {
            ProgressState::AgentApproval(value)
                if request.operation.server_id() == Some(value.server) =>
            {
                Ok(())
            }
            _ => Err(Invalid("progress server")),
        }
    }
}
impl files::NativePath {
    pub fn require_local_platform(&self) -> Result<(), Invalid> {
        self.validate()?;
        match self {
            Self::Unix(_) if cfg!(unix) => Ok(()),
            Self::Windows(_) if cfg!(windows) => Ok(()),
            _ => Err(Invalid("native path platform")),
        }
    }
}
impl stream::Opened {
    pub fn validate_for(&self, operation: &stream::Operation) -> Result<(), Invalid> {
        self.validate()?;
        let matches = match (self, operation) {
            (Self::Terminal(v), stream::Operation::TerminalOpen(r)) => v.session == r.session,
            (Self::Command(v), stream::Operation::CommandOpen(r)) => v.server == r.server,
            (Self::RemoteApps(v), stream::Operation::RemoteAppsOpen(r)) => {
                v.server == r.server && v.service_version == crate::remote_apps::REVISION
            }
            (Self::Lsp(v), stream::Operation::LspOpen(r)) => v.server == r.server,
            _ => false,
        };
        if matches {
            Ok(())
        } else {
            Err(Invalid("opened stream identity"))
        }
    }
}
impl git::GitResult {
    pub fn validate_request(&self, request: &crate::git::Request) -> Result<(), Invalid> {
        match self {
            Self::Diff(_)
                if matches!(
                    request,
                    crate::git::Request::Diff { .. } | crate::git::Request::CommitDiff { .. }
                ) =>
            {
                Ok(())
            }
            Self::Reply(reply)
                if !matches!(
                    request,
                    crate::git::Request::Diff { .. } | crate::git::Request::CommitDiff { .. }
                ) && reply.kind() == request.reply_kind() =>
            {
                if let crate::git::Reply::Operation(record) = reply.as_ref() {
                    let expected = match request {
                        crate::git::Request::Start { operation_id, .. }
                        | crate::git::Request::Get { operation_id }
                        | crate::git::Request::Review { operation_id }
                        | crate::git::Request::Init { operation_id, .. }
                        | crate::git::Request::Clone { operation_id, .. } => operation_id,
                        _ => return Err(Invalid("Git operation receipt")),
                    };
                    if &record.operation_id != expected {
                        return Err(Invalid("Git operation identity"));
                    }
                }
                Ok(())
            }
            _ => Err(Invalid("Git response operation")),
        }
    }
}
