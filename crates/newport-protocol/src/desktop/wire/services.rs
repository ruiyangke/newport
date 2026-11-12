use super::*;
impl Convert for crate::desktop::services::SnapshotArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::services::SnapshotArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::services::SnapshotArgs {
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

impl Convert for crate::desktop::services::LogsArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::services::LogsArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::services::LogsArgs {
                server: Some(self.server.to_string()),
                request: Some(self.request.to_proto()?),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            request: <crate::monitor::services::LogsRequest as Convert>::from_proto(required(
                value.request,
            )?)?,
        })
    }
}

impl Convert for crate::desktop::services::UnitFileArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::services::UnitFileArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::services::UnitFileArgs {
                server: Some(self.server.to_string()),
                unit: Some(self.unit.clone()),
                scope: Some(self.scope.to_proto()?),
                refresh: Some(self.refresh),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            unit: required(value.unit)?,
            scope: <crate::monitor::services::Scope as Convert>::from_proto(required(
                value.scope,
            )?)?,
            refresh: value.refresh.unwrap_or(false),
        })
    }
}

impl Convert for crate::desktop::services::ActionArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::services::ActionArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::services::ActionArgs {
                server: Some(self.server.to_string()),
                unit: Some(self.unit.clone()),
                scope: Some(self.scope.to_proto()?),
                verb: Some(self.verb.to_proto()?),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            unit: required(value.unit)?,
            scope: <crate::monitor::services::Scope as Convert>::from_proto(required(
                value.scope,
            )?)?,
            verb: <crate::monitor::services::Verb as Convert>::from_proto(required(value.verb)?)?,
        })
    }
}

impl Convert for crate::desktop::services::Operation {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::services::Operation;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::services::Operation { value: Some(match self { crate::desktop::services::Operation::Snapshot(v) => crate::encoding::protobuf::generated::newport::desktop::services::operation::Value::Snapshot(Box::new(v.as_ref().to_proto()?)), crate::desktop::services::Operation::Logs(v) => crate::encoding::protobuf::generated::newport::desktop::services::operation::Value::Logs(Box::new(v.as_ref().to_proto()?)), crate::desktop::services::Operation::UnitFile(v) => crate::encoding::protobuf::generated::newport::desktop::services::operation::Value::UnitFile(Box::new(v.as_ref().to_proto()?)), crate::desktop::services::Operation::Action(v) => crate::encoding::protobuf::generated::newport::desktop::services::operation::Value::Action(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::services::operation::Value::Snapshot(v) => crate::desktop::services::Operation::Snapshot(Box::new(<crate::desktop::services::SnapshotArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::services::operation::Value::Logs(v) => crate::desktop::services::Operation::Logs(Box::new(<crate::desktop::services::LogsArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::services::operation::Value::UnitFile(v) => crate::desktop::services::Operation::UnitFile(Box::new(<crate::desktop::services::UnitFileArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::services::operation::Value::Action(v) => crate::desktop::services::Operation::Action(Box::new(<crate::desktop::services::ActionArgs as Convert>::from_proto(*v)?)) })
    }
}

impl Convert for crate::desktop::services::Reply {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::services::Reply;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::services::Reply { value: Some(match self { crate::desktop::services::Reply::Snapshot(v) => crate::encoding::protobuf::generated::newport::desktop::services::reply::Value::Snapshot(Box::new(v.as_ref().to_proto()?)), crate::desktop::services::Reply::Logs(v) => crate::encoding::protobuf::generated::newport::desktop::services::reply::Value::Logs(Box::new(v.as_ref().to_proto()?)), crate::desktop::services::Reply::UnitFile(v) => crate::encoding::protobuf::generated::newport::desktop::services::reply::Value::UnitFile(Box::new(v.as_ref().to_proto()?)), crate::desktop::services::Reply::Action(v) => crate::encoding::protobuf::generated::newport::desktop::services::reply::Value::Action(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::services::reply::Value::Snapshot(v) => crate::desktop::services::Reply::Snapshot(Box::new(<crate::monitor::services::Snapshot as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::services::reply::Value::Logs(v) => crate::desktop::services::Reply::Logs(Box::new(<crate::monitor::services::Logs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::services::reply::Value::UnitFile(v) => crate::desktop::services::Reply::UnitFile(Box::new(<crate::monitor::services::UnitFile as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::services::reply::Value::Action(v) => crate::desktop::services::Reply::Action(Box::new(<crate::monitor::services::ActionReply as Convert>::from_proto(*v)?)) })
    }
}
