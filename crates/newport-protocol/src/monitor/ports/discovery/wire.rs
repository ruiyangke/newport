//! Generated protobuf mapping shared by monitor observations and finite helpers.
use super::*;
use crate::{
    encoding::protobuf::{self, generated::newport::discovery as pb},
    monitor::{SERVER_LIMITS, wire::Error},
};
use bytes::Bytes;
use prost::Message;
use std::{
    io::{Read, Write},
    net::{Ipv4Addr, Ipv6Addr},
};

fn required<T>(value: Option<T>) -> Result<T, Invalid> {
    value.ok_or(Invalid("required discovery field"))
}

pub(crate) trait Record: Sized {
    type Wire;
    fn to_wire(&self) -> Self::Wire;
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid>;
}

fn list<T: Record>(values: Vec<T::Wire>) -> Result<Vec<T>, Invalid> {
    values.into_iter().map(T::from_wire).collect()
}

trait Enum: Sized {
    fn to_wire(self) -> i32;
    fn from_wire(value: i32) -> Result<Self, Invalid>;
}
macro_rules! enumeration {
    ($ty:ident { $($variant:ident = $value:literal),+ $(,)? }) => {
        impl Enum for $ty {
            fn to_wire(self) -> i32 { match self { $(Self::$variant => $value),+ } }
            fn from_wire(value: i32) -> Result<Self, Invalid> {
                match value { $($value => Ok(Self::$variant)),+, _ => Err(Invalid("discovery enum")) }
            }
        }
    };
}
/// The enum maps `enumeration!` writes.
#[cfg(test)]
pub(crate) fn enum_maps() -> Vec<crate::desktop::wire::wire_tables::EnumMap> {
    fn map<T: Enum + Copy + std::fmt::Debug + 'static>(
        proto: &'static str,
    ) -> crate::desktop::wire::wire_tables::EnumMap {
        crate::desktop::wire::wire_tables::enum_map(
            proto,
            |value| T::from_wire(value).ok(),
            |value| value.to_wire(),
        )
    }
    vec![
        map::<SocketSource>("newport.discovery.SocketSource"),
        map::<HolderState>("newport.discovery.HolderState"),
        map::<ContainerState>("newport.discovery.ContainerState"),
        map::<Outcome>("newport.discovery.Outcome"),
        map::<IssueKind>("newport.discovery.IssueKind"),
    ]
}
enumeration!(SocketSource { Diagnostic = 1, Proc = 2 });
enumeration!(HolderState { Unresolved = 1, Observed = 2, Changed = 3 });
enumeration!(ContainerState { Running = 1, Stopped = 2, Unknown = 3 });
enumeration!(Outcome { NotAttempted = 1, Complete = 2, Partial = 3, Unavailable = 4, NotApplicable = 5 });
enumeration!(IssueKind { PermissionDenied = 1, Unsupported = 2, TimedOut = 3,
    Interrupted = 4, BudgetExceeded = 5, Io = 6, InvalidResponse = 7, Unavailable = 8 });

fn address(value: &IpAddr) -> Bytes {
    match value {
        IpAddr::V4(v) => Bytes::copy_from_slice(&v.octets()),
        IpAddr::V6(v) => Bytes::copy_from_slice(&v.octets()),
    }
}
fn parse_address(value: &[u8]) -> Result<IpAddr, Invalid> {
    match value.len() {
        4 => Ok(
            Ipv4Addr::from(<[u8; 4]>::try_from(value).map_err(|_| Invalid("IPv4 address"))?).into(),
        ),
        16 => Ok(
            Ipv6Addr::from(<[u8; 16]>::try_from(value).map_err(|_| Invalid("IPv6 address"))?)
                .into(),
        ),
        _ => Err(Invalid("discovery address")),
    }
}

