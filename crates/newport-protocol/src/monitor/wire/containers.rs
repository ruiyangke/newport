use super::super::containers as contract;
use super::{Invalid, Record, pb, required};

impl Record for contract::ContainerVerb {
    type Wire = i32;
    fn to_wire(&self) -> i32 {
        match self {
            Self::Start => 1,
            Self::Stop => 2,
            Self::Restart => 3,
            Self::Pause => 4,
            Self::Unpause => 5,
        }
    }
    fn from_wire(value: i32) -> Result<Self, Invalid> {
        Ok(match value {
            1 => Self::Start,
            2 => Self::Stop,
            3 => Self::Restart,
            4 => Self::Pause,
            5 => Self::Unpause,
            _ => return Err(Invalid("containers ContainerVerb")),
        })
    }
}

impl Record for contract::ProjectVerb {
    type Wire = i32;
    fn to_wire(&self) -> i32 {
        match self {
            Self::Start => 1,
            Self::Stop => 2,
            Self::Restart => 3,
        }
    }
    fn from_wire(value: i32) -> Result<Self, Invalid> {
        Ok(match value {
            1 => Self::Start,
            2 => Self::Stop,
            3 => Self::Restart,
            _ => return Err(Invalid("containers ProjectVerb")),
        })
    }
}

impl Record for contract::PruneTarget {
    type Wire = i32;
    fn to_wire(&self) -> i32 {
        match self {
            Self::Containers => 1,
            Self::DanglingImages => 2,
            Self::UnusedImages => 3,
            Self::BuildCache => 4,
            Self::Volumes => 5,
        }
    }
    fn from_wire(value: i32) -> Result<Self, Invalid> {
        Ok(match value {
            1 => Self::Containers,
            2 => Self::DanglingImages,
            3 => Self::UnusedImages,
            4 => Self::BuildCache,
            5 => Self::Volumes,
            _ => return Err(Invalid("containers PruneTarget")),
        })
    }
}

impl Record for contract::ContainerState {
    type Wire = i32;
    fn to_wire(&self) -> i32 {
        match self {
            Self::Created => 1,
            Self::Running => 2,
            Self::Paused => 3,
            Self::Restarting => 4,
            Self::Removing => 5,
            Self::Exited => 6,
            Self::Dead => 7,
        }
    }
    fn from_wire(value: i32) -> Result<Self, Invalid> {
        Ok(match value {
            1 => Self::Created,
            2 => Self::Running,
            3 => Self::Paused,
            4 => Self::Restarting,
            5 => Self::Removing,
            6 => Self::Exited,
            7 => Self::Dead,
            _ => return Err(Invalid("containers ContainerState")),
        })
    }
}

impl Record for contract::ContainerHealth {
    type Wire = i32;
    fn to_wire(&self) -> i32 {
        match self {
            Self::Healthy => 1,
            Self::Unhealthy => 2,
            Self::Starting => 3,
        }
    }
    fn from_wire(value: i32) -> Result<Self, Invalid> {
        Ok(match value {
            1 => Self::Healthy,
            2 => Self::Unhealthy,
            3 => Self::Starting,
            _ => return Err(Invalid("containers ContainerHealth")),
        })
    }
}

impl Record for contract::PortProtocol {
    type Wire = i32;
    fn to_wire(&self) -> i32 {
        match self {
            Self::Tcp => 1,
            Self::Udp => 2,
            Self::Sctp => 3,
        }
    }
    fn from_wire(value: i32) -> Result<Self, Invalid> {
        Ok(match value {
            1 => Self::Tcp,
            2 => Self::Udp,
            3 => Self::Sctp,
            _ => return Err(Invalid("containers PortProtocol")),
        })
    }
}

impl Record for contract::LogStream {
    type Wire = i32;
    fn to_wire(&self) -> i32 {
        match self {
            Self::Stdout => 1,
            Self::Stderr => 2,
        }
    }
    fn from_wire(value: i32) -> Result<Self, Invalid> {
        Ok(match value {
            1 => Self::Stdout,
            2 => Self::Stderr,
            _ => return Err(Invalid("containers LogStream")),
        })
    }
}

impl Record for contract::DiskPart {
    type Wire = i32;
    fn to_wire(&self) -> i32 {
        match self {
            Self::Quick => 1,
            Self::Volumes => 2,
        }
    }
    fn from_wire(value: i32) -> Result<Self, Invalid> {
        Ok(match value {
            1 => Self::Quick,
            2 => Self::Volumes,
            _ => return Err(Invalid("containers DiskPart")),
        })
    }
}

