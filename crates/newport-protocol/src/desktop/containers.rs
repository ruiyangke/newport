//! Typed desktop containers contracts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiskPart {
    Full,
    Quick,
    Volumes,
}

#[derive(Clone, Debug)]
pub struct SnapshotArgs {
    pub server: uuid::Uuid,
    /// Defaults to false when absent on the wire.
    pub refresh: bool,
}

#[derive(Clone, Debug)]
pub struct InspectArgs {
    pub server: uuid::Uuid,
    pub container: String,
    /// Defaults to false when absent on the wire.
    pub refresh: bool,
}

#[derive(Clone, Debug)]
pub struct LogsArgs {
    pub server: uuid::Uuid,
    pub request: crate::monitor::containers::LogsRequest,
}

#[derive(Clone, Debug)]
pub struct ActionArgs {
    pub server: uuid::Uuid,
    pub container: String,
    pub verb: crate::monitor::containers::ContainerVerb,
}

#[derive(Clone, Debug)]
pub struct ProjectActionArgs {
    pub server: uuid::Uuid,
    pub project: String,
    pub verb: crate::monitor::containers::ProjectVerb,
    pub expected_ids: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct RemoveArgs {
    pub server: uuid::Uuid,
    pub containers: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct DiskArgs {
    pub server: uuid::Uuid,
    /// Defaults to Full when absent on the wire.
    pub part: crate::desktop::containers::DiskPart,
    /// Defaults to false when absent on the wire.
    pub refresh: bool,
}

#[derive(Clone, Debug)]
pub struct PruneArgs {
    pub server: uuid::Uuid,
    pub what: crate::monitor::containers::PruneTarget,
}

#[derive(Clone, Debug)]
pub struct ComposeProjectArgs {
    pub server: uuid::Uuid,
    pub project: String,
    /// Defaults to false when absent on the wire.
    pub refresh: bool,
}

#[derive(Clone, Debug)]
pub struct ComposeRunArgs {
    pub server: uuid::Uuid,
    pub project: String,
    pub action: crate::monitor::compose::Action,
}

#[derive(Clone, Debug)]
pub struct ComposeJobArgs {
    pub server: uuid::Uuid,
    pub job: String,
    /// Defaults to 0 when absent on the wire.
    pub offset: u64,
}

#[derive(Clone, Debug)]
pub struct ComposeCancelArgs {
    pub server: uuid::Uuid,
    pub job: String,
}

#[derive(Clone, Debug)]
pub struct ComposeJobsArgs {
    pub server: uuid::Uuid,
}

#[derive(Clone, Debug)]
pub enum Operation {
    Snapshot(Box<crate::desktop::containers::SnapshotArgs>),
    Inspect(Box<crate::desktop::containers::InspectArgs>),
    Logs(Box<crate::desktop::containers::LogsArgs>),
    Action(Box<crate::desktop::containers::ActionArgs>),
    ProjectAction(Box<crate::desktop::containers::ProjectActionArgs>),
    Remove(Box<crate::desktop::containers::RemoveArgs>),
    Disk(Box<crate::desktop::containers::DiskArgs>),
    Prune(Box<crate::desktop::containers::PruneArgs>),
    ComposeProject(Box<crate::desktop::containers::ComposeProjectArgs>),
    ComposeRun(Box<crate::desktop::containers::ComposeRunArgs>),
    ComposeJob(Box<crate::desktop::containers::ComposeJobArgs>),
    ComposeCancel(Box<crate::desktop::containers::ComposeCancelArgs>),
    ComposeJobs(Box<crate::desktop::containers::ComposeJobsArgs>),
}

#[derive(Clone, Debug)]
pub enum Reply {
    Snapshot(Box<crate::monitor::containers::ContainersSnapshot>),
    Inspect(Box<crate::monitor::containers::ContainerInspect>),
    Logs(Box<crate::monitor::containers::ContainerLogPage>),
    Action(Box<crate::monitor::containers::Done>),
    ProjectAction(Box<crate::monitor::containers::Done>),
    Remove(Box<crate::monitor::containers::Removed>),
    Disk(Box<crate::monitor::containers::DockerDisk>),
    Prune(Box<crate::monitor::containers::Pruned>),
    ComposeProject(Box<crate::monitor::compose::Project>),
    ComposeRun(Box<crate::monitor::compose::Started>),
    ComposeJob(Box<crate::monitor::compose::Job>),
    ComposeCancel(Box<crate::monitor::compose::Done>),
    ComposeJobs(Box<crate::monitor::compose::Jobs>),
}