impl Record for Namespace {
    type Wire = pb::Namespace;
    fn to_wire(&self) -> Self::Wire {
        pb::Namespace {
            device: Some(self.device),
            inode: self.inode,
        }
    }
    fn from_wire(v: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            device: required(v.device)?,
            inode: v.inode,
        })
    }
}
impl Record for Scope {
    type Wire = pb::Scope;
    fn to_wire(&self) -> Self::Wire {
        pb::Scope {
            network: self.network.as_ref().map(Record::to_wire),
            processes: self.processes.as_ref().map(Record::to_wire),
            boot_id: self.boot_id.clone(),
        }
    }
    fn from_wire(v: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            network: v.network.map(Namespace::from_wire).transpose()?,
            processes: v.processes.map(Namespace::from_wire).transpose()?,
            boot_id: v.boot_id,
        })
    }
}
impl Record for Endpoint {
    type Wire = pb::Endpoint;
    fn to_wire(&self) -> Self::Wire {
        pb::Endpoint {
            address: address(&self.address),
            port: u32::from(self.port),
            interface: self.interface,
        }
    }
    fn from_wire(v: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            address: parse_address(&v.address)?,
            port: u16::try_from(v.port).map_err(|_| Invalid("discovery port"))?,
            interface: v.interface,
        })
    }
}
impl Record for Cgroup {
    type Wire = pb::Cgroup;
    fn to_wire(&self) -> Self::Wire {
        pb::Cgroup {
            id: self.id,
            path: self.path.clone(),
        }
    }
    fn from_wire(v: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            id: v.id,
            path: v.path,
        })
    }
}
impl Record for Association {
    type Wire = pb::Association;
    fn to_wire(&self) -> Self::Wire {
        use pb::association::Relation;
        pb::Association {
            service: self.service,
            relation: Some(match self.relation {
                ServiceRelation::SocketCgroup => Relation::SocketCgroup(protobuf::Present {}),
                ServiceRelation::Holder { process } => Relation::Holder(process),
            }),
        }
    }
    fn from_wire(v: Self::Wire) -> Result<Self, Invalid> {
        use pb::association::Relation;
        Ok(Self {
            service: v.service,
            relation: match required(v.relation)? {
                Relation::SocketCgroup(_) => ServiceRelation::SocketCgroup,
                Relation::Holder(process) => ServiceRelation::Holder { process },
            },
        })
    }
}
impl Record for Socket {
    type Wire = pb::Socket;
    fn to_wire(&self) -> Self::Wire {
        pb::Socket {
            id: self.id,
            endpoint: Some(self.endpoint.to_wire()),
            source: self.source.to_wire(),
            inode: Some(self.inode),
            cookie: self.cookie,
            uid: Some(self.uid),
            account: self.account.clone(),
            ipv6_only: self.ipv6_only,
            cgroup: self.cgroup.as_ref().map(Record::to_wire),
            holders: self.holders.clone(),
            holder_state: self.holder_state.to_wire(),
            holders_truncated: self.holders_truncated,
            services: self.services.iter().map(Record::to_wire).collect(),
        }
    }
    fn from_wire(v: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            id: v.id,
            endpoint: Endpoint::from_wire(required(v.endpoint)?)?,
            source: SocketSource::from_wire(v.source)?,
            inode: required(v.inode)?,
            cookie: v.cookie,
            uid: required(v.uid)?,
            account: v.account,
            ipv6_only: v.ipv6_only,
            cgroup: v.cgroup.map(Cgroup::from_wire).transpose()?,
            holders: v.holders,
            holder_state: HolderState::from_wire(v.holder_state)?,
            holders_truncated: v.holders_truncated,
            services: list(v.services)?,
        })
    }
}
impl Record for ProcessIdentity {
    type Wire = pb::ProcessIdentity;
    fn to_wire(&self) -> Self::Wire {
        pb::ProcessIdentity {
            pid: self.pid,
            start_ticks: Some(self.start_ticks),
        }
    }
    fn from_wire(v: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            pid: v.pid,
            start_ticks: required(v.start_ticks)?,
        })
    }
}
impl Record for ObservedPath {
    type Wire = pb::ObservedPath;
    fn to_wire(&self) -> Self::Wire {
        pb::ObservedPath {
            value: self.value.clone(),
            deleted: self.deleted,
            lossy: self.lossy,
        }
    }
    fn from_wire(v: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            value: v.value,
            deleted: v.deleted,
            lossy: v.lossy,
        })
    }
}
impl Record for Project {
    type Wire = pb::Project;
    fn to_wire(&self) -> Self::Wire {
        pb::Project {
            root: self.root.clone(),
            marker: self.marker.clone(),
            worktree: self.worktree.clone(),
            branch: self.branch.clone(),
            main: self.main,
        }
    }
    fn from_wire(v: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            root: v.root,
            marker: v.marker,
            worktree: v.worktree,
            branch: v.branch,
            main: v.main,
        })
    }
}
impl Record for Details {
    type Wire = pb::Details;
    fn to_wire(&self) -> Self::Wire {
        pb::Details {
            executable: self.executable.as_ref().map(Record::to_wire),
            directory: self.directory.as_ref().map(Record::to_wire),
            arguments: self.arguments.clone(),
            project: self.project.as_ref().map(Record::to_wire),
        }
    }
    fn from_wire(v: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            executable: v.executable.map(ObservedPath::from_wire).transpose()?,
            directory: v.directory.map(ObservedPath::from_wire).transpose()?,
            arguments: v.arguments,
            project: v.project.map(Project::from_wire).transpose()?,
        })
    }
}
impl Record for Process {
    type Wire = pb::Process;
    fn to_wire(&self) -> Self::Wire {
        pb::Process {
            id: self.id,
            identity: Some(self.identity.to_wire()),
            name: self.name.clone(),
            executable_name: self.executable_name.clone(),
            uid: self.uid,
            account: self.account.clone(),
            details: self.details.as_ref().map(Record::to_wire),
            metadata: Some(self.metadata.to_wire()),
        }
    }
    fn from_wire(v: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            id: v.id,
            identity: ProcessIdentity::from_wire(required(v.identity)?)?,
            name: v.name,
            executable_name: v.executable_name,
            uid: v.uid,
            account: v.account,
            details: v.details.map(Details::from_wire).transpose()?,
            metadata: Report::from_wire(required(v.metadata)?)?,
        })
    }
}
impl Record for Manager {
    type Wire = pb::Manager;
    fn to_wire(&self) -> Self::Wire {
        pb::Manager {
            scope: Some(match self {
                Self::System => pb::manager::Scope::System(protobuf::Present {}),
                Self::User { uid } => pb::manager::Scope::UserUid(*uid),
            }),
        }
    }
    fn from_wire(v: Self::Wire) -> Result<Self, Invalid> {
        Ok(match required(v.scope)? {
            pb::manager::Scope::System(_) => Self::System,
            pb::manager::Scope::UserUid(uid) => Self::User { uid },
        })
    }
}
impl Record for Service {
    type Wire = pb::Service;
    fn to_wire(&self) -> Self::Wire {
        pb::Service {
            id: self.id,
            manager: Some(self.manager.to_wire()),
            unit: self.unit.clone(),
            description: self.description.clone(),
            control_group: self.control_group.clone(),
            main_pid: self.main_pid,
            triggers: self.triggers.clone(),
        }
    }
    fn from_wire(v: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            id: v.id,
            manager: Manager::from_wire(required(v.manager)?)?,
            unit: v.unit,
            description: v.description,
            control_group: v.control_group,
            main_pid: v.main_pid,
            triggers: v.triggers,
        })
    }
}
impl Record for Compose {
    type Wire = pb::Compose;
    fn to_wire(&self) -> Self::Wire {
        pb::Compose {
            project: self.project.clone(),
            service: self.service.clone(),
            directory: self.directory.clone(),
            directory_project: self.directory_project.as_ref().map(Record::to_wire),
        }
    }
    fn from_wire(v: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            project: v.project,
            service: v.service,
            directory: v.directory,
            directory_project: v.directory_project.map(Project::from_wire).transpose()?,
        })
    }
}
impl Record for Publication {
    type Wire = pb::Publication;
    fn to_wire(&self) -> Self::Wire {
        pb::Publication {
            engine: self.engine.clone(),
            container: self.container.clone(),
            name: self.name.clone(),
            image: self.image.clone(),
            state: self.state.to_wire(),
            host: Some(self.host.to_wire()),
            target_port: u32::from(self.target_port),
            target_address: self.target_address.as_ref().map(address),
            compose: self.compose.as_ref().map(Record::to_wire),
        }
    }
    fn from_wire(v: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            engine: v.engine,
            container: v.container,
            name: v.name,
            image: v.image,
            state: ContainerState::from_wire(v.state)?,
            host: Endpoint::from_wire(required(v.host)?)?,
            target_port: u16::try_from(v.target_port)
                .map_err(|_| Invalid("discovery target port"))?,
            target_address: v.target_address.as_deref().map(parse_address).transpose()?,
            compose: v.compose.map(Compose::from_wire).transpose()?,
        })
    }
}
impl Record for Issue {
    type Wire = pb::Issue;
    fn to_wire(&self) -> Self::Wire {
        pb::Issue {
            kind: self.kind.to_wire(),
            count: self.count,
            message: self.message.clone(),
        }
    }
    fn from_wire(v: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            kind: IssueKind::from_wire(v.kind)?,
            count: v.count,
            message: v.message,
        })
    }
}
impl Record for Report {
    type Wire = pb::Report;
    fn to_wire(&self) -> Self::Wire {
        pb::Report {
            outcome: self.outcome.to_wire(),
            issues: self.issues.iter().map(Record::to_wire).collect(),
        }
    }
    fn from_wire(v: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            outcome: Outcome::from_wire(v.outcome)?,
            issues: list(v.issues)?,
        })
    }
}
impl Record for Coverage {
    type Wire = pb::Coverage;
    fn to_wire(&self) -> Self::Wire {
        pb::Coverage {
            ipv4: Some(self.ipv4.to_wire()),
            ipv6: Some(self.ipv6.to_wire()),
            processes: Some(self.processes.to_wire()),
            cgroups: Some(self.cgroups.to_wire()),
            services: Some(self.services.to_wire()),
            containers: Some(self.containers.to_wire()),
            elevation: Some(self.elevation.to_wire()),
            details: Some(self.details.to_wire()),
        }
    }
    fn from_wire(v: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            ipv4: Report::from_wire(required(v.ipv4)?)?,
            ipv6: Report::from_wire(required(v.ipv6)?)?,
            processes: Report::from_wire(required(v.processes)?)?,
            cgroups: Report::from_wire(required(v.cgroups)?)?,
            services: Report::from_wire(required(v.services)?)?,
            containers: Report::from_wire(required(v.containers)?)?,
            elevation: Report::from_wire(required(v.elevation)?)?,
            details: Report::from_wire(required(v.details)?)?,
        })
    }
}
impl Record for Diagnostics {
    type Wire = pb::Diagnostics;
    fn to_wire(&self) -> Self::Wire {
        pb::Diagnostics {
            ipv4_source: self.ipv4_source.map(Enum::to_wire),
            ipv6_source: self.ipv6_source.map(Enum::to_wire),
        }
    }
    fn from_wire(v: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            ipv4_source: v.ipv4_source.map(SocketSource::from_wire).transpose()?,
            ipv6_source: v.ipv6_source.map(SocketSource::from_wire).transpose()?,
        })
    }
}

