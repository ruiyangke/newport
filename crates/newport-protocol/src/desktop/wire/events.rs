use super::*;
impl Convert for crate::desktop::events::GitCommand {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::events::GitCommand;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::events::GitCommand {
                server: Some(self.server.to_string()),
                repo_id: self
                    .repo_id
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
                operation_id: self
                    .operation_id
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.to_string()) })
                    .transpose()?,
                entry: Some(self.entry.to_proto()?),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            repo_id: value
                .repo_id
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            operation_id: value
                .operation_id
                .map(|v| -> Result<_, Error> { uuid(v) })
                .transpose()?,
            entry: <crate::git::CommandLog as Convert>::from_proto(required(value.entry)?)?,
        })
    }
}

impl Convert for crate::desktop::events::Event {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::events::Event;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::events::Event { value: Some(match self { crate::desktop::events::Event::GitCommand(v) => crate::encoding::protobuf::generated::newport::desktop::events::event::Value::GitCommand(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::events::event::Value::GitCommand(v) => crate::desktop::events::Event::GitCommand(Box::new(<crate::desktop::events::GitCommand as Convert>::from_proto(*v)?)) })
    }
}

impl Convert for crate::desktop::events::Batch {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::events::Batch;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::events::Batch {
                cursor: Some(self.cursor),
                entries: self
                    .entries
                    .iter()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .collect::<Result<_, _>>()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            cursor: required(value.cursor)?,
            entries: value
                .entries
                .into_iter()
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::events::Event as Convert>::from_proto(v)
                })
                .collect::<Result<_, _>>()?,
        })
    }
}

impl Convert for crate::desktop::events::PollArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::events::PollArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::events::PollArgs {
                after: Some(self.after),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            after: required(value.after)?,
        })
    }
}

impl Convert for crate::desktop::events::Operation {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::events::Operation;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::events::Operation { value: Some(match self { crate::desktop::events::Operation::Poll(v) => crate::encoding::protobuf::generated::newport::desktop::events::operation::Value::Poll(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::events::operation::Value::Poll(v) => crate::desktop::events::Operation::Poll(Box::new(<crate::desktop::events::PollArgs as Convert>::from_proto(*v)?)) })
    }
}

impl Convert for crate::desktop::events::Reply {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::events::Reply;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::events::Reply { value: Some(match self { crate::desktop::events::Reply::Poll(v) => crate::encoding::protobuf::generated::newport::desktop::events::reply::Value::Poll(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? {
            crate::encoding::protobuf::generated::newport::desktop::events::reply::Value::Poll(
                v,
            ) => crate::desktop::events::Reply::Poll(Box::new(
                <crate::desktop::events::Batch as Convert>::from_proto(*v)?,
            )),
        })
    }
}
