use super::*;
impl Convert for crate::desktop::containers::DiskPart {
    type Proto = i32;
    fn to_proto(&self) -> Result<i32, Error> {
        Ok(match self {
            Self::Full => 1,
            Self::Quick => 2,
            Self::Volumes => 3,
        })
    }
    fn from_proto(value: i32) -> Result<Self, Error> {
        Ok(match value {
            1 => Self::Full,
            2 => Self::Quick,
            3 => Self::Volumes,
            _ => return Err(Invalid("DiskPart").into()),
        })
    }
}

impl Convert for crate::desktop::containers::SnapshotArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::containers::SnapshotArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::containers::SnapshotArgs {
                server: Some(self.server.to_string()),
                refresh: Some(self.refresh),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            refresh: value.refresh.unwrap_or(false),
        })
    }
}

impl Convert for crate::desktop::containers::InspectArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::containers::InspectArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::containers::InspectArgs {
                server: Some(self.server.to_string()),
                container: Some(self.container.clone()),
                refresh: Some(self.refresh),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            container: required(value.container)?,
            refresh: value.refresh.unwrap_or(false),
        })
    }
}

impl Convert for crate::desktop::containers::LogsArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::containers::LogsArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::containers::LogsArgs {
                server: Some(self.server.to_string()),
                request: Some(self.request.to_proto()?),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            request: <crate::monitor::containers::LogsRequest as Convert>::from_proto(required(
                value.request,
            )?)?,
        })
    }
}

impl Convert for crate::desktop::containers::ActionArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::containers::ActionArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::containers::ActionArgs {
                server: Some(self.server.to_string()),
                container: Some(self.container.clone()),
                verb: Some(self.verb.to_proto()?),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            container: required(value.container)?,
            verb: <crate::monitor::containers::ContainerVerb as Convert>::from_proto(required(
                value.verb,
            )?)?,
        })
    }
}

impl Convert for crate::desktop::containers::ProjectActionArgs {
    type Proto =
        crate::encoding::protobuf::generated::newport::desktop::containers::ProjectActionArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::containers::ProjectActionArgs {
                server: Some(self.server.to_string()),
                project: Some(self.project.clone()),
                verb: Some(self.verb.to_proto()?),
                expected_ids: self
                    .expected_ids
                    .iter()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .collect::<Result<_, _>>()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            project: required(value.project)?,
            verb: <crate::monitor::containers::ProjectVerb as Convert>::from_proto(required(
                value.verb,
            )?)?,
            expected_ids: value
                .expected_ids
                .into_iter()
                .map(|v| -> Result<_, Error> { Ok(v) })
                .collect::<Result<_, _>>()?,
        })
    }
}

impl Convert for crate::desktop::containers::RemoveArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::containers::RemoveArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::containers::RemoveArgs {
                server: Some(self.server.to_string()),
                containers: self
                    .containers
                    .iter()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .collect::<Result<_, _>>()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            containers: value
                .containers
                .into_iter()
                .map(|v| -> Result<_, Error> { Ok(v) })
                .collect::<Result<_, _>>()?,
        })
    }
}

impl Convert for crate::desktop::containers::DiskArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::containers::DiskArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::containers::DiskArgs {
                server: Some(self.server.to_string()),
                part: Some(self.part.to_proto()?),
                refresh: Some(self.refresh),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            part: <crate::desktop::containers::DiskPart as Convert>::from_proto(
                value.part.unwrap_or(1),
            )?,
            refresh: value.refresh.unwrap_or(false),
        })
    }
}

impl Convert for crate::desktop::containers::PruneArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::containers::PruneArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::containers::PruneArgs {
                server: Some(self.server.to_string()),
                what: Some(self.what.to_proto()?),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            what: <crate::monitor::containers::PruneTarget as Convert>::from_proto(required(
                value.what,
            )?)?,
        })
    }
}

impl Convert for crate::desktop::containers::ComposeProjectArgs {
    type Proto =
        crate::encoding::protobuf::generated::newport::desktop::containers::ComposeProjectArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::containers::ComposeProjectArgs { server: Some(self.server.to_string()), project: Some(self.project.clone()), refresh: Some(self.refresh) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            project: required(value.project)?,
            refresh: value.refresh.unwrap_or(false),
        })
    }
}

impl Convert for crate::desktop::containers::ComposeRunArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::containers::ComposeRunArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::containers::ComposeRunArgs {
                server: Some(self.server.to_string()),
                project: Some(self.project.clone()),
                action: Some(self.action.to_proto()?),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            project: required(value.project)?,
            action: <crate::monitor::compose::Action as Convert>::from_proto(required(
                value.action,
            )?)?,
        })
    }
}

impl Convert for crate::desktop::containers::ComposeJobArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::containers::ComposeJobArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::containers::ComposeJobArgs {
                server: Some(self.server.to_string()),
                job: Some(self.job.clone()),
                offset: Some(self.offset),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            job: required(value.job)?,
            offset: value.offset.unwrap_or(0),
        })
    }
}