impl Record for Snapshot {
    type Wire = pb::Snapshot;
    fn to_wire(&self) -> Self::Wire {
        pb::Snapshot {
            generation: self.generation,
            started_at_ms: Some(self.started_at_ms),
            elapsed_ms: Some(self.elapsed_ms),
            scope: Some(self.scope.to_wire()),
            sockets: self.sockets.iter().map(Record::to_wire).collect(),
            processes: self.processes.iter().map(Record::to_wire).collect(),
            services: self.services.iter().map(Record::to_wire).collect(),
            publications: self.publications.iter().map(Record::to_wire).collect(),
            container_engine: self.container_engine.clone(),
            coverage: Some(self.coverage.to_wire()),
            diagnostics: Some(self.diagnostics.to_wire()),
        }
    }
    fn from_wire(v: Self::Wire) -> Result<Self, Invalid> {
        let result = Self {
            generation: v.generation,
            started_at_ms: required(v.started_at_ms)?,
            elapsed_ms: required(v.elapsed_ms)?,
            scope: Scope::from_wire(required(v.scope)?)?,
            sockets: list(v.sockets)?,
            processes: list(v.processes)?,
            services: list(v.services)?,
            publications: list(v.publications)?,
            container_engine: v.container_engine,
            coverage: Coverage::from_wire(required(v.coverage)?)?,
            diagnostics: Diagnostics::from_wire(required(v.diagnostics)?)?,
        };
        result.validate()?;
        Ok(result)
    }
}

