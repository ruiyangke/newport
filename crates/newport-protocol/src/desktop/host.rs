//! Typed desktop host contracts.
#[derive(Clone, Debug)]
pub struct Startup {
    pub enabled: bool,
    pub available: bool,
}

#[derive(Clone, Debug)]
pub struct BuildStamp {
    pub commit: Option<String>,
    pub local: bool,
    pub built_at: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdatePhase {
    Idle,
    Checking,
    Downloading,
    Ready,
    Installing,
    Current,
    Error,
}

#[derive(Clone, Debug)]
pub struct UpdateStatus {
    pub enabled: bool,
    pub current_version: String,
    pub build: crate::desktop::host::BuildStamp,
    pub phase: crate::desktop::host::UpdatePhase,
    pub version: Option<String>,
    pub downloaded: u64,
    pub total: Option<u64>,
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct OpenRemoteAppArgs {
    pub id: String,
    pub action: Option<String>,
}

#[derive(Clone, Debug)]
pub struct SetLaunchAtLoginArgs {
    pub enabled: bool,
}

#[derive(Clone, Debug)]
pub struct InstallUpdateArgs {
    pub confirmed_version: String,
}

#[derive(Clone, Debug)]
pub enum Operation {
    Activate(Box<crate::common::Present>),
    OpenRemoteApp(Box<crate::desktop::host::OpenRemoteAppArgs>),
    StartupSettings(Box<crate::common::Present>),
    SetLaunchAtLogin(Box<crate::desktop::host::SetLaunchAtLoginArgs>),
    UpdateStatus(Box<crate::common::Present>),
    CheckForUpdates(Box<crate::common::Present>),
    InstallUpdate(Box<crate::desktop::host::InstallUpdateArgs>),
}

#[derive(Clone, Debug)]
pub enum Reply {
    Activate(Box<crate::common::Present>),
    OpenRemoteApp(Box<crate::common::Present>),
    StartupSettings(Box<crate::desktop::host::Startup>),
    SetLaunchAtLogin(Box<crate::desktop::host::Startup>),
    UpdateStatus(Box<crate::desktop::host::UpdateStatus>),
    CheckForUpdates(Box<crate::common::Present>),
    InstallUpdate(Box<crate::common::Present>),
}
