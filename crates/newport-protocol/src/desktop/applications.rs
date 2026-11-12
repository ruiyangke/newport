//! Typed desktop applications contracts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Runtime {
    pub instance: uuid::Uuid,
    pub build_sha256: String,
    pub transport_generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub runtime: crate::desktop::applications::Runtime,
    pub session: uuid::Uuid,
    pub worker: uuid::Uuid,
    pub display_generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    pub argv: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Desktop {
    pub id: String,
    pub uris: Vec<String>,
    pub action: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Command(Box<crate::desktop::applications::Command>),
    Desktop(Box<crate::desktop::applications::Desktop>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Accepted(Box<crate::common::Present>),
    Rejected(Box<String>),
    Unknown(Box<String>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Application {
    pub display: crate::desktop::applications::Identity,
    pub request: std::num::NonZeroU64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Pending,
    Opening,
    Background,
    Running,
    Stopping,
    Ended,
}

#[derive(Clone, Debug)]
pub struct Launched {
    pub application: crate::desktop::applications::Application,
    pub outcome: crate::desktop::applications::Outcome,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OptionalDetail {
    pub detail: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ending {
    Failed(Box<String>),
    NoWindow(Box<crate::desktop::applications::OptionalDetail>),
    HandedOff(Box<crate::common::Present>),
    DisplayStopped(Box<crate::common::Present>),
    ShownElsewhere(Box<crate::desktop::applications::OptionalDetail>),
}

#[derive(Clone, Debug)]
pub struct Observation {
    pub application: crate::desktop::applications::Application,
    pub state: Option<crate::desktop::applications::State>,
    pub error: Option<String>,
    pub ending: Option<crate::desktop::applications::Ending>,
}

#[derive(Clone, Debug)]
pub struct Observations {
    pub observations: Vec<crate::desktop::applications::Observation>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shown {
    pub app_id: String,
    pub windows: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShownOn {
    pub computer: Option<String>,
    pub session: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Display {
    pub shown_on: Option<crate::desktop::applications::ShownOn>,
    pub applications: Vec<crate::desktop::applications::Shown>,
}

#[derive(Clone, Debug)]
pub struct Stopped {
    pub stopped: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Requirement {
    Dbus,
    Xwayland,
    Fonts,
    Gles,
    Portal,
    Audio,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inactive {
    Root,
    Stopped,
    Failed,
}

#[derive(Clone, Debug)]
pub enum Finding {
    Missing(Box<crate::common::Present>),
    Outdated(Box<String>),
    Inactive(Box<crate::desktop::applications::Inactive>),
    Unverified(Box<crate::common::Present>),
}

#[derive(Clone, Debug)]
pub struct Gap {
    pub requirement: crate::desktop::applications::Requirement,
    pub finding: crate::desktop::applications::Finding,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Elevation {
    Root,
    Sudo,
    Doas,
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Updater {
    RpmOstree,
    TransactionalUpdate,
}

#[derive(Clone, Debug)]
pub struct Host {
    pub elevation: crate::desktop::applications::Elevation,
    pub updater: Option<crate::desktop::applications::Updater>,
    pub user_services: bool,
}

#[derive(Clone, Debug)]
pub struct LegacyAgent {
    pub revision: String,
    pub agent: String,
    pub built_at: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct SelectedRuntime {
    pub build_sha256: String,
    pub product: crate::product::Variant,
}

#[derive(Clone, Debug)]
pub struct Availability {
    pub agent: Option<String>,
    pub legacy_agent: Option<String>,
    pub legacy_revision: Option<String>,
    pub legacy_agents: Vec<crate::desktop::applications::LegacyAgent>,
    pub system: Option<String>,
    pub architecture: Option<String>,
    pub gaps: Vec<crate::desktop::applications::Gap>,
    pub host: crate::desktop::applications::Host,
    pub os_release: String,
    pub expected_server: crate::desktop::configuration::Server,
    pub bundled_variant: crate::product::Variant,
    pub bundled_revision: Option<String>,
    pub total: Option<u64>,
    pub installed_variant: Option<crate::product::Variant>,
    pub revision: Option<String>,
    pub build_relation: crate::desktop::configuration::BuildRelation,
    pub installation_error: Option<String>,
    pub selected_runtime: Option<crate::desktop::applications::SelectedRuntime>,
    pub agent_error: Option<String>,
    pub automatic_update: Option<crate::desktop::configuration::UpdateOutcome>,
}

#[derive(Clone, Debug)]
pub struct StatusArgs {
    pub server: uuid::Uuid,
    /// Defaults to false when absent on the wire.
    pub refresh: bool,
}

#[derive(Clone, Debug)]
pub struct CatalogueArgs {
    pub server: uuid::Uuid,
    /// Defaults to false when absent on the wire.
    pub refresh: bool,
}

#[derive(Clone, Debug)]
pub struct DisplayArgs {
    pub server: uuid::Uuid,
}

#[derive(Clone, Debug)]
pub struct LaunchArgs {
    pub server: uuid::Uuid,
    pub expected: crate::desktop::configuration::ConnectionScope,
    pub target: crate::desktop::applications::Target,
}

#[derive(Clone, Debug)]
pub struct ShowArgs {
    pub server: uuid::Uuid,
    pub expected: crate::desktop::configuration::ConnectionScope,
}

#[derive(Clone, Debug)]
pub struct StatesArgs {
    pub server: uuid::Uuid,
    pub expected: crate::desktop::configuration::ConnectionScope,
    pub applications: Vec<crate::desktop::applications::Application>,
}

#[derive(Clone, Debug)]
pub struct StopArgs {
    pub server: uuid::Uuid,
    pub expected: crate::desktop::configuration::ConnectionScope,
    pub application: crate::desktop::applications::Application,
}

#[derive(Clone, Debug)]
pub enum Operation {
    Status(Box<crate::desktop::applications::StatusArgs>),
    Catalogue(Box<crate::desktop::applications::CatalogueArgs>),
    Display(Box<crate::desktop::applications::DisplayArgs>),
    Launch(Box<crate::desktop::applications::LaunchArgs>),
    Show(Box<crate::desktop::applications::ShowArgs>),
    States(Box<crate::desktop::applications::StatesArgs>),
    Stop(Box<crate::desktop::applications::StopArgs>),
}

#[derive(Clone, Debug)]
pub enum Reply {
    Status(Box<crate::desktop::applications::Availability>),
    Catalogue(Box<crate::remote_apps::apps::Catalogue>),
    Display(Box<crate::desktop::applications::Display>),
    Launch(Box<crate::desktop::applications::Launched>),
    Show(Box<crate::common::Present>),
    States(Box<crate::desktop::applications::Observations>),
    Stop(Box<crate::desktop::applications::Stopped>),
}
