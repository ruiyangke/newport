use super::*;
impl Convert for crate::desktop::connection::discovery::Discovery {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::connection::Discovery;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        use crate::monitor::ports::discovery::wire::Record;
        self.validate_encoding()?;
        Ok(Self::Proto {
            current: Some(self.current.to_wire()),
            retained: self.retained.iter().map(Record::to_wire).collect(),
            retention: Some(self.retention.to_wire()),
        })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        use crate::monitor::ports::discovery::{Report, Snapshot, wire::Record};
        let result = Self {
            current: Snapshot::from_wire(required(value.current)?)?,
            retained: value
                .retained
                .into_iter()
                .map(Snapshot::from_wire)
                .collect::<Result<_, _>>()?,
            retention: Report::from_wire(required(value.retention)?)?,
        };
        result.validate_encoding()?;
        Ok(result)
    }
}
impl Convert for crate::desktop::connection::SetupInspection {
    type Proto =
        crate::encoding::protobuf::generated::newport::desktop::connection::SetupInspection;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::connection::SetupInspection {
                os: Some(self.os.clone()),
                architecture: Some(self.architecture.clone()),
                cores: self
                    .cores
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(*v) })
                    .transpose()?,
                memory_kib: self
                    .memory_kib
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(*v) })
                    .transpose()?,
                free_kib: self
                    .free_kib
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(*v) })
                    .transpose()?,
                installed: Some(self.installed),
                total: Some(self.total),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            os: required(value.os)?,
            architecture: required(value.architecture)?,
            cores: value
                .cores
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            memory_kib: value
                .memory_kib
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            free_kib: value
                .free_kib
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            installed: required(value.installed)?,
            total: required(value.total)?,
        })
    }
}

impl Convert for crate::desktop::connection::OptionalNotice {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::connection::OptionalNotice;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::connection::OptionalNotice {
                notice: self
                    .notice
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            notice: value
                .notice
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
        })
    }
}

impl Convert for crate::desktop::connection::AgentKey {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::connection::AgentKey;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::connection::AgentKey {
                source: Some(self.source.clone()),
                comment: Some(self.comment.clone()),
                fingerprint: Some(self.fingerprint.clone()),
                algorithm: Some(self.algorithm.clone()),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            source: required(value.source)?,
            comment: required(value.comment)?,
            fingerprint: required(value.fingerprint)?,
            algorithm: required(value.algorithm)?,
        })
    }
}

impl Convert for crate::desktop::connection::AgentKeyList {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::connection::AgentKeyList;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::connection::AgentKeyList {
                keys: self
                    .keys
                    .iter()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .collect::<Result<_, _>>()?,
                warnings: self
                    .warnings
                    .iter()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .collect::<Result<_, _>>()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            keys: value
                .keys
                .into_iter()
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::connection::AgentKey as Convert>::from_proto(v)
                })
                .collect::<Result<_, _>>()?,
            warnings: value
                .warnings
                .into_iter()
                .map(|v| -> Result<_, Error> { Ok(v) })
                .collect::<Result<_, _>>()?,
        })
    }
}

impl Convert for crate::desktop::connection::ConfigHost {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::connection::ConfigHost;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::connection::ConfigHost {
                id: Some(self.id.to_string()),
                alias: Some(self.alias.clone()),
                host: Some(self.host.clone()),
                user: Some(self.user.clone()),
                port: Some(u32::from(self.port)),
                identity_file: self
                    .identity_file
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
                proxy: self
                    .proxy
                    .as_ref()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .transpose()?,
                through: self
                    .through
                    .iter()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .collect::<Result<_, _>>()?,
                importable: Some(self.importable),
                suggested: Some(self.suggested),
                note: self
                    .note
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            id: uuid(required(value.id)?)?,
            alias: required(value.alias)?,
            host: required(value.host)?,
            user: required(value.user)?,
            port: u16::try_from(required(value.port)?).map_err(|_| Invalid("u16 range"))?,
            identity_file: value
                .identity_file
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            proxy: value
                .proxy
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::configuration::Proxy as Convert>::from_proto(v)
                })
                .transpose()?,
            through: value
                .through
                .into_iter()
                .map(|v| -> Result<_, Error> { Ok(v) })
                .collect::<Result<_, _>>()?,
            importable: required(value.importable)?,
            suggested: required(value.suggested)?,
            note: value
                .note
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
        })
    }
}

impl Convert for crate::desktop::connection::SshResolveArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::connection::SshResolveArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::connection::SshResolveArgs {
                host: Some(self.host.clone()),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            host: required(value.host)?,
        })
    }
}