/// One standard length-delimited protobuf frame, bounded before allocation.
pub fn encode(value: &Snapshot) -> Result<Vec<u8>, Error> {
    value.validate()?;
    let wire = value.to_wire();
    if wire.encoded_len() > SERVER_LIMITS.message {
        return Err(protobuf::FrameError::TooLarge {
            length: u64::try_from(wire.encoded_len()).unwrap_or(u64::MAX),
            limit: SERVER_LIMITS.message,
        }
        .into());
    }
    crate::monitor::wire::scan::check_discovery(&wire.encode_to_vec())?;
    let mut frame = Vec::new();
    protobuf::write_message(&mut frame, &wire, &SERVER_LIMITS)?;
    Ok(frame)
}

/// Canonical protobuf cost, including nested fields, after all snapshot bounds pass.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Size {
    pub bytes: usize,
    pub fields: usize,
}

pub fn measure(value: &Snapshot) -> Result<Size, Error> {
    value.validate()?;
    let wire = value.to_wire();
    let bytes = wire.encoded_len();
    if bytes > SERVER_LIMITS.message {
        return Err(protobuf::FrameError::TooLarge {
            length: u64::try_from(bytes).unwrap_or(u64::MAX),
            limit: SERVER_LIMITS.message,
        }
        .into());
    }
    let fields = crate::monitor::wire::scan::measure_discovery(&wire.encode_to_vec())?;
    Ok(Size { bytes, fields })
}