impl Record for contract::EngineInfo {
    type Wire = pb::ContainersEngineInfo;
    fn to_wire(&self) -> Self::Wire {
        Self::Wire {
            version: self.version.clone(),
            api_version: self.api_version.clone(),
            os: self.os.clone(),
            rootless: self.rootless,
            podman: self.podman,
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            version: value.version,
            api_version: value.api_version,
            os: value.os,
            rootless: value.rootless,
            podman: value.podman,
        })
    }
}

impl Record for contract::ContainerPort {
    type Wire = pb::ContainersContainerPort;
    fn to_wire(&self) -> Self::Wire {
        Self::Wire {
            ip: self.ip.clone(),
            host_port: self.host_port.map(u32::from),
            container_port: u32::from(self.container_port),
            protocol: self.protocol.to_wire(),
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            ip: value.ip,
            host_port: value
                .host_port
                .map(|v| u16::try_from(v).map_err(|_| Invalid("containers host_port")))
                .transpose()?,
            container_port: u16::try_from(value.container_port)
                .map_err(|_| Invalid("containers container_port"))?,
            protocol: contract::PortProtocol::from_wire(value.protocol)?,
        })
    }
}

impl Record for contract::ComposeLabels {
    type Wire = pb::ContainersComposeLabels;
    fn to_wire(&self) -> Self::Wire {
        Self::Wire {
            project: self.project.clone(),
            service: self.service.clone(),
            oneoff: self.oneoff,
            number: self.number,
            working_dir: self.working_dir.clone(),
            config_files: self.config_files.clone(),
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            project: value.project,
            service: value.service,
            oneoff: value.oneoff,
            number: value.number,
            working_dir: value.working_dir,
            config_files: value.config_files,
        })
    }
}

impl Record for contract::Container {
    type Wire = pb::ContainersContainer;
    fn to_wire(&self) -> Self::Wire {
        Self::Wire {
            id: self.id.clone(),
            name: self.name.clone(),
            image: self.image.clone(),
            image_id: self.image_id.clone(),
            command: self.command.clone(),
            state: self.state.to_wire(),
            status: self.status.clone(),
            health: self.health.as_ref().map(Record::to_wire),
            exit_code: self.exit_code,
            oom_killed: self.oom_killed,
            error: self.error.clone(),
            restart_count: self.restart_count,
            restart_policy: self.restart_policy.clone(),
            created: self.created,
            started_at: self.started_at,
            finished_at: self.finished_at,
            ports: self.ports.iter().map(Record::to_wire).collect(),
            compose: self.compose.as_ref().map(Record::to_wire),
            cpu: self.cpu,
            memory: self.memory,
            memory_limit: self.memory_limit,
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            id: value.id,
            name: value.name,
            image: value.image,
            image_id: value.image_id,
            command: value.command,
            state: contract::ContainerState::from_wire(value.state)?,
            status: value.status,
            health: value
                .health
                .map(contract::ContainerHealth::from_wire)
                .transpose()?,
            exit_code: value.exit_code,
            oom_killed: value.oom_killed,
            error: value.error,
            restart_count: value.restart_count,
            restart_policy: value.restart_policy,
            created: value.created,
            started_at: value.started_at,
            finished_at: value.finished_at,
            ports: value
                .ports
                .into_iter()
                .map(contract::ContainerPort::from_wire)
                .collect::<Result<_, _>>()?,
            compose: value
                .compose
                .map(contract::ComposeLabels::from_wire)
                .transpose()?,
            cpu: value.cpu,
            memory: value.memory,
            memory_limit: value.memory_limit,
        })
    }
}

impl Record for contract::ContainersSnapshot {
    type Wire = pb::ContainersContainersSnapshot;
    fn to_wire(&self) -> Self::Wire {
        Self::Wire {
            engine: Some(self.engine.to_wire()),
            containers: self.containers.iter().map(Record::to_wire).collect(),
            truncated: self.truncated,
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            engine: contract::EngineInfo::from_wire(required(value.engine)?)?,
            containers: value
                .containers
                .into_iter()
                .map(contract::Container::from_wire)
                .collect::<Result<_, _>>()?,
            truncated: value.truncated,
        })
    }
}

impl Record for contract::EnvVar {
    type Wire = pb::ContainersEnvVar;
    fn to_wire(&self) -> Self::Wire {
        Self::Wire {
            name: self.name.clone(),
            value: self.value.clone(),
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            name: value.name,
            value: value.value,
        })
    }
}