impl Convert for crate::desktop::connection::SshResolved {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::connection::SshResolved;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::connection::SshResolved {
                hostname: self.hostname.clone(),
                user: self.user.clone(),
                port: self.port.map(u32::from),
                identity_file: self.identity_file.clone(),
                proxy_command: self.proxy_command.clone(),
                proxy_jump: self.proxy_jump.clone(),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            hostname: value.hostname,
            user: value.user,
            port: value
                .port
                .map(|port| u16::try_from(port).map_err(|_| Invalid("u16 range")))
                .transpose()?,
            identity_file: value.identity_file,
            proxy_command: value.proxy_command,
            proxy_jump: value.proxy_jump,
        })
    }
}

impl Convert for crate::desktop::connection::ConfigHosts {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::connection::ConfigHosts;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::connection::ConfigHosts {
                path: Some(self.path.clone()),
                hosts: self
                    .hosts
                    .iter()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .collect::<Result<_, _>>()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            path: required(value.path)?,
            hosts: value
                .hosts
                .into_iter()
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::connection::ConfigHost as Convert>::from_proto(v)
                })
                .collect::<Result<_, _>>()?,
        })
    }
}

impl Convert for crate::desktop::connection::PortsObservation {
    type Proto =
        crate::encoding::protobuf::generated::newport::desktop::connection::PortsObservation;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::connection::PortsObservation {
                discovery: self
                    .data
                    .as_ref()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .transpose()?,
                error: self
                    .error
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
                fetching: Some(self.fetching),
                stale: Some(self.stale),
                revision: Some(self.revision),
                updated_at: self
                    .updated_at
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(*v) })
                    .transpose()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            data: value
                .discovery
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::connection::discovery::Discovery as Convert>::from_proto(v)
                })
                .transpose()?,
            error: value
                .error
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            fetching: required(value.fetching)?,
            stale: required(value.stale)?,
            revision: required(value.revision)?,
            updated_at: value
                .updated_at
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
        })
    }
}

impl Convert for crate::desktop::connection::UninstallOutput {
    type Proto =
        crate::encoding::protobuf::generated::newport::desktop::connection::UninstallOutput;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::connection::UninstallOutput {
                text: Some(self.text.clone()),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            text: required(value.text)?,
        })
    }
}

impl Convert for crate::desktop::connection::SetupInspectArgs {
    type Proto =
        crate::encoding::protobuf::generated::newport::desktop::connection::SetupInspectArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::connection::SetupInspectArgs {
                expected_server: Some(self.expected_server.to_proto()?),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            expected_server: <crate::desktop::configuration::Server as Convert>::from_proto(
                required(value.expected_server)?,
            )?,
        })
    }
}

impl Convert for crate::desktop::connection::SetupInstallArgs {
    type Proto =
        crate::encoding::protobuf::generated::newport::desktop::connection::SetupInstallArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::connection::SetupInstallArgs {
                expected_server: Some(self.expected_server.to_proto()?),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            expected_server: <crate::desktop::configuration::Server as Convert>::from_proto(
                required(value.expected_server)?,
            )?,
        })
    }
}

impl Convert for crate::desktop::connection::SetupFinishArgs {
    type Proto =
        crate::encoding::protobuf::generated::newport::desktop::connection::SetupFinishArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::connection::SetupFinishArgs {
                expected_server: Some(self.expected_server.to_proto()?),
                browser: Some(self.browser),
                clipboard: Some(self.clipboard),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            expected_server: <crate::desktop::configuration::Server as Convert>::from_proto(
                required(value.expected_server)?,
            )?,
            browser: required(value.browser)?,
            clipboard: required(value.clipboard)?,
        })
    }
}

impl Convert for crate::desktop::connection::SetupSkipArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::connection::SetupSkipArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::connection::SetupSkipArgs {
                expected_server: Some(self.expected_server.to_proto()?),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            expected_server: <crate::desktop::configuration::Server as Convert>::from_proto(
                required(value.expected_server)?,
            )?,
        })
    }
}

impl Convert for crate::desktop::connection::TestConnectionArgs {
    type Proto =
        crate::encoding::protobuf::generated::newport::desktop::connection::TestConnectionArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::connection::TestConnectionArgs { server: Some(self.server.to_string()) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
        })
    }
}

impl Convert for crate::desktop::connection::ConnectionsQueryArgs {
    type Proto =
        crate::encoding::protobuf::generated::newport::desktop::connection::ConnectionsQueryArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::connection::ConnectionsQueryArgs { server: Some(self.server.to_string()), refresh: Some(self.refresh) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            refresh: value.refresh.unwrap_or(false),
        })
    }
}

impl Convert for crate::desktop::connection::ReinstallAgentArgs {
    type Proto =
        crate::encoding::protobuf::generated::newport::desktop::connection::ReinstallAgentArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::connection::ReinstallAgentArgs { server: Some(self.server.to_string()) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
        })
    }
}