impl Convert for crate::desktop::containers::ComposeCancelArgs {
    type Proto =
        crate::encoding::protobuf::generated::newport::desktop::containers::ComposeCancelArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::containers::ComposeCancelArgs {
                server: Some(self.server.to_string()),
                job: Some(self.job.clone()),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            job: required(value.job)?,
        })
    }
}

impl Convert for crate::desktop::containers::ComposeJobsArgs {
    type Proto =
        crate::encoding::protobuf::generated::newport::desktop::containers::ComposeJobsArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::containers::ComposeJobsArgs {
                server: Some(self.server.to_string()),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
        })
    }
}

impl Convert for crate::desktop::containers::Operation {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::containers::Operation;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::containers::Operation { value: Some(match self { crate::desktop::containers::Operation::Snapshot(v) => crate::encoding::protobuf::generated::newport::desktop::containers::operation::Value::Snapshot(Box::new(v.as_ref().to_proto()?)), crate::desktop::containers::Operation::Inspect(v) => crate::encoding::protobuf::generated::newport::desktop::containers::operation::Value::Inspect(Box::new(v.as_ref().to_proto()?)), crate::desktop::containers::Operation::Logs(v) => crate::encoding::protobuf::generated::newport::desktop::containers::operation::Value::Logs(Box::new(v.as_ref().to_proto()?)), crate::desktop::containers::Operation::Action(v) => crate::encoding::protobuf::generated::newport::desktop::containers::operation::Value::Action(Box::new(v.as_ref().to_proto()?)), crate::desktop::containers::Operation::ProjectAction(v) => crate::encoding::protobuf::generated::newport::desktop::containers::operation::Value::ProjectAction(Box::new(v.as_ref().to_proto()?)), crate::desktop::containers::Operation::Remove(v) => crate::encoding::protobuf::generated::newport::desktop::containers::operation::Value::Remove(Box::new(v.as_ref().to_proto()?)), crate::desktop::containers::Operation::Disk(v) => crate::encoding::protobuf::generated::newport::desktop::containers::operation::Value::Disk(Box::new(v.as_ref().to_proto()?)), crate::desktop::containers::Operation::Prune(v) => crate::encoding::protobuf::generated::newport::desktop::containers::operation::Value::Prune(Box::new(v.as_ref().to_proto()?)), crate::desktop::containers::Operation::ComposeProject(v) => crate::encoding::protobuf::generated::newport::desktop::containers::operation::Value::ComposeProject(Box::new(v.as_ref().to_proto()?)), crate::desktop::containers::Operation::ComposeRun(v) => crate::encoding::protobuf::generated::newport::desktop::containers::operation::Value::ComposeRun(Box::new(v.as_ref().to_proto()?)), crate::desktop::containers::Operation::ComposeJob(v) => crate::encoding::protobuf::generated::newport::desktop::containers::operation::Value::ComposeJob(Box::new(v.as_ref().to_proto()?)), crate::desktop::containers::Operation::ComposeCancel(v) => crate::encoding::protobuf::generated::newport::desktop::containers::operation::Value::ComposeCancel(Box::new(v.as_ref().to_proto()?)), crate::desktop::containers::Operation::ComposeJobs(v) => crate::encoding::protobuf::generated::newport::desktop::containers::operation::Value::ComposeJobs(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::containers::operation::Value::Snapshot(v) => crate::desktop::containers::Operation::Snapshot(Box::new(<crate::desktop::containers::SnapshotArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::containers::operation::Value::Inspect(v) => crate::desktop::containers::Operation::Inspect(Box::new(<crate::desktop::containers::InspectArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::containers::operation::Value::Logs(v) => crate::desktop::containers::Operation::Logs(Box::new(<crate::desktop::containers::LogsArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::containers::operation::Value::Action(v) => crate::desktop::containers::Operation::Action(Box::new(<crate::desktop::containers::ActionArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::containers::operation::Value::ProjectAction(v) => crate::desktop::containers::Operation::ProjectAction(Box::new(<crate::desktop::containers::ProjectActionArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::containers::operation::Value::Remove(v) => crate::desktop::containers::Operation::Remove(Box::new(<crate::desktop::containers::RemoveArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::containers::operation::Value::Disk(v) => crate::desktop::containers::Operation::Disk(Box::new(<crate::desktop::containers::DiskArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::containers::operation::Value::Prune(v) => crate::desktop::containers::Operation::Prune(Box::new(<crate::desktop::containers::PruneArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::containers::operation::Value::ComposeProject(v) => crate::desktop::containers::Operation::ComposeProject(Box::new(<crate::desktop::containers::ComposeProjectArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::containers::operation::Value::ComposeRun(v) => crate::desktop::containers::Operation::ComposeRun(Box::new(<crate::desktop::containers::ComposeRunArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::containers::operation::Value::ComposeJob(v) => crate::desktop::containers::Operation::ComposeJob(Box::new(<crate::desktop::containers::ComposeJobArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::containers::operation::Value::ComposeCancel(v) => crate::desktop::containers::Operation::ComposeCancel(Box::new(<crate::desktop::containers::ComposeCancelArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::containers::operation::Value::ComposeJobs(v) => crate::desktop::containers::Operation::ComposeJobs(Box::new(<crate::desktop::containers::ComposeJobsArgs as Convert>::from_proto(*v)?)) })
    }
}

impl Convert for crate::desktop::containers::Reply {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::containers::Reply;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::containers::Reply { value: Some(match self { crate::desktop::containers::Reply::Snapshot(v) => crate::encoding::protobuf::generated::newport::desktop::containers::reply::Value::Snapshot(Box::new(v.as_ref().to_proto()?)), crate::desktop::containers::Reply::Inspect(v) => crate::encoding::protobuf::generated::newport::desktop::containers::reply::Value::Inspect(Box::new(v.as_ref().to_proto()?)), crate::desktop::containers::Reply::Logs(v) => crate::encoding::protobuf::generated::newport::desktop::containers::reply::Value::Logs(Box::new(v.as_ref().to_proto()?)), crate::desktop::containers::Reply::Action(v) => crate::encoding::protobuf::generated::newport::desktop::containers::reply::Value::Action(Box::new(v.as_ref().to_proto()?)), crate::desktop::containers::Reply::ProjectAction(v) => crate::encoding::protobuf::generated::newport::desktop::containers::reply::Value::ProjectAction(Box::new(v.as_ref().to_proto()?)), crate::desktop::containers::Reply::Remove(v) => crate::encoding::protobuf::generated::newport::desktop::containers::reply::Value::Remove(Box::new(v.as_ref().to_proto()?)), crate::desktop::containers::Reply::Disk(v) => crate::encoding::protobuf::generated::newport::desktop::containers::reply::Value::Disk(Box::new(v.as_ref().to_proto()?)), crate::desktop::containers::Reply::Prune(v) => crate::encoding::protobuf::generated::newport::desktop::containers::reply::Value::Prune(Box::new(v.as_ref().to_proto()?)), crate::desktop::containers::Reply::ComposeProject(v) => crate::encoding::protobuf::generated::newport::desktop::containers::reply::Value::ComposeProject(Box::new(v.as_ref().to_proto()?)), crate::desktop::containers::Reply::ComposeRun(v) => crate::encoding::protobuf::generated::newport::desktop::containers::reply::Value::ComposeRun(Box::new(v.as_ref().to_proto()?)), crate::desktop::containers::Reply::ComposeJob(v) => crate::encoding::protobuf::generated::newport::desktop::containers::reply::Value::ComposeJob(Box::new(v.as_ref().to_proto()?)), crate::desktop::containers::Reply::ComposeCancel(v) => crate::encoding::protobuf::generated::newport::desktop::containers::reply::Value::ComposeCancel(Box::new(v.as_ref().to_proto()?)), crate::desktop::containers::Reply::ComposeJobs(v) => crate::encoding::protobuf::generated::newport::desktop::containers::reply::Value::ComposeJobs(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::containers::reply::Value::Snapshot(v) => crate::desktop::containers::Reply::Snapshot(Box::new(<crate::monitor::containers::ContainersSnapshot as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::containers::reply::Value::Inspect(v) => crate::desktop::containers::Reply::Inspect(Box::new(<crate::monitor::containers::ContainerInspect as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::containers::reply::Value::Logs(v) => crate::desktop::containers::Reply::Logs(Box::new(<crate::monitor::containers::ContainerLogPage as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::containers::reply::Value::Action(v) => crate::desktop::containers::Reply::Action(Box::new(<crate::monitor::containers::Done as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::containers::reply::Value::ProjectAction(v) => crate::desktop::containers::Reply::ProjectAction(Box::new(<crate::monitor::containers::Done as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::containers::reply::Value::Remove(v) => crate::desktop::containers::Reply::Remove(Box::new(<crate::monitor::containers::Removed as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::containers::reply::Value::Disk(v) => crate::desktop::containers::Reply::Disk(Box::new(<crate::monitor::containers::DockerDisk as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::containers::reply::Value::Prune(v) => crate::desktop::containers::Reply::Prune(Box::new(<crate::monitor::containers::Pruned as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::containers::reply::Value::ComposeProject(v) => crate::desktop::containers::Reply::ComposeProject(Box::new(<crate::monitor::compose::Project as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::containers::reply::Value::ComposeRun(v) => crate::desktop::containers::Reply::ComposeRun(Box::new(<crate::monitor::compose::Started as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::containers::reply::Value::ComposeJob(v) => crate::desktop::containers::Reply::ComposeJob(Box::new(<crate::monitor::compose::Job as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::containers::reply::Value::ComposeCancel(v) => crate::desktop::containers::Reply::ComposeCancel(Box::new(<crate::monitor::compose::Done as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::containers::reply::Value::ComposeJobs(v) => crate::desktop::containers::Reply::ComposeJobs(Box::new(<crate::monitor::compose::Jobs as Convert>::from_proto(*v)?)) })
    }
}