impl Record for contract::Mount {
    type Wire = pb::ContainersMount;
    fn to_wire(&self) -> Self::Wire {
        Self::Wire {
            kind: self.kind.clone(),
            source: self.source.clone(),
            destination: self.destination.clone(),
            mode: self.mode.clone(),
            rw: self.rw,
            name: self.name.clone(),
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            kind: value.kind,
            source: value.source,
            destination: value.destination,
            mode: value.mode,
            rw: value.rw,
            name: value.name,
        })
    }
}

impl Record for contract::Network {
    type Wire = pb::ContainersNetwork;
    fn to_wire(&self) -> Self::Wire {
        Self::Wire {
            name: self.name.clone(),
            ip_address: self.ip_address.clone(),
            aliases: self.aliases.clone(),
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            name: value.name,
            ip_address: value.ip_address,
            aliases: value.aliases,
        })
    }
}

impl Record for contract::Healthcheck {
    type Wire = pb::ContainersHealthcheck;
    fn to_wire(&self) -> Self::Wire {
        Self::Wire {
            test: self.test.clone(),
            interval: self.interval,
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            test: value.test,
            interval: value.interval,
        })
    }
}

impl Record for contract::HealthResult {
    type Wire = pb::ContainersHealthResult;
    fn to_wire(&self) -> Self::Wire {
        Self::Wire {
            at: self.at,
            exit_code: self.exit_code,
            output: self.output.clone(),
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            at: value.at,
            exit_code: value.exit_code,
            output: value.output,
        })
    }
}

impl Record for contract::ContainerInspect {
    type Wire = pb::ContainersContainerInspect;
    fn to_wire(&self) -> Self::Wire {
        Self::Wire {
            id: self.id.clone(),
            name: self.name.clone(),
            image: self.image.clone(),
            created: self.created,
            entrypoint: self.entrypoint.as_ref().map(|values| pb::StringList {
                values: values.clone(),
            }),
            cmd: self.cmd.as_ref().map(|values| pb::StringList {
                values: values.clone(),
            }),
            working_dir: self.working_dir.clone(),
            user: self.user.clone(),
            hostname: self.hostname.clone(),
            env: self.env.iter().map(Record::to_wire).collect(),
            mounts: self.mounts.iter().map(Record::to_wire).collect(),
            networks: self.networks.iter().map(Record::to_wire).collect(),
            restart_policy: self.restart_policy.clone(),
            labels: self
                .labels
                .iter()
                .map(|(key, value)| pb::Label {
                    key: key.clone(),
                    value: value.clone(),
                })
                .collect(),
            healthcheck: self.healthcheck.as_ref().map(Record::to_wire),
            last_health: self.last_health.iter().map(Record::to_wire).collect(),
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            id: value.id,
            name: value.name,
            image: value.image,
            created: value.created,
            entrypoint: value.entrypoint.map(|v| v.values),
            cmd: value.cmd.map(|v| v.values),
            working_dir: value.working_dir,
            user: value.user,
            hostname: value.hostname,
            env: value
                .env
                .into_iter()
                .map(contract::EnvVar::from_wire)
                .collect::<Result<_, _>>()?,
            mounts: value
                .mounts
                .into_iter()
                .map(contract::Mount::from_wire)
                .collect::<Result<_, _>>()?,
            networks: value
                .networks
                .into_iter()
                .map(contract::Network::from_wire)
                .collect::<Result<_, _>>()?,
            restart_policy: value.restart_policy,
            labels: super::labels(value.labels)?,
            healthcheck: value
                .healthcheck
                .map(contract::Healthcheck::from_wire)
                .transpose()?,
            last_health: value
                .last_health
                .into_iter()
                .map(contract::HealthResult::from_wire)
                .collect::<Result<_, _>>()?,
        })
    }
}

impl Record for contract::ContainerLogEntry {
    type Wire = pb::ContainersContainerLogEntry;
    fn to_wire(&self) -> Self::Wire {
        Self::Wire {
            at: self.at,
            stream: self.stream.to_wire(),
            message: self.message.clone(),
            container: self.container.clone(),
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            at: value.at,
            stream: contract::LogStream::from_wire(value.stream)?,
            message: value.message,
            container: value.container,
        })
    }
}

