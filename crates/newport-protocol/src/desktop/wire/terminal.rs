use super::*;
impl Convert for crate::desktop::terminal::HeldShell {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::terminal::HeldShell;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::terminal::HeldShell {
                terminal: Some(self.terminal.to_string()),
                name: self
                    .name
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
                title: self
                    .title
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
                agent: self.agent.clone(),
                creator: Some(self.creator.to_proto()?),
                attached: self
                    .attached
                    .iter()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .collect::<Result<_, _>>()?,
                started: Some(self.started),
                exited: Some(self.exited),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            terminal: uuid(required(value.terminal)?)?,
            name: value
                .name
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            title: value
                .title
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            agent: value.agent,
            creator: <crate::terminal::Computer as Convert>::from_proto(required(value.creator)?)?,
            attached: value
                .attached
                .into_iter()
                .map(|v| -> Result<_, Error> {
                    <crate::terminal::Attached as Convert>::from_proto(v)
                })
                .collect::<Result<_, _>>()?,
            started: required(value.started)?,
            exited: required(value.exited)?,
        })
    }
}

impl Convert for crate::desktop::terminal::ShellList {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::terminal::ShellList;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::terminal::ShellList {
                shells: self
                    .shells
                    .iter()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .collect::<Result<_, _>>()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            shells: value
                .shells
                .into_iter()
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::terminal::HeldShell as Convert>::from_proto(v)
                })
                .collect::<Result<_, _>>()?,
        })
    }
}

impl Convert for crate::desktop::terminal::HeldShells {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::terminal::HeldShells;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::terminal::HeldShells {
                desktop: Some(self.desktop.clone()),
                holds: Some(self.holds),
                shells: self
                    .shells
                    .as_ref()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .transpose()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            desktop: required(value.desktop)?,
            holds: required(value.holds)?,
            shells: value
                .shells
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::terminal::ShellList as Convert>::from_proto(v)
                })
                .transpose()?,
        })
    }
}

impl Convert for crate::desktop::terminal::Renamed {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::terminal::Renamed;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::terminal::Renamed {
                renamed: Some(self.renamed),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            renamed: required(value.renamed)?,
        })
    }
}

impl Convert for crate::desktop::terminal::Ended {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::terminal::Ended;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::terminal::Ended {
                ended: Some(self.ended),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            ended: required(value.ended)?,
        })
    }
}

impl Convert for crate::desktop::terminal::CloseArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::terminal::CloseArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::terminal::CloseArgs {
                session: Some(self.session.to_string()),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            session: uuid(required(value.session)?)?,
        })
    }
}

impl Convert for crate::desktop::terminal::ListArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::terminal::ListArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::terminal::ListArgs {
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

impl Convert for crate::desktop::terminal::RenameArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::terminal::RenameArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::terminal::RenameArgs {
                server: Some(self.server.to_string()),
                terminal: Some(self.terminal.to_string()),
                name: Some(self.name.clone()),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            terminal: uuid(required(value.terminal)?)?,
            name: required(value.name)?,
        })
    }
}

impl Convert for crate::desktop::terminal::EndArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::terminal::EndArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::terminal::EndArgs {
                server: Some(self.server.to_string()),
                terminal: Some(self.terminal.to_string()),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            terminal: uuid(required(value.terminal)?)?,
        })
    }
}

impl Convert for crate::desktop::terminal::Operation {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::terminal::Operation;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::terminal::Operation { value: Some(match self { crate::desktop::terminal::Operation::Close(v) => crate::encoding::protobuf::generated::newport::desktop::terminal::operation::Value::Close(Box::new(v.as_ref().to_proto()?)), crate::desktop::terminal::Operation::List(v) => crate::encoding::protobuf::generated::newport::desktop::terminal::operation::Value::List(Box::new(v.as_ref().to_proto()?)), crate::desktop::terminal::Operation::Rename(v) => crate::encoding::protobuf::generated::newport::desktop::terminal::operation::Value::Rename(Box::new(v.as_ref().to_proto()?)), crate::desktop::terminal::Operation::End(v) => crate::encoding::protobuf::generated::newport::desktop::terminal::operation::Value::End(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::terminal::operation::Value::Close(v) => crate::desktop::terminal::Operation::Close(Box::new(<crate::desktop::terminal::CloseArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::terminal::operation::Value::List(v) => crate::desktop::terminal::Operation::List(Box::new(<crate::desktop::terminal::ListArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::terminal::operation::Value::Rename(v) => crate::desktop::terminal::Operation::Rename(Box::new(<crate::desktop::terminal::RenameArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::terminal::operation::Value::End(v) => crate::desktop::terminal::Operation::End(Box::new(<crate::desktop::terminal::EndArgs as Convert>::from_proto(*v)?)) })
    }
}

impl Convert for crate::desktop::terminal::Reply {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::terminal::Reply;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::terminal::Reply { value: Some(match self { crate::desktop::terminal::Reply::Close(v) => crate::encoding::protobuf::generated::newport::desktop::terminal::reply::Value::Close(Box::new(v.as_ref().to_proto()?)), crate::desktop::terminal::Reply::List(v) => crate::encoding::protobuf::generated::newport::desktop::terminal::reply::Value::List(Box::new(v.as_ref().to_proto()?)), crate::desktop::terminal::Reply::Rename(v) => crate::encoding::protobuf::generated::newport::desktop::terminal::reply::Value::Rename(Box::new(v.as_ref().to_proto()?)), crate::desktop::terminal::Reply::End(v) => crate::encoding::protobuf::generated::newport::desktop::terminal::reply::Value::End(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::terminal::reply::Value::Close(v) => crate::desktop::terminal::Reply::Close(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::terminal::reply::Value::List(v) => crate::desktop::terminal::Reply::List(Box::new(<crate::desktop::terminal::HeldShells as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::terminal::reply::Value::Rename(v) => crate::desktop::terminal::Reply::Rename(Box::new(<crate::desktop::terminal::Renamed as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::terminal::reply::Value::End(v) => crate::desktop::terminal::Reply::End(Box::new(<crate::desktop::terminal::Ended as Convert>::from_proto(*v)?)) })
    }
}
