//! Typed desktop services contracts.
#[derive(Clone, Debug)]
pub struct SnapshotArgs {
    pub server: uuid::Uuid,
    /// Defaults to false when absent on the wire.
    pub refresh: bool,
}

#[derive(Clone, Debug)]
pub struct LogsArgs {
    pub server: uuid::Uuid,
    pub request: crate::monitor::services::LogsRequest,
}

#[derive(Clone, Debug)]
pub struct UnitFileArgs {
    pub server: uuid::Uuid,
    pub unit: String,
    pub scope: crate::monitor::services::Scope,
    /// Defaults to false when absent on the wire.
    pub refresh: bool,
}

#[derive(Clone, Debug)]
pub struct ActionArgs {
    pub server: uuid::Uuid,
    pub unit: String,
    pub scope: crate::monitor::services::Scope,
    pub verb: crate::monitor::services::Verb,
}

#[derive(Clone, Debug)]
pub enum Operation {
    Snapshot(Box<crate::desktop::services::SnapshotArgs>),
    Logs(Box<crate::desktop::services::LogsArgs>),
    UnitFile(Box<crate::desktop::services::UnitFileArgs>),
    Action(Box<crate::desktop::services::ActionArgs>),
}

#[derive(Clone, Debug)]
pub enum Reply {
    Snapshot(Box<crate::monitor::services::Snapshot>),
    Logs(Box<crate::monitor::services::Logs>),
    UnitFile(Box<crate::monitor::services::UnitFile>),
    Action(Box<crate::monitor::services::ActionReply>),
}
