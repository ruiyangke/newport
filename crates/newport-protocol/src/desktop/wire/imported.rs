use super::*;
use crate::monitor::wire::Record;
impl Convert for crate::common::Present {
    type Proto = crate::encoding::protobuf::generated::newport::wire::Present;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::wire::Present {})
    }
    fn from_proto(_value: Self::Proto) -> Result<Self, Error> {
        let value = Self {};
        Ok(value)
    }
}
impl Convert for crate::common::Secret {
    type Proto = crate::encoding::protobuf::generated::newport::wire::Secret;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        let value = self;
        if value.value.len() > 16 * 1024 {
            return Err(Invalid("secret size").into());
        }
        Ok(
            crate::encoding::protobuf::generated::newport::wire::Secret {
                value: self.value.clone(),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        let value = Self { value: value.value };
        if value.value.len() > 16 * 1024 {
            return Err(Invalid("secret size").into());
        }
        Ok(value)
    }
}
impl Convert for crate::common::Error {
    type Proto = crate::encoding::protobuf::generated::newport::wire::Error;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        let value = self;
        if value.code.is_empty()
            || value.code.len() > 64
            || value.message.len() > 8 * 1024
            || value.code.contains('\0')
            || value.message.contains('\0')
        {
            return Err(Invalid("failure detail").into());
        }
        Ok(crate::encoding::protobuf::generated::newport::wire::Error {
            code: self.code.clone(),
            message: self.message.clone(),
            retry: match self.retry {
                crate::common::Retry::Unknown => 0,
                crate::common::Retry::Yes => 1,
                crate::common::Retry::No => 2,
            },
        })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        let value = Self {
            code: value.code,
            message: value.message,
            retry: match value.retry {
                0 => crate::common::Retry::Unknown,
                1 => crate::common::Retry::Yes,
                2 => crate::common::Retry::No,
                _ => return Err(Invalid("retry advice").into()),
            },
        };
        if value.code.is_empty()
            || value.code.len() > 64
            || value.message.len() > 8 * 1024
            || value.code.contains('\0')
            || value.message.contains('\0')
        {
            return Err(Invalid("failure detail").into());
        }
        Ok(value)
    }
}
impl Convert for crate::product::Variant {
    type Proto = i32;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(match self {
            Self::Core => 1,
            Self::Full => 2,
        })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        let value = match value {
            1 => Self::Core,
            2 => Self::Full,
            _ => return Err(Invalid("product").into()),
        };
        Ok(value)
    }
}
impl Convert for crate::runtime::Hello {
    type Proto = crate::encoding::protobuf::generated::newport::runtime::Hello;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        let value = self;
        value.validate()?;
        Ok(crate::runtime::wire::convert::hello(self)?)
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        let value = crate::runtime::wire::convert::from_hello(value)?;
        value.validate()?;
        Ok(value)
    }
}
impl Convert for crate::runtime::Permissions {
    type Proto = crate::encoding::protobuf::generated::newport::runtime::Permissions;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        let value = self;
        value.validate()?;
        Ok(crate::runtime::wire::convert::permission_values(self))
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        let value = crate::runtime::wire::convert::from_permission_values(Some(value))?;
        value.validate()?;
        Ok(value)
    }
}
impl Convert for crate::monitor::metrics::Snapshot {
    type Proto = crate::encoding::protobuf::generated::newport::monitor::MetricsSnapshot;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        let value = self;
        value.validate()?;
        Ok(Record::to_wire(self))
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        let value = <Self as Record>::from_wire(value)?;
        value.validate()?;
        Ok(value)
    }
}
impl Convert for crate::monitor::metrics::Core {
    type Proto = crate::encoding::protobuf::generated::newport::monitor::MetricsCore;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        let value = self;
        crate::monitor::metrics::Snapshot {
            cores: 1,
            cpus: vec![value.clone()],
            ..Default::default()
        }
        .validate()?;
        Ok(Record::to_wire(self))
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        let value = <Self as Record>::from_wire(value)?;
        crate::monitor::metrics::Snapshot {
            cores: 1,
            cpus: vec![value.clone()],
            ..Default::default()
        }
        .validate()?;
        Ok(value)
    }
}
impl Convert for crate::monitor::metrics::Disk {
    type Proto = crate::encoding::protobuf::generated::newport::monitor::MetricsDisk;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        let value = self;
        crate::monitor::metrics::Snapshot {
            cores: 1,
            disks: vec![value.clone()],
            ..Default::default()
        }
        .validate()?;
        Ok(Record::to_wire(self))
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        let value = <Self as Record>::from_wire(value)?;
        crate::monitor::metrics::Snapshot {
            cores: 1,
            disks: vec![value.clone()],
            ..Default::default()
        }
        .validate()?;
        Ok(value)
    }
}
impl Convert for crate::monitor::metrics::Process {
    type Proto = crate::encoding::protobuf::generated::newport::monitor::MetricsProcess;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        let value = self;
        crate::monitor::metrics::Snapshot {
            cores: 1,
            processes: vec![value.clone()],
            ..Default::default()
        }
        .validate()?;
        Ok(Record::to_wire(self))
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        let value = <Self as Record>::from_wire(value)?;
        crate::monitor::metrics::Snapshot {
            cores: 1,
            processes: vec![value.clone()],
            ..Default::default()
        }
        .validate()?;
        Ok(value)
    }
}
impl Convert for crate::monitor::services::Scope {
    type Proto = i32;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(Record::to_wire(self))
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        let value = <Self as Record>::from_wire(value)?;
        Ok(value)
    }
}
impl Convert for crate::monitor::services::Verb {
    type Proto = i32;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(Record::to_wire(self))
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        let value = <Self as Record>::from_wire(value)?;
        Ok(value)
    }
}
impl Convert for crate::monitor::services::LogsRequest {
    type Proto = crate::encoding::protobuf::generated::newport::monitor::ServicesLogsRequest;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        let value = self;
        value
            .validate()
            .map_err(crate::monitor::InvalidRequest::Services)?;
        Ok(Record::to_wire(self))
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        let value = <Self as Record>::from_wire(value)?;
        value
            .validate()
            .map_err(crate::monitor::InvalidRequest::Services)?;
        Ok(value)
    }
}
impl Convert for crate::monitor::services::Snapshot {
    type Proto = crate::encoding::protobuf::generated::newport::monitor::ServicesSnapshot;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        let value = self;
        value.validate()?;
        Ok(Record::to_wire(self))
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        let value = <Self as Record>::from_wire(value)?;
        value.validate()?;
        Ok(value)
    }
}
impl Convert for crate::monitor::services::Logs {
    type Proto = crate::encoding::protobuf::generated::newport::monitor::ServicesLogs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        let value = self;
        value.validate()?;
        Ok(Record::to_wire(self))
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        let value = <Self as Record>::from_wire(value)?;
        value.validate()?;
        Ok(value)
    }
}
impl Convert for crate::monitor::services::UnitFile {
    type Proto = crate::encoding::protobuf::generated::newport::monitor::ServicesUnitFile;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        let value = self;
        value.validate()?;
        Ok(Record::to_wire(self))
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        let value = <Self as Record>::from_wire(value)?;
        value.validate()?;
        Ok(value)
    }
}
impl Convert for crate::monitor::services::ActionReply {
    type Proto = crate::encoding::protobuf::generated::newport::monitor::ServicesActionReply;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        let value = self;
        value.validate()?;
        Ok(Record::to_wire(self))
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        let value = <Self as Record>::from_wire(value)?;
        value.validate()?;
        Ok(value)
    }
}
impl Convert for crate::monitor::containers::ContainerVerb {
    type Proto = i32;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(Record::to_wire(self))
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        let value = <Self as Record>::from_wire(value)?;
        Ok(value)
    }
}
impl Convert for crate::monitor::containers::ProjectVerb {
    type Proto = i32;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(Record::to_wire(self))
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        let value = <Self as Record>::from_wire(value)?;
        Ok(value)
    }
}
impl Convert for crate::monitor::containers::PruneTarget {
    type Proto = i32;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(Record::to_wire(self))
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        let value = <Self as Record>::from_wire(value)?;
        Ok(value)
    }
}
impl Convert for crate::monitor::containers::LogsRequest {
    type Proto = crate::encoding::protobuf::generated::newport::monitor::ContainersLogsRequest;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        let value = self;
        crate::monitor::containers::Request::Logs(value.clone())
            .validate()
            .map_err(crate::monitor::InvalidRequest::Containers)?;
        Ok(Record::to_wire(self))
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        let value = <Self as Record>::from_wire(value)?;
        crate::monitor::containers::Request::Logs(value.clone())
            .validate()
            .map_err(crate::monitor::InvalidRequest::Containers)?;
        Ok(value)
    }
}
impl Convert for crate::monitor::containers::ContainersSnapshot {
    type Proto =
        crate::encoding::protobuf::generated::newport::monitor::ContainersContainersSnapshot;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        let value = self;
        value.validate()?;
        Ok(Record::to_wire(self))
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        let value = <Self as Record>::from_wire(value)?;
        value.validate()?;
        Ok(value)
    }
}
impl Convert for crate::monitor::containers::ContainerInspect {
    type Proto = crate::encoding::protobuf::generated::newport::monitor::ContainersContainerInspect;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        let value = self;
        value.validate()?;
        Ok(Record::to_wire(self))
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        let value = <Self as Record>::from_wire(value)?;
        value.validate()?;
        Ok(value)
    }
}
impl Convert for crate::monitor::containers::ContainerLogPage {
    type Proto = crate::encoding::protobuf::generated::newport::monitor::ContainersContainerLogPage;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        let value = self;
        value.validate()?;
        Ok(Record::to_wire(self))
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        let value = <Self as Record>::from_wire(value)?;
        value.validate()?;
        Ok(value)
    }
}
impl Convert for crate::monitor::containers::Done {
    type Proto = crate::encoding::protobuf::generated::newport::monitor::ContainersDone;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        let value = self;
        if !value.ok {
            return Err(Invalid("successful result").into());
        }
        Ok(Record::to_wire(self))
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        let value = <Self as Record>::from_wire(value)?;
        if !value.ok {
            return Err(Invalid("successful result").into());
        }
        Ok(value)
    }
}
impl Convert for crate::monitor::containers::Removed {
    type Proto = crate::encoding::protobuf::generated::newport::monitor::ContainersRemoved;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(Record::to_wire(self))
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        let value = <Self as Record>::from_wire(value)?;
        Ok(value)
    }
}
impl Convert for crate::monitor::containers::DockerDisk {
    type Proto = crate::encoding::protobuf::generated::newport::monitor::ContainersDockerDisk;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        let value = self;
        value.validate()?;
        Ok(Record::to_wire(self))
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        let value = <Self as Record>::from_wire(value)?;
        value.validate()?;
        Ok(value)
    }
}
impl Convert for crate::monitor::containers::Pruned {
    type Proto = crate::encoding::protobuf::generated::newport::monitor::ContainersPruned;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(Record::to_wire(self))
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        let value = <Self as Record>::from_wire(value)?;
        Ok(value)
    }
}
impl Convert for crate::monitor::compose::Action {
    type Proto = crate::encoding::protobuf::generated::newport::monitor::ComposeAction;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        let value = self;
        value
            .validate()
            .map_err(crate::monitor::InvalidRequest::Compose)?;
        Ok(Record::to_wire(self))
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        let value = <Self as Record>::from_wire(value)?;
        value
            .validate()
            .map_err(crate::monitor::InvalidRequest::Compose)?;
        Ok(value)
    }
}
impl Convert for crate::monitor::compose::Project {
    type Proto = crate::encoding::protobuf::generated::newport::monitor::ComposeProject;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        let value = self;
        value.validate()?;
        Ok(Record::to_wire(self))
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        let value = <Self as Record>::from_wire(value)?;
        value.validate()?;
        Ok(value)
    }
}
impl Convert for crate::monitor::compose::Started {
    type Proto = crate::encoding::protobuf::generated::newport::monitor::ComposeStarted;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        let value = self;
        if !crate::monitor::compose::valid_job(&value.job) {
            return Err(Invalid("Compose job identity").into());
        }
        Ok(Record::to_wire(self))
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        let value = <Self as Record>::from_wire(value)?;
        if !crate::monitor::compose::valid_job(&value.job) {
            return Err(Invalid("Compose job identity").into());
        }
        Ok(value)
    }
}
impl Convert for crate::monitor::compose::Job {
    type Proto = crate::encoding::protobuf::generated::newport::monitor::ComposeJob;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        let value = self;
        value.validate()?;
        Ok(Record::to_wire(self))
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        let value = <Self as Record>::from_wire(value)?;
        value.validate()?;
        Ok(value)
    }
}
impl Convert for crate::monitor::compose::Done {
    type Proto = crate::encoding::protobuf::generated::newport::monitor::ComposeDone;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        let value = self;
        if !value.ok {
            return Err(Invalid("successful result").into());
        }
        Ok(Record::to_wire(self))
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        let value = <Self as Record>::from_wire(value)?;
        if !value.ok {
            return Err(Invalid("successful result").into());
        }
        Ok(value)
    }
}
impl Convert for crate::monitor::compose::Jobs {
    type Proto = crate::encoding::protobuf::generated::newport::monitor::ComposeJobs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        let value = self;
        value.validate()?;
        Ok(Record::to_wire(self))
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        let value = <Self as Record>::from_wire(value)?;
        value.validate()?;
        Ok(value)
    }
}
impl Convert for crate::git::Request {
    type Proto = crate::encoding::protobuf::generated::newport::git::Request;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        let value = self;
        value.validate()?;
        Ok(crate::git::wire::convert::request_to_wire(self)?)
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        let value = crate::git::wire::convert::request_from_wire(value)?;
        value.validate()?;
        Ok(value)
    }
}
impl Convert for crate::git::Reply {
    type Proto = crate::encoding::protobuf::generated::newport::git::Reply;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        let value = self;
        value.validate()?;
        Ok(crate::git::wire::convert::reply_to_wire(self)?)
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        let value = crate::git::wire::convert::reply_from_wire(value)?;
        value.validate()?;
        Ok(value)
    }
}
impl Convert for crate::git::Diff {
    type Proto = crate::encoding::protobuf::generated::newport::git::Diff;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        let value = self;
        value.validate()?;
        Ok(crate::git::wire::convert::diff_to_wire(self)?)
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        let value = crate::git::wire::convert::diff_from_wire(value)?;
        value.validate()?;
        Ok(value)
    }
}
impl Convert for crate::git::Path {
    type Proto = crate::encoding::protobuf::generated::newport::git::Path;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::git::wire::convert::path_to_wire(self)?)
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        let value = crate::git::wire::convert::path_from_wire(value)?;
        Ok(value)
    }
}
impl Convert for crate::git::CommandLog {
    type Proto = crate::encoding::protobuf::generated::newport::git::CommandLog;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        let value = self;
        crate::git::ServerMessage::Log(crate::git::Log {
            id: "00000000-0000-0000-0000-000000000000".into(),
            entry: value.clone(),
        })
        .validate()?;
        Ok(crate::git::wire::convert::command_log_to_wire(self)?)
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        let value = crate::git::wire::convert::command_log_from_wire(value)?;
        crate::git::ServerMessage::Log(crate::git::Log {
            id: "00000000-0000-0000-0000-000000000000".into(),
            entry: value.clone(),
        })
        .validate()?;
        Ok(value)
    }
}
impl Convert for crate::remote_apps::apps::Catalogue {
    type Proto = crate::encoding::protobuf::generated::newport::remote::catalogue::Catalogue;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        let value = self;
        value.validate()?;
        Ok(crate::encoding::protobuf::generated::newport::remote::catalogue::Catalogue { apps: self.apps.iter().map(|v| crate::encoding::protobuf::generated::newport::remote::catalogue::Application { id: v.id.clone(), name: v.name.clone(), description: v.description.clone(), keywords: v.keywords.clone(), icon: v.icon.clone(), actions: v.actions.iter().map(|a| crate::encoding::protobuf::generated::newport::remote::catalogue::DesktopAction { id: a.id.clone(), name: a.name.clone() }).collect() }).collect() })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        let value = Self {
            apps: value
                .apps
                .into_iter()
                .map(|v| crate::remote_apps::apps::Application {
                    id: v.id,
                    name: v.name,
                    description: v.description,
                    keywords: v.keywords,
                    icon: v.icon,
                    actions: v
                        .actions
                        .into_iter()
                        .map(|a| crate::remote_apps::apps::DesktopAction {
                            id: a.id,
                            name: a.name,
                        })
                        .collect(),
                })
                .collect(),
        };
        value.validate()?;
        Ok(value)
    }
}
impl Convert for crate::terminal::Computer {
    type Proto = crate::encoding::protobuf::generated::newport::terminal::Computer;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        let value = self;
        if value.id.is_empty()
            || value.id.len() > 64
            || value.name.len() > crate::terminal::MAX_COMPUTER_NAME
            || value.id.contains('\0')
            || value.name.contains('\0')
        {
            return Err(Invalid("computer identity").into());
        }
        Ok(
            crate::encoding::protobuf::generated::newport::terminal::Computer {
                id: self.id.clone(),
                name: self.name.clone(),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        let value = Self {
            id: value.id,
            name: value.name,
        };
        if value.id.is_empty()
            || value.id.len() > 64
            || value.name.len() > crate::terminal::MAX_COMPUTER_NAME
            || value.id.contains('\0')
            || value.name.contains('\0')
        {
            return Err(Invalid("computer identity").into());
        }
        Ok(value)
    }
}
impl Convert for crate::terminal::Attached {
    type Proto = crate::encoding::protobuf::generated::newport::terminal::Attached;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        let value = self;
        if value.id.is_empty()
            || value.id.len() > 64
            || value.name.len() > crate::terminal::MAX_COMPUTER_NAME
        {
            return Err(Invalid("attached computer").into());
        }
        Ok(
            crate::encoding::protobuf::generated::newport::terminal::Attached {
                id: self.id.clone(),
                name: self.name.clone(),
                active: self.active,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        let value = Self {
            id: value.id,
            name: value.name,
            active: value.active,
        };
        if value.id.is_empty()
            || value.id.len() > 64
            || value.name.len() > crate::terminal::MAX_COMPUTER_NAME
        {
            return Err(Invalid("attached computer").into());
        }
        Ok(value)
    }
}