impl Convert for crate::desktop::connection::UninstallAgentArgs {
    type Proto =
        crate::encoding::protobuf::generated::newport::desktop::connection::UninstallAgentArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::connection::UninstallAgentArgs { expected_server: Some(self.expected_server.to_proto()?) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            expected_server: <crate::desktop::configuration::Server as Convert>::from_proto(
                required(value.expected_server)?,
            )?,
        })
    }
}

impl Convert for crate::desktop::connection::InspectAgentArgs {
    type Proto =
        crate::encoding::protobuf::generated::newport::desktop::connection::InspectAgentArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::connection::InspectAgentArgs {
                expected_server: Some(self.expected_server.to_proto()?),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            expected_server: <crate::desktop::configuration::Server as Convert>::from_proto(
                required(value.expected_server)?,
            )?,
        })
    }
}

impl Convert for crate::desktop::connection::Operation {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::connection::Operation;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::connection::Operation { value: Some(match self { crate::desktop::connection::Operation::SetupInspect(v) => crate::encoding::protobuf::generated::newport::desktop::connection::operation::Value::SetupInspect(Box::new(v.as_ref().to_proto()?)), crate::desktop::connection::Operation::SetupInstall(v) => crate::encoding::protobuf::generated::newport::desktop::connection::operation::Value::SetupInstall(Box::new(v.as_ref().to_proto()?)), crate::desktop::connection::Operation::SetupFinish(v) => crate::encoding::protobuf::generated::newport::desktop::connection::operation::Value::SetupFinish(Box::new(v.as_ref().to_proto()?)), crate::desktop::connection::Operation::SetupSkip(v) => crate::encoding::protobuf::generated::newport::desktop::connection::operation::Value::SetupSkip(Box::new(v.as_ref().to_proto()?)), crate::desktop::connection::Operation::SshAgentKeys(v) => crate::encoding::protobuf::generated::newport::desktop::connection::operation::Value::SshAgentKeys(Box::new(v.as_ref().to_proto()?)), crate::desktop::connection::Operation::SshConfigHosts(v) => crate::encoding::protobuf::generated::newport::desktop::connection::operation::Value::SshConfigHosts(Box::new(v.as_ref().to_proto()?)), crate::desktop::connection::Operation::TestConnection(v) => crate::encoding::protobuf::generated::newport::desktop::connection::operation::Value::TestConnection(Box::new(v.as_ref().to_proto()?)), crate::desktop::connection::Operation::ConnectionsQuery(v) => crate::encoding::protobuf::generated::newport::desktop::connection::operation::Value::ConnectionsQuery(Box::new(v.as_ref().to_proto()?)), crate::desktop::connection::Operation::ReinstallAgent(v) => crate::encoding::protobuf::generated::newport::desktop::connection::operation::Value::ReinstallAgent(Box::new(v.as_ref().to_proto()?)), crate::desktop::connection::Operation::UninstallAgent(v) => crate::encoding::protobuf::generated::newport::desktop::connection::operation::Value::UninstallAgent(Box::new(v.as_ref().to_proto()?)), crate::desktop::connection::Operation::InspectAgent(v) => crate::encoding::protobuf::generated::newport::desktop::connection::operation::Value::InspectAgent(Box::new(v.as_ref().to_proto()?)), crate::desktop::connection::Operation::SshResolve(v) => crate::encoding::protobuf::generated::newport::desktop::connection::operation::Value::SshResolve(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::connection::operation::Value::SetupInspect(v) => crate::desktop::connection::Operation::SetupInspect(Box::new(<crate::desktop::connection::SetupInspectArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::connection::operation::Value::SetupInstall(v) => crate::desktop::connection::Operation::SetupInstall(Box::new(<crate::desktop::connection::SetupInstallArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::connection::operation::Value::SetupFinish(v) => crate::desktop::connection::Operation::SetupFinish(Box::new(<crate::desktop::connection::SetupFinishArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::connection::operation::Value::SetupSkip(v) => crate::desktop::connection::Operation::SetupSkip(Box::new(<crate::desktop::connection::SetupSkipArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::connection::operation::Value::SshAgentKeys(v) => crate::desktop::connection::Operation::SshAgentKeys(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::connection::operation::Value::SshConfigHosts(v) => crate::desktop::connection::Operation::SshConfigHosts(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::connection::operation::Value::TestConnection(v) => crate::desktop::connection::Operation::TestConnection(Box::new(<crate::desktop::connection::TestConnectionArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::connection::operation::Value::ConnectionsQuery(v) => crate::desktop::connection::Operation::ConnectionsQuery(Box::new(<crate::desktop::connection::ConnectionsQueryArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::connection::operation::Value::ReinstallAgent(v) => crate::desktop::connection::Operation::ReinstallAgent(Box::new(<crate::desktop::connection::ReinstallAgentArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::connection::operation::Value::UninstallAgent(v) => crate::desktop::connection::Operation::UninstallAgent(Box::new(<crate::desktop::connection::UninstallAgentArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::connection::operation::Value::InspectAgent(v) => crate::desktop::connection::Operation::InspectAgent(Box::new(<crate::desktop::connection::InspectAgentArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::connection::operation::Value::SshResolve(v) => crate::desktop::connection::Operation::SshResolve(Box::new(<crate::desktop::connection::SshResolveArgs as Convert>::from_proto(*v)?)) })
    }
}

impl Convert for crate::desktop::connection::Reply {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::connection::Reply;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::connection::Reply { value: Some(match self { crate::desktop::connection::Reply::SetupInspect(v) => crate::encoding::protobuf::generated::newport::desktop::connection::reply::Value::SetupInspect(Box::new(v.as_ref().to_proto()?)), crate::desktop::connection::Reply::SetupInstall(v) => crate::encoding::protobuf::generated::newport::desktop::connection::reply::Value::SetupInstall(Box::new(v.as_ref().to_proto()?)), crate::desktop::connection::Reply::SetupFinish(v) => crate::encoding::protobuf::generated::newport::desktop::connection::reply::Value::SetupFinish(Box::new(v.as_ref().to_proto()?)), crate::desktop::connection::Reply::SetupSkip(v) => crate::encoding::protobuf::generated::newport::desktop::connection::reply::Value::SetupSkip(Box::new(v.as_ref().to_proto()?)), crate::desktop::connection::Reply::SshAgentKeys(v) => crate::encoding::protobuf::generated::newport::desktop::connection::reply::Value::SshAgentKeys(Box::new(v.as_ref().to_proto()?)), crate::desktop::connection::Reply::SshConfigHosts(v) => crate::encoding::protobuf::generated::newport::desktop::connection::reply::Value::SshConfigHosts(Box::new(v.as_ref().to_proto()?)), crate::desktop::connection::Reply::TestConnection(v) => crate::encoding::protobuf::generated::newport::desktop::connection::reply::Value::TestConnection(Box::new(v.as_ref().to_proto()?)), crate::desktop::connection::Reply::ConnectionsQuery(v) => crate::encoding::protobuf::generated::newport::desktop::connection::reply::Value::ConnectionsQuery(Box::new(v.as_ref().to_proto()?)), crate::desktop::connection::Reply::ReinstallAgent(v) => crate::encoding::protobuf::generated::newport::desktop::connection::reply::Value::ReinstallAgent(Box::new(v.as_ref().to_proto()?)), crate::desktop::connection::Reply::UninstallAgent(v) => crate::encoding::protobuf::generated::newport::desktop::connection::reply::Value::UninstallAgent(Box::new(v.as_ref().to_proto()?)), crate::desktop::connection::Reply::InspectAgent(v) => crate::encoding::protobuf::generated::newport::desktop::connection::reply::Value::InspectAgent(Box::new(v.as_ref().to_proto()?)), crate::desktop::connection::Reply::SshResolve(v) => crate::encoding::protobuf::generated::newport::desktop::connection::reply::Value::SshResolve(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::connection::reply::Value::SetupInspect(v) => crate::desktop::connection::Reply::SetupInspect(Box::new(<crate::desktop::connection::SetupInspection as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::connection::reply::Value::SetupInstall(v) => crate::desktop::connection::Reply::SetupInstall(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::connection::reply::Value::SetupFinish(v) => crate::desktop::connection::Reply::SetupFinish(Box::new(<crate::desktop::connection::OptionalNotice as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::connection::reply::Value::SetupSkip(v) => crate::desktop::connection::Reply::SetupSkip(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::connection::reply::Value::SshAgentKeys(v) => crate::desktop::connection::Reply::SshAgentKeys(Box::new(<crate::desktop::connection::AgentKeyList as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::connection::reply::Value::SshConfigHosts(v) => crate::desktop::connection::Reply::SshConfigHosts(Box::new(<crate::desktop::connection::ConfigHosts as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::connection::reply::Value::TestConnection(v) => crate::desktop::connection::Reply::TestConnection(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::connection::reply::Value::ConnectionsQuery(v) => crate::desktop::connection::Reply::ConnectionsQuery(Box::new(<crate::desktop::connection::PortsObservation as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::connection::reply::Value::ReinstallAgent(v) => crate::desktop::connection::Reply::ReinstallAgent(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::connection::reply::Value::UninstallAgent(v) => crate::desktop::connection::Reply::UninstallAgent(Box::new(<crate::desktop::connection::UninstallOutput as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::connection::reply::Value::InspectAgent(v) => crate::desktop::connection::Reply::InspectAgent(Box::new(<crate::desktop::configuration::InstallationStatus as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::connection::reply::Value::SshResolve(v) => crate::desktop::connection::Reply::SshResolve(Box::new(<crate::desktop::connection::SshResolved as Convert>::from_proto(*v)?)) })
    }
}