impl Record for contract::ContainerLogPage {
    type Wire = pb::ContainersContainerLogPage;
    fn to_wire(&self) -> Self::Wire {
        Self::Wire {
            entries: self.entries.iter().map(Record::to_wire).collect(),
            cursor: self.cursor.clone(),
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            entries: value
                .entries
                .into_iter()
                .map(contract::ContainerLogEntry::from_wire)
                .collect::<Result<_, _>>()?,
            cursor: value.cursor,
        })
    }
}

impl Record for contract::DockerImage {
    type Wire = pb::ContainersDockerImage;
    fn to_wire(&self) -> Self::Wire {
        Self::Wire {
            id: self.id.clone(),
            tags: self.tags.clone(),
            size: self.size,
            shared_size: self.shared_size,
            created: self.created,
            containers: self.containers,
            dangling: self.dangling,
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            id: value.id,
            tags: value.tags,
            size: value.size,
            shared_size: value.shared_size,
            created: value.created,
            containers: value.containers,
            dangling: value.dangling,
        })
    }
}

impl Record for contract::DockerVolume {
    type Wire = pb::ContainersDockerVolume;
    fn to_wire(&self) -> Self::Wire {
        Self::Wire {
            name: self.name.clone(),
            driver: self.driver.clone(),
            size: self.size,
            ref_count: self.ref_count,
            anonymous: self.anonymous,
            compose: self.compose.clone(),
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            name: value.name,
            driver: value.driver,
            size: value.size,
            ref_count: value.ref_count,
            anonymous: value.anonymous,
            compose: value.compose,
        })
    }
}

impl Record for contract::DiskTotals {
    type Wire = pb::ContainersDiskTotals;
    fn to_wire(&self) -> Self::Wire {
        Self::Wire {
            count: self.count,
            size: self.size,
            reclaimable: self.reclaimable,
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            count: value.count,
            size: value.size,
            reclaimable: value.reclaimable,
        })
    }
}

impl Record for contract::DockerTotals {
    type Wire = pb::ContainersDockerTotals;
    fn to_wire(&self) -> Self::Wire {
        Self::Wire {
            images: Some(self.images.to_wire()),
            containers: Some(self.containers.to_wire()),
            volumes: Some(self.volumes.to_wire()),
            build_cache: Some(self.build_cache.to_wire()),
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            images: contract::DiskTotals::from_wire(required(value.images)?)?,
            containers: contract::DiskTotals::from_wire(required(value.containers)?)?,
            volumes: contract::DiskTotals::from_wire(required(value.volumes)?)?,
            build_cache: contract::DiskTotals::from_wire(required(value.build_cache)?)?,
        })
    }
}

impl Record for contract::DockerDisk {
    type Wire = pb::ContainersDockerDisk;
    fn to_wire(&self) -> Self::Wire {
        Self::Wire {
            images: self.images.iter().map(Record::to_wire).collect(),
            volumes: self.volumes.iter().map(Record::to_wire).collect(),
            totals: Some(self.totals.to_wire()),
            volumes_measured: self.volumes_measured,
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            images: value
                .images
                .into_iter()
                .map(contract::DockerImage::from_wire)
                .collect::<Result<_, _>>()?,
            volumes: value
                .volumes
                .into_iter()
                .map(contract::DockerVolume::from_wire)
                .collect::<Result<_, _>>()?,
            totals: contract::DockerTotals::from_wire(required(value.totals)?)?,
            volumes_measured: value.volumes_measured,
        })
    }
}

impl Record for contract::Pruned {
    type Wire = pb::ContainersPruned;
    fn to_wire(&self) -> Self::Wire {
        Self::Wire {
            removed: self.removed,
            reclaimed: self.reclaimed,
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            removed: value.removed,
            reclaimed: value.reclaimed,
        })
    }
}

impl Record for contract::Done {
    type Wire = pb::ContainersDone;
    fn to_wire(&self) -> Self::Wire {
        Self::Wire { ok: self.ok }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self { ok: value.ok })
    }
}

impl Record for contract::Removed {
    type Wire = pb::ContainersRemoved;
    fn to_wire(&self) -> Self::Wire {
        Self::Wire {
            removed: self.removed,
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            removed: value.removed,
        })
    }
}

