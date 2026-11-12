//! Socket observations and independently verified process and service attribution.
use super::{MAX_ARGUMENTS, MAX_FIELD, MAX_LISTENERS, MAX_REPLY_TEXT, Project};
use crate::monitor::Invalid;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, net::IpAddr};

pub mod helper;
#[cfg(feature = "protobuf")]
pub mod wire;

pub const MAX_IDENTITIES: usize = 4096;
pub const MAX_DETAILS: usize = 256;
pub const MAX_HOLDERS: usize = 64;
pub const MAX_HOLDER_REFERENCES: usize = 16384;
pub const MAX_SERVICES: usize = 4096;
pub const MAX_ASSOCIATIONS: usize = 64;
pub const MAX_ISSUES: usize = 16;
pub const MAX_DIAGNOSTIC: usize = 512;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub refresh: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Namespace {
    pub device: u64,
    pub inode: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scope {
    pub network: Option<Namespace>,
    pub processes: Option<Namespace>,
    pub boot_id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Endpoint {
    pub address: IpAddr,
    pub port: u16,
    /// Interface on the observed server, never an interface on the desktop.
    pub interface: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SocketSource {
    Diagnostic,
    Proc,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cgroup {
    pub id: u64,
    pub path: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum HolderState {
    #[default]
    Unresolved,
    Observed,
    Changed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ServiceRelation {
    SocketCgroup,
    Holder { process: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Association {
    pub service: u32,
    pub relation: ServiceRelation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Socket {
    pub id: u32,
    pub endpoint: Endpoint,
    pub source: SocketSource,
    pub inode: u64,
    pub cookie: Option<u64>,
    pub uid: u32,
    pub account: Option<String>,
    pub ipv6_only: Option<bool>,
    pub cgroup: Option<Cgroup>,
    /// Snapshot-local process references; a service MainPID never creates one.
    pub holders: Vec<u32>,
    pub holder_state: HolderState,
    pub holders_truncated: bool,
    pub services: Vec<Association>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub start_ticks: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedPath {
    pub value: String,
    pub deleted: bool,
    /// The display path cannot be used to address a filesystem object losslessly.
    pub lossy: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Details {
    pub executable: Option<ObservedPath>,
    pub directory: Option<ObservedPath>,
    pub arguments: Vec<String>,
    pub project: Option<Project>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Process {
    pub id: u32,
    pub identity: ProcessIdentity,
    pub name: Option<String>,
    pub executable_name: Option<String>,
    pub uid: Option<u32>,
    pub account: Option<String>,
    pub details: Option<Details>,
    pub metadata: Report,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Manager {
    System,
    User { uid: u32 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Service {
    pub id: u32,
    pub manager: Manager,
    pub unit: String,
    pub description: Option<String>,
    pub control_group: String,
    /// Manager metadata only; does not establish a socket holder.
    pub main_pid: Option<u32>,
    pub triggers: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContainerState {
    Running,
    Stopped,
    Unknown,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Compose {
    pub project: Option<String>,
    pub service: Option<String>,
    pub directory: Option<String>,
    pub directory_project: Option<Project>,
}

/// Engine configuration is independent of an observed host listening socket.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Publication {
    /// Stable engine identity, when supplied by the container API.
    pub engine: Option<String>,
    pub container: String,
    pub name: String,
    pub image: Option<String>,
    pub state: ContainerState,
    pub host: Endpoint,
    pub target_port: u16,
    pub target_address: Option<IpAddr>,
    pub compose: Option<Compose>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    #[default]
    NotAttempted,
    Complete,
    Partial,
    Unavailable,
    NotApplicable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum IssueKind {
    PermissionDenied,
    Unsupported,
    TimedOut,
    Interrupted,
    BudgetExceeded,
    Io,
    InvalidResponse,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issue {
    pub kind: IssueKind,
    pub count: u64,
    pub message: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub outcome: Outcome,
    pub issues: Vec<Issue>,
}

impl Report {
    fn has_observations(&self) -> bool {
        matches!(self.outcome, Outcome::Complete | Outcome::Partial)
    }

    pub fn complete() -> Self {
        Self {
            outcome: Outcome::Complete,
            issues: Vec::new(),
        }
    }

    pub fn not_applicable() -> Self {
        Self {
            outcome: Outcome::NotApplicable,
            issues: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Coverage {
    pub ipv4: Report,
    pub ipv6: Report,
    pub processes: Report,
    pub cgroups: Report,
    pub services: Report,
    pub containers: Report,
    pub elevation: Report,
    pub details: Report,
}

impl Coverage {
    /// Callers must additionally require a fresh snapshot in the same namespace.
    /// Process visibility is unrelated to completeness of the socket inventory.
    pub fn socket_inventory_complete(&self, address: IpAddr) -> bool {
        let report = if address.is_ipv4() {
            &self.ipv4
        } else {
            &self.ipv6
        };
        report.outcome == Outcome::Complete
    }

    fn reports(&self) -> [&Report; 8] {
        [
            &self.ipv4,
            &self.ipv6,
            &self.processes,
            &self.cgroups,
            &self.services,
            &self.containers,
            &self.elevation,
            &self.details,
        ]
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostics {
    pub ipv4_source: Option<SocketSource>,
    pub ipv6_source: Option<SocketSource>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub generation: u64,
    pub started_at_ms: u64,
    pub elapsed_ms: u64,
    pub scope: Scope,
    pub sockets: Vec<Socket>,
    pub processes: Vec<Process>,
    pub services: Vec<Service>,
    pub publications: Vec<Publication>,
    pub container_engine: Option<String>,
    pub coverage: Coverage,
    pub diagnostics: Diagnostics,
}

fn unique<T: Ord>(values: impl IntoIterator<Item = T>) -> bool {
    let mut found = BTreeSet::new();
    values.into_iter().all(|value| found.insert(value))
}

fn endpoint(value: &Endpoint) -> Result<(), Invalid> {
    if value.port == 0 || value.interface == Some(0) {
        return Err(Invalid("discovery endpoint"));
    }
    Ok(())
}

pub fn valid_unit(value: &str) -> bool {
    let stem = [".service", ".socket", ".scope"]
        .into_iter()
        .find_map(|suffix| value.strip_suffix(suffix));
    stem.is_some_and(|stem| {
        !stem.is_empty()
            && value.len() <= 255
            && !value.starts_with('-')
            && stem
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b":_.@-\\".contains(&b))
    })
}

pub fn valid_cgroup_path(value: &str) -> bool {
    value == "/"
        || (value.starts_with('/')
            && value.len() <= MAX_FIELD
            && !value.chars().any(char::is_control)
            && value[1..]
                .split('/')
                .all(|part| !matches!(part, "" | "." | "..")))
}

fn report(value: &Report, bytes: &mut usize) -> Result<(), Invalid> {
    if value.issues.len() > MAX_ISSUES
        || (matches!(value.outcome, Outcome::Partial | Outcome::Unavailable)
            && value.issues.is_empty())
        || (matches!(
            value.outcome,
            Outcome::NotAttempted | Outcome::NotApplicable
        ) && !value.issues.is_empty())
    {
        return Err(Invalid("discovery coverage"));
    }
    for issue in &value.issues {
        if issue.count == 0 || issue.message.len() > MAX_DIAGNOSTIC {
            return Err(Invalid("discovery diagnostic"));
        }
        super::text([issue.message.as_str()], bytes)?;
    }
    Ok(())
}

impl Snapshot {
    /// Freshness and server incarnation must also match before claiming absence.
    pub fn publications_complete_for(&self, engine: &str) -> bool {
        self.coverage.containers.outcome == Outcome::Complete
            && self.container_engine.as_deref() == Some(engine)
    }

    pub fn validate(&self) -> Result<(), Invalid> {
        if self.validate_records()? > MAX_REPLY_TEXT {
            return Err(Invalid("discovery text budget"));
        }
        Ok(())
    }

    /// Checks individual records and references, returning aggregate text bytes.
    /// Producers use this before fitting a snapshot to the response budget.
    pub fn validate_records(&self) -> Result<usize, Invalid> {
        if self.generation == 0
            || self.sockets.len() > MAX_LISTENERS
            || self.processes.len() > MAX_IDENTITIES
            || self.services.len() > MAX_SERVICES
            || self.publications.len() > MAX_LISTENERS
            || self
                .processes
                .iter()
                .filter(|p| p.details.is_some())
                .count()
                > MAX_DETAILS
        {
            return Err(Invalid("discovery snapshot"));
        }
        let mut bytes = 0;
        if self
            .scope
            .network
            .iter()
            .chain(self.scope.processes.iter())
            .any(|ns| ns.inode == 0)
            || self
                .scope
                .boot_id
                .as_ref()
                .is_some_and(|id| uuid::Uuid::parse_str(id).is_err())
        {
            return Err(Invalid("discovery scope"));
        }
        super::text(self.scope.boot_id.as_deref(), &mut bytes)?;
        super::text(self.container_engine.as_deref(), &mut bytes)?;
        if self.container_engine.as_ref().is_some_and(|engine| {
            engine.is_empty()
                || engine.chars().any(char::is_control)
                || matches!(
                    self.coverage.containers.outcome,
                    Outcome::NotAttempted | Outcome::NotApplicable
                )
        }) || (self.coverage.containers.outcome == Outcome::Complete
            && self.container_engine.is_none())
        {
            return Err(Invalid("discovery container engine"));
        }
        for value in self.coverage.reports() {
            report(value, &mut bytes)?;
        }
        if self.sockets.iter().any(|socket| {
            let report = if socket.endpoint.address.is_ipv4() {
                &self.coverage.ipv4
            } else {
                &self.coverage.ipv6
            };
            !report.has_observations()
        }) || (!self.processes.is_empty() && !self.coverage.processes.has_observations())
            || (!self.services.is_empty() && !self.coverage.services.has_observations())
            || (!self.publications.is_empty() && !self.coverage.containers.has_observations())
        {
            return Err(Invalid("discovery source coverage"));
        }
        let processes = self.processes.iter().map(|p| p.id).collect::<BTreeSet<_>>();
        let services = self.services.iter().map(|s| s.id).collect::<BTreeSet<_>>();
        if processes.len() != self.processes.len()
            || services.len() != self.services.len()
            || processes.contains(&0)
            || services.contains(&0)
            || !unique(self.sockets.iter().map(|s| s.id))
            || !unique(self.processes.iter().map(|p| p.identity))
            || !unique(self.services.iter().map(|s| (s.manager, &s.unit)))
        {
            return Err(Invalid("discovery identity"));
        }
        let mut references = 0usize;
        for socket in &self.sockets {
            endpoint(&socket.endpoint)?;
            references = references.saturating_add(socket.holders.len());
            if socket.id == 0
                || socket.holders.len() > MAX_HOLDERS
                || references > MAX_HOLDER_REFERENCES
                || socket.cookie == Some(u64::MAX)
                || socket.services.len() > MAX_ASSOCIATIONS
                || (socket.endpoint.address.is_ipv4() && socket.ipv6_only.is_some())
                || (socket.holder_state == HolderState::Observed) != !socket.holders.is_empty()
                || !unique(socket.holders.iter())
                || !unique(socket.services.iter())
                || socket.holders.iter().any(|id| !processes.contains(id))
            {
                return Err(Invalid("discovery socket"));
            }
            super::text(socket.account.as_deref(), &mut bytes)?;
            if let Some(cgroup) = &socket.cgroup {
                if cgroup.id == 0 || cgroup.path.as_ref().is_some_and(|p| !valid_cgroup_path(p)) {
                    return Err(Invalid("discovery cgroup"));
                }
                super::text(cgroup.path.as_deref(), &mut bytes)?;
            }
            for association in &socket.services {
                if !services.contains(&association.service)
                    || matches!(association.relation, ServiceRelation::Holder { process }
                        if !socket.holders.contains(&process))
                    || (association.relation == ServiceRelation::SocketCgroup
                        && socket.cgroup.is_none())
                {
                    return Err(Invalid("discovery service association"));
                }
            }
        }
        for process in &self.processes {
            if process.identity.pid == 0 {
                return Err(Invalid("discovery process"));
            }
            super::text(
                process
                    .name
                    .as_deref()
                    .into_iter()
                    .chain(process.executable_name.as_deref())
                    .chain(process.account.as_deref()),
                &mut bytes,
            )?;
            report(&process.metadata, &mut bytes)?;
            if let Some(details) = &process.details {
                if details.arguments.len() > MAX_ARGUMENTS {
                    return Err(Invalid("discovery arguments"));
                }
                for path in details.executable.iter().chain(details.directory.iter()) {
                    if !path.value.starts_with('/') {
                        return Err(Invalid("discovery process path"));
                    }
                    super::text([path.value.as_str()], &mut bytes)?;
                }
                super::text(details.arguments.iter().map(String::as_str), &mut bytes)?;
                if let Some(project) = &details.project {
                    super::project(project, &mut bytes)?;
                }
            }
        }
        for service in &self.services {
            if !valid_unit(&service.unit)
                || !valid_cgroup_path(&service.control_group)
                || service.main_pid == Some(0)
                || service.triggers.len() > MAX_ASSOCIATIONS
                || service.triggers.iter().any(|unit| !valid_unit(unit))
                || !unique(service.triggers.iter())
            {
                return Err(Invalid("discovery service"));
            }
            super::text(
                [service.unit.as_str(), &service.control_group]
                    .into_iter()
                    .chain(service.description.as_deref())
                    .chain(service.triggers.iter().map(String::as_str)),
                &mut bytes,
            )?;
        }
        if !unique(self.publications.iter().map(|p| {
            (
                &p.engine,
                &p.container,
                p.host,
                p.target_port,
                p.target_address,
            )
        })) {
            return Err(Invalid("discovery publication identity"));
        }
        for publication in &self.publications {
            endpoint(&publication.host)?;
            if publication.engine != self.container_engine
                || publication.container.is_empty()
                || publication.name.is_empty()
                || publication.target_port == 0
            {
                return Err(Invalid("discovery publication"));
            }
            super::text(
                [publication.container.as_str(), &publication.name]
                    .into_iter()
                    .chain(publication.engine.as_deref())
                    .chain(publication.image.as_deref()),
                &mut bytes,
            )?;
            if let Some(compose) = &publication.compose {
                super::text(
                    compose
                        .project
                        .as_deref()
                        .into_iter()
                        .chain(compose.service.as_deref())
                        .chain(compose.directory.as_deref()),
                    &mut bytes,
                )?;
                if let Some(project) = &compose.directory_project {
                    super::project(project, &mut bytes)?;
                }
            }
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests;