/// Leaves room for monitor and finite-helper envelopes around the observation.
pub fn fits_response(value: &Snapshot) -> Result<bool, Error> {
    const ENVELOPE_BYTES: usize = 1024;
    const ENVELOPE_FIELDS: usize = 128;
    if value.validate_records()? > MAX_REPLY_TEXT {
        return Ok(false);
    }
    let wire = value.to_wire();
    if wire.encoded_len() > SERVER_LIMITS.message - ENVELOPE_BYTES {
        return Ok(false);
    }
    match crate::monitor::wire::scan::measure_discovery(&wire.encode_to_vec()) {
        Ok(fields) => Ok(fields <= crate::monitor::wire::scan::MAX_FIELDS - ENVELOPE_FIELDS),
        Err(Error::FieldBudget) => Ok(false),
        Err(error) => Err(error),
    }
}

pub fn decode(body: Bytes) -> Result<Snapshot, Error> {
    if body.len() > SERVER_LIMITS.message {
        return Err(protobuf::FrameError::TooLarge {
            length: u64::try_from(body.len()).unwrap_or(u64::MAX),
            limit: SERVER_LIMITS.message,
        }
        .into());
    }
    crate::monitor::wire::scan::check_discovery(&body)?;
    Ok(Snapshot::from_wire(protobuf::decode(
        body,
        &SERVER_LIMITS,
    )?)?)
}

pub fn read(input: &mut impl Read) -> Result<Snapshot, Error> {
    decode(protobuf::read_frame(input, SERVER_LIMITS.message)?.ok_or(Error::Ended)?)
}

pub fn write(output: &mut impl Write, value: &Snapshot) -> Result<(), Error> {
    output
        .write_all(&encode(value)?)
        .map_err(protobuf::FrameError::from)?;
    output.flush().map_err(protobuf::FrameError::from)?;
    Ok(())
}

#[cfg(test)]
mod tests;