impl Record for contract::LogsRequest {
    type Wire = pb::ContainersLogsRequest;
    fn to_wire(&self) -> Self::Wire {
        use pb::containers_logs_request::Target;
        let target = match &self.target {
            contract::LogTarget::Container(v) => Target::Container(v.clone()),
            contract::LogTarget::Project(v) => Target::Project(v.clone()),
        };
        Self::Wire {
            target: Some(target),
            cursor: self.cursor.clone(),
            lines: self.lines,
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        use pb::containers_logs_request::Target;
        let target = match required(value.target)? {
            Target::Container(v) => contract::LogTarget::Container(v),
            Target::Project(v) => contract::LogTarget::Project(v),
        };
        Ok(Self {
            target,
            cursor: value.cursor,
            lines: value.lines,
        })
    }
}
impl Record for contract::Request {
    type Wire = pb::ContainersRequest;
    fn to_wire(&self) -> Self::Wire {
        use pb::containers_request::Operation;
        let operation = match self {
            Self::Snapshot => Operation::Snapshot(crate::encoding::protobuf::Present {}),
            Self::Inspect { container } => Operation::Inspect(pb::ContainersTarget {
                container: container.clone(),
            }),
            Self::Logs(v) => Operation::Logs(v.to_wire()),
            Self::Action { container, verb } => Operation::Action(pb::ContainersAction {
                container: container.clone(),
                verb: verb.to_wire(),
            }),
            Self::ProjectAction {
                project,
                verb,
                expected_ids,
            } => Operation::ProjectAction(pb::ContainersProjectAction {
                project: project.clone(),
                verb: verb.to_wire(),
                expected_ids: expected_ids.clone(),
            }),
            Self::Remove { containers } => Operation::Remove(pb::ContainersRemove {
                containers: containers.clone(),
            }),
            Self::Disk { part } => Operation::Disk(pb::ContainersDiskRequest {
                part: part.as_ref().map(Record::to_wire),
            }),
            Self::Prune { what } => Operation::Prune(pb::ContainersPrune {
                what: what.to_wire(),
            }),
        };
        Self::Wire {
            operation: Some(operation),
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        use pb::containers_request::Operation;
        Ok(match required(value.operation)? {
            Operation::Snapshot(_) => Self::Snapshot,
            Operation::Inspect(v) => Self::Inspect {
                container: v.container,
            },
            Operation::Logs(v) => Self::Logs(contract::LogsRequest::from_wire(v)?),
            Operation::Action(v) => Self::Action {
                container: v.container,
                verb: contract::ContainerVerb::from_wire(v.verb)?,
            },
            Operation::ProjectAction(v) => Self::ProjectAction {
                project: v.project,
                verb: contract::ProjectVerb::from_wire(v.verb)?,
                expected_ids: v.expected_ids,
            },
            Operation::Remove(v) => Self::Remove {
                containers: v.containers,
            },
            Operation::Disk(v) => Self::Disk {
                part: v.part.map(contract::DiskPart::from_wire).transpose()?,
            },
            Operation::Prune(v) => Self::Prune {
                what: contract::PruneTarget::from_wire(v.what)?,
            },
        })
    }
}
impl Record for contract::Reply {
    type Wire = pb::ContainersReply;
    fn to_wire(&self) -> Self::Wire {
        use pb::containers_reply::Result;
        let result = match self {
            Self::Snapshot(v) => Result::Snapshot(v.to_wire()),
            Self::Inspect(v) => Result::Inspect(v.to_wire()),
            Self::Logs(v) => Result::Logs(v.to_wire()),
            Self::Action(v) => Result::Action(v.to_wire()),
            Self::ProjectAction(v) => Result::ProjectAction(v.to_wire()),
            Self::Removed(v) => Result::Removed(v.to_wire()),
            Self::Disk(v) => Result::Disk(v.to_wire()),
            Self::Pruned(v) => Result::Pruned(v.to_wire()),
        };
        Self::Wire {
            result: Some(result),
        }
    }
    fn from_wire(value: Self::Wire) -> std::result::Result<Self, Invalid> {
        use pb::containers_reply::Result;
        Ok(match required(value.result)? {
            Result::Snapshot(v) => Self::Snapshot(contract::ContainersSnapshot::from_wire(v)?),
            Result::Inspect(v) => {
                Self::Inspect(Box::new(contract::ContainerInspect::from_wire(v)?))
            }
            Result::Logs(v) => Self::Logs(contract::ContainerLogPage::from_wire(v)?),
            Result::Action(v) => Self::Action(contract::Done::from_wire(v)?),
            Result::ProjectAction(v) => Self::ProjectAction(contract::Done::from_wire(v)?),
            Result::Removed(v) => Self::Removed(contract::Removed::from_wire(v)?),
            Result::Disk(v) => Self::Disk(contract::DockerDisk::from_wire(v)?),
            Result::Pruned(v) => Self::Pruned(contract::Pruned::from_wire(v)?),
        })
    }
}
