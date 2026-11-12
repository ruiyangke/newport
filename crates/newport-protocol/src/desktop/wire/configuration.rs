use super::*;
impl Convert for crate::desktop::configuration::AuthMethod {
    type Proto = i32;
    fn to_proto(&self) -> Result<i32, Error> {
        Ok(match self {
            Self::PublicKey => 1,
            Self::Password => 2,
        })
    }
    fn from_proto(value: i32) -> Result<Self, Error> {
        Ok(match value {
            1 => Self::PublicKey,
            2 => Self::Password,
            _ => return Err(Invalid("AuthMethod").into()),
        })
    }
}

impl Convert for crate::desktop::configuration::Direction {
    type Proto = i32;
    fn to_proto(&self) -> Result<i32, Error> {
        Ok(match self {
            Self::Local => 1,
            Self::Remote => 2,
        })
    }
    fn from_proto(value: i32) -> Result<Self, Error> {
        Ok(match value {
            1 => Self::Local,
            2 => Self::Remote,
            _ => return Err(Invalid("Direction").into()),
        })
    }
}

impl Convert for crate::desktop::configuration::Feature {
    type Proto = i32;
    fn to_proto(&self) -> Result<i32, Error> {
        Ok(match self {
            Self::Clipboard => 1,
            Self::Browser => 2,
        })
    }
    fn from_proto(value: i32) -> Result<Self, Error> {
        Ok(match value {
            1 => Self::Clipboard,
            2 => Self::Browser,
            _ => return Err(Invalid("Feature").into()),
        })
    }
}

impl Convert for crate::desktop::configuration::Proxy {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::configuration::Proxy;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::configuration::Proxy { value: Some(match self { crate::desktop::configuration::Proxy::Command(v) => crate::encoding::protobuf::generated::newport::desktop::configuration::proxy::Value::Command(v.as_ref().clone()), crate::desktop::configuration::Proxy::Jump(v) => crate::encoding::protobuf::generated::newport::desktop::configuration::proxy::Value::Jump(v.as_ref().to_string()) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::configuration::proxy::Value::Command(v) => crate::desktop::configuration::Proxy::Command(Box::new(v)), crate::encoding::protobuf::generated::newport::desktop::configuration::proxy::Value::Jump(v) => crate::desktop::configuration::Proxy::Jump(Box::new(uuid(v)?)) })
    }
}

impl Convert for crate::desktop::configuration::Server {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::configuration::Server;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::configuration::Server {
                id: Some(self.id.to_string()),
                name: Some(self.name.clone()),
                ssh_user: Some(self.ssh_user.clone()),
                ssh_host: Some(self.ssh_host.clone()),
                ssh_port: Some(u32::from(self.ssh_port)),
                identity_file: self
                    .identity_file
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
                agent_source: self
                    .agent_source
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
                agent_key_fingerprint: self
                    .agent_key_fingerprint
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
                auth_method: Some(self.auth_method.to_proto()?),
                clipboard_enabled: Some(self.clipboard_enabled),
                browser_enabled: Some(self.browser_enabled),
                agent_install_approved: Some(self.agent_install_approved),
                setup_complete: Some(self.setup_complete),
                proxy: self
                    .proxy
                    .as_ref()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .transpose()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            id: uuid(required(value.id)?)?,
            name: required(value.name)?,
            ssh_user: required(value.ssh_user)?,
            ssh_host: required(value.ssh_host)?,
            ssh_port: u16::try_from(required(value.ssh_port)?).map_err(|_| Invalid("u16 range"))?,
            identity_file: value
                .identity_file
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            agent_source: value
                .agent_source
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            agent_key_fingerprint: value
                .agent_key_fingerprint
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            auth_method: <crate::desktop::configuration::AuthMethod as Convert>::from_proto(
                required(value.auth_method)?,
            )?,
            clipboard_enabled: required(value.clipboard_enabled)?,
            browser_enabled: required(value.browser_enabled)?,
            agent_install_approved: required(value.agent_install_approved)?,
            setup_complete: required(value.setup_complete)?,
            proxy: value
                .proxy
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::configuration::Proxy as Convert>::from_proto(v)
                })
                .transpose()?,
        })
    }
}

impl Convert for crate::desktop::configuration::Tunnel {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::configuration::Tunnel;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::configuration::Tunnel {
                id: Some(self.id.to_string()),
                name: Some(self.name.clone()),
                server_id: Some(self.server_id.to_string()),
                local_port: Some(u32::from(self.local_port)),
                local_port_end: self
                    .local_port_end
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(u32::from(*v)) })
                    .transpose()?,
                remote_host: Some(self.remote_host.clone()),
                remote_port: Some(u32::from(self.remote_port)),
                remote_port_end: self
                    .remote_port_end
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(u32::from(*v)) })
                    .transpose()?,
                auto_connect: Some(self.auto_connect),
                auto_reconnect: Some(self.auto_reconnect),
                direction: Some(self.direction.to_proto()?),
                project: self
                    .project
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            id: uuid(required(value.id)?)?,
            name: required(value.name)?,
            server_id: uuid(required(value.server_id)?)?,
            local_port: u16::try_from(required(value.local_port)?)
                .map_err(|_| Invalid("u16 range"))?,
            local_port_end: value
                .local_port_end
                .map(|v| -> Result<_, Error> {
                    Ok(u16::try_from(v).map_err(|_| Invalid("u16 range"))?)
                })
                .transpose()?,
            remote_host: required(value.remote_host)?,
            remote_port: u16::try_from(required(value.remote_port)?)
                .map_err(|_| Invalid("u16 range"))?,
            remote_port_end: value
                .remote_port_end
                .map(|v| -> Result<_, Error> {
                    Ok(u16::try_from(v).map_err(|_| Invalid("u16 range"))?)
                })
                .transpose()?,
            auto_connect: required(value.auto_connect)?,
            auto_reconnect: required(value.auto_reconnect)?,
            direction: <crate::desktop::configuration::Direction as Convert>::from_proto(
                required(value.direction)?,
            )?,
            project: value
                .project
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
        })
    }
}

impl Convert for crate::desktop::configuration::Config {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::configuration::Config;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::configuration::Config {
                servers: self
                    .servers
                    .iter()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .collect::<Result<_, _>>()?,
                tunnels: self
                    .tunnels
                    .iter()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .collect::<Result<_, _>>()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            servers: value
                .servers
                .into_iter()
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::configuration::Server as Convert>::from_proto(v)
                })
                .collect::<Result<_, _>>()?,
            tunnels: value
                .tunnels
                .into_iter()
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::configuration::Tunnel as Convert>::from_proto(v)
                })
                .collect::<Result<_, _>>()?,
        })
    }
}

impl Convert for crate::desktop::configuration::ConnectionSettings {
    type Proto =
        crate::encoding::protobuf::generated::newport::desktop::configuration::ConnectionSettings;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::configuration::ConnectionSettings { server_id: Some(self.server_id.to_string()), ssh_host: Some(self.ssh_host.clone()), ssh_user: Some(self.ssh_user.clone()), ssh_port: Some(u32::from(self.ssh_port)), auth_method: Some(self.auth_method.to_proto()?), identity_file: self.identity_file.as_ref().map(|v| -> Result<_, Error> { Ok(v.clone()) }).transpose()?, agent_source: self.agent_source.as_ref().map(|v| -> Result<_, Error> { Ok(v.clone()) }).transpose()?, agent_key_fingerprint: self.agent_key_fingerprint.as_ref().map(|v| -> Result<_, Error> { Ok(v.clone()) }).transpose()?, proxy: self.proxy.as_ref().map(|v| -> Result<_, Error> { v.to_proto() }).transpose()? })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server_id: uuid(required(value.server_id)?)?,
            ssh_host: required(value.ssh_host)?,
            ssh_user: required(value.ssh_user)?,
            ssh_port: u16::try_from(required(value.ssh_port)?).map_err(|_| Invalid("u16 range"))?,
            auth_method: <crate::desktop::configuration::AuthMethod as Convert>::from_proto(
                required(value.auth_method)?,
            )?,
            identity_file: value
                .identity_file
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            agent_source: value
                .agent_source
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            agent_key_fingerprint: value
                .agent_key_fingerprint
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            proxy: value
                .proxy
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::configuration::Proxy as Convert>::from_proto(v)
                })
                .transpose()?,
        })
    }
}

impl Convert for crate::desktop::configuration::ConnectionScope {
    type Proto =
        crate::encoding::protobuf::generated::newport::desktop::configuration::ConnectionScope;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::configuration::ConnectionScope { settings: Some(self.settings.to_proto()?), connection_revision: Some(self.connection_revision), backend_instance: Some(self.backend_instance.to_string()) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            settings: <crate::desktop::configuration::ConnectionSettings as Convert>::from_proto(
                required(value.settings)?,
            )?,
            connection_revision: required(value.connection_revision)?,
            backend_instance: uuid(required(value.backend_instance)?)?,
        })
    }
}

impl Convert for crate::desktop::configuration::TransportStatus {
    type Proto = i32;
    fn to_proto(&self) -> Result<i32, Error> {
        Ok(match self {
            Self::Disconnected => 1,
            Self::Connecting => 2,
            Self::Connected => 3,
            Self::Recovering => 4,
            Self::Failed => 5,
        })
    }
    fn from_proto(value: i32) -> Result<Self, Error> {
        Ok(match value {
            1 => Self::Disconnected,
            2 => Self::Connecting,
            3 => Self::Connected,
            4 => Self::Recovering,
            5 => Self::Failed,
            _ => return Err(Invalid("TransportStatus").into()),
        })
    }
}

impl Convert for crate::desktop::configuration::Transport {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::configuration::Transport;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::configuration::Transport {
                status: Some(self.status.to_proto()?),
                generation: Some(self.generation),
                waiting: Some(self.waiting),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            status: <crate::desktop::configuration::TransportStatus as Convert>::from_proto(
                required(value.status)?,
            )?,
            generation: required(value.generation)?,
            waiting: required(value.waiting)?,
        })
    }
}

impl Convert for crate::desktop::configuration::RuntimeStatus {
    type Proto = i32;
    fn to_proto(&self) -> Result<i32, Error> {
        Ok(match self {
            Self::Unknown => 1,
            Self::Starting => 2,
            Self::Ready => 3,
            Self::Failed => 4,
        })
    }
    fn from_proto(value: i32) -> Result<Self, Error> {
        Ok(match value {
            1 => Self::Unknown,
            2 => Self::Starting,
            3 => Self::Ready,
            4 => Self::Failed,
            _ => return Err(Invalid("RuntimeStatus").into()),
        })
    }
}

impl Convert for crate::desktop::configuration::RuntimeInstance {
    type Proto =
        crate::encoding::protobuf::generated::newport::desktop::configuration::RuntimeInstance;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::configuration::RuntimeInstance { instance_id: Some(self.instance_id.to_string()), build_sha256: Some(self.build_sha256.clone()), generation: Some(self.generation), retiring: Some(self.retiring), draining: Some(self.draining), connected: Some(self.connected), registrations: Some(self.registrations), worker_leases: Some(self.worker_leases) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            instance_id: uuid(required(value.instance_id)?)?,
            build_sha256: required(value.build_sha256)?,
            generation: required(value.generation)?,
            retiring: required(value.retiring)?,
            draining: required(value.draining)?,
            connected: required(value.connected)?,
            registrations: required(value.registrations)?,
            worker_leases: required(value.worker_leases)?,
        })
    }
}

impl Convert for crate::desktop::configuration::RuntimeSnapshot {
    type Proto =
        crate::encoding::protobuf::generated::newport::desktop::configuration::RuntimeSnapshot;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::configuration::RuntimeSnapshot { status: Some(self.status.to_proto()?), generation: Some(self.generation), hello: self.hello.as_ref().map(|v| -> Result<_, Error> { v.to_proto() }).transpose()?, error: self.error.as_ref().map(|v| -> Result<_, Error> { Ok(v.clone()) }).transpose()?, retryable: Some(self.retryable), failures: Some(self.failures), instances: self.instances.iter().map(|v| -> Result<_, Error> { v.to_proto() }).collect::<Result<_, _>>()? })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            status: <crate::desktop::configuration::RuntimeStatus as Convert>::from_proto(
                required(value.status)?,
            )?,
            generation: required(value.generation)?,
            hello: value
                .hello
                .map(|v| -> Result<_, Error> { <crate::runtime::Hello as Convert>::from_proto(v) })
                .transpose()?,
            error: value
                .error
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            retryable: required(value.retryable)?,
            failures: required(value.failures)?,
            instances: value
                .instances
                .into_iter()
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::configuration::RuntimeInstance as Convert>::from_proto(v)
                })
                .collect::<Result<_, _>>()?,
        })
    }
}

impl Convert for crate::desktop::configuration::PermissionsSnapshot {
    type Proto =
        crate::encoding::protobuf::generated::newport::desktop::configuration::PermissionsSnapshot;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        self.validate()?;
        Ok(Self::Proto {
            desired: Some(self.desired.to_proto()?),
            effective: Some(crate::runtime::wire::convert::permission_values(
                &self.effective,
            )),
            error: self.error.clone(),
            clipboard_epoch: Some(self.clipboard_epoch),
            browser_epoch: Some(self.browser_epoch),
            session_epoch: Some(self.session_epoch),
        })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        let value = Self {
            desired: <crate::runtime::Permissions as Convert>::from_proto(required(
                value.desired,
            )?)?,
            effective: crate::runtime::wire::convert::from_permission_values(value.effective)?,
            error: value
                .error
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            clipboard_epoch: required(value.clipboard_epoch)?,
            browser_epoch: required(value.browser_epoch)?,
            session_epoch: required(value.session_epoch)?,
        };
        value.validate()?;
        Ok(value)
    }
}

impl Convert for crate::desktop::configuration::BuildRelation {
    type Proto = i32;
    fn to_proto(&self) -> Result<i32, Error> {
        Ok(match self {
            Self::Unknown => 1,
            Self::Missing => 2,
            Self::Bundled => 3,
            Self::Different => 4,
            Self::Older => 5,
            Self::Newer => 6,
            Self::SameRelease => 7,
        })
    }
    fn from_proto(value: i32) -> Result<Self, Error> {
        Ok(match value {
            1 => Self::Unknown,
            2 => Self::Missing,
            3 => Self::Bundled,
            4 => Self::Different,
            5 => Self::Older,
            6 => Self::Newer,
            7 => Self::SameRelease,
            _ => return Err(Invalid("BuildRelation").into()),
        })
    }
}

impl Convert for crate::desktop::configuration::UpdateOutcome {
    type Proto =
        crate::encoding::protobuf::generated::newport::desktop::configuration::UpdateOutcome;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::configuration::UpdateOutcome {
                at: Some(self.at),
                from: self
                    .from
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
                to: self
                    .to
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
                error: self
                    .error
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            at: required(value.at)?,
            from: value
                .from
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            to: value
                .to
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            error: value
                .error
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
        })
    }
}

impl Convert for crate::desktop::configuration::InstallationStatus {
    type Proto =
        crate::encoding::protobuf::generated::newport::desktop::configuration::InstallationStatus;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::configuration::InstallationStatus { installed: self.installed.as_ref().map(|v| -> Result<_, Error> { Ok(*v) }).transpose()?, update_available: self.update_available.as_ref().map(|v| -> Result<_, Error> { Ok(*v) }).transpose()?, build_relation: Some(self.build_relation.to_proto()?), installed_hash: self.installed_hash.as_ref().map(|v| -> Result<_, Error> { Ok(v.clone()) }).transpose()?, bundled_hash: self.bundled_hash.as_ref().map(|v| -> Result<_, Error> { Ok(v.clone()) }).transpose()?, installed_version: self.installed_version.as_ref().map(|v| -> Result<_, Error> { Ok(v.clone()) }).transpose()?, bundled_version: self.bundled_version.as_ref().map(|v| -> Result<_, Error> { Ok(v.clone()) }).transpose()?, installed_variant: self.installed_variant.as_ref().map(|v| -> Result<_, Error> { v.to_proto() }).transpose()?, bundled_variant: Some(self.bundled_variant.to_proto()?), installed_running: self.installed_running.as_ref().map(|v| -> Result<_, Error> { Ok(*v) }).transpose()?, bundled_staged: Some(self.bundled_staged), checked_at: Some(self.checked_at), error: self.error.as_ref().map(|v| -> Result<_, Error> { Ok(v.clone()) }).transpose()?, automatic_update: self.automatic_update.as_ref().map(|v| -> Result<_, Error> { v.to_proto() }).transpose()? })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            installed: value
                .installed
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            update_available: value
                .update_available
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            build_relation: <crate::desktop::configuration::BuildRelation as Convert>::from_proto(
                required(value.build_relation)?,
            )?,
            installed_hash: value
                .installed_hash
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            bundled_hash: value
                .bundled_hash
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            installed_version: value
                .installed_version
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            bundled_version: value
                .bundled_version
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            installed_variant: value
                .installed_variant
                .map(|v| -> Result<_, Error> {
                    <crate::product::Variant as Convert>::from_proto(v)
                })
                .transpose()?,
            bundled_variant: <crate::product::Variant as Convert>::from_proto(required(
                value.bundled_variant,
            )?)?,
            installed_running: value
                .installed_running
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            bundled_staged: required(value.bundled_staged)?,
            checked_at: required(value.checked_at)?,
            error: value
                .error
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            automatic_update: value
                .automatic_update
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::configuration::UpdateOutcome as Convert>::from_proto(v)
                })
                .transpose()?,
        })
    }
}

impl Convert for crate::desktop::configuration::InstallStage {
    type Proto = i32;
    fn to_proto(&self) -> Result<i32, Error> {
        Ok(match self {
            Self::Preparing => 1,
            Self::Uploading => 2,
            Self::Installing => 3,
            Self::Verifying => 4,
            Self::Activating => 5,
            Self::Ready => 6,
            Self::Error => 7,
        })
    }
    fn from_proto(value: i32) -> Result<Self, Error> {
        Ok(match value {
            1 => Self::Preparing,
            2 => Self::Uploading,
            3 => Self::Installing,
            4 => Self::Verifying,
            5 => Self::Activating,
            6 => Self::Ready,
            7 => Self::Error,
            _ => return Err(Invalid("InstallStage").into()),
        })
    }
}

impl Convert for crate::desktop::configuration::InstallProgress {
    type Proto =
        crate::encoding::protobuf::generated::newport::desktop::configuration::InstallProgress;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::configuration::InstallProgress { stage: Some(self.stage.to_proto()?), sent: Some(self.sent), total: Some(self.total), error: self.error.as_ref().map(|v| -> Result<_, Error> { Ok(v.clone()) }).transpose()?, failed_stage: self.failed_stage.as_ref().map(|v| -> Result<_, Error> { v.to_proto() }).transpose()?, bytes_per_second: self.bytes_per_second.as_ref().map(|v| -> Result<_, Error> { Ok(*v) }).transpose()? })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            stage: <crate::desktop::configuration::InstallStage as Convert>::from_proto(required(
                value.stage,
            )?)?,
            sent: required(value.sent)?,
            total: required(value.total)?,
            error: value
                .error
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            failed_stage: value
                .failed_stage
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::configuration::InstallStage as Convert>::from_proto(v)
                })
                .transpose()?,
            bytes_per_second: value
                .bytes_per_second
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
        })
    }
}

impl Convert for crate::desktop::configuration::ConnectionStatus {
    type Proto = i32;
    fn to_proto(&self) -> Result<i32, Error> {
        Ok(match self {
            Self::Disconnected => 1,
            Self::Connecting => 2,
            Self::Connected => 3,
            Self::Reconnecting => 4,
            Self::Error => 5,
        })
    }
    fn from_proto(value: i32) -> Result<Self, Error> {
        Ok(match value {
            1 => Self::Disconnected,
            2 => Self::Connecting,
            3 => Self::Connected,
            4 => Self::Reconnecting,
            5 => Self::Error,
            _ => return Err(Invalid("ConnectionStatus").into()),
        })
    }
}

impl Convert for crate::desktop::configuration::ConnectionState {
    type Proto =
        crate::encoding::protobuf::generated::newport::desktop::configuration::ConnectionState;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::configuration::ConnectionState { status: Some(self.status.to_proto()?), error_message: self.error_message.as_ref().map(|v| -> Result<_, Error> { Ok(v.clone()) }).transpose()?, reconnect_attempt: Some(self.reconnect_attempt) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            status: <crate::desktop::configuration::ConnectionStatus as Convert>::from_proto(
                required(value.status)?,
            )?,
            error_message: value
                .error_message
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            reconnect_attempt: required(value.reconnect_attempt)?,
        })
    }
}

impl Convert for crate::desktop::configuration::DestinationStatus {
    type Proto = i32;
    fn to_proto(&self) -> Result<i32, Error> {
        Ok(match self {
            Self::Checking => 1,
            Self::Reachable => 2,
            Self::Unavailable => 3,
            Self::Blocked => 4,
            Self::Unknown => 5,
        })
    }
    fn from_proto(value: i32) -> Result<Self, Error> {
        Ok(match value {
            1 => Self::Checking,
            2 => Self::Reachable,
            3 => Self::Unavailable,
            4 => Self::Blocked,
            5 => Self::Unknown,
            _ => return Err(Invalid("DestinationStatus").into()),
        })
    }
}

impl Convert for crate::desktop::configuration::DestinationHealth {
    type Proto =
        crate::encoding::protobuf::generated::newport::desktop::configuration::DestinationHealth;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::configuration::DestinationHealth { local_port: Some(u32::from(self.local_port)), remote_port: Some(u32::from(self.remote_port)), status: Some(self.status.to_proto()?), message: self.message.as_ref().map(|v| -> Result<_, Error> { Ok(v.clone()) }).transpose()?, checked_at: self.checked_at.as_ref().map(|v| -> Result<_, Error> { Ok(*v) }).transpose()? })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            local_port: u16::try_from(required(value.local_port)?)
                .map_err(|_| Invalid("u16 range"))?,
            remote_port: u16::try_from(required(value.remote_port)?)
                .map_err(|_| Invalid("u16 range"))?,
            status: <crate::desktop::configuration::DestinationStatus as Convert>::from_proto(
                required(value.status)?,
            )?,
            message: value
                .message
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            checked_at: value
                .checked_at
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
        })
    }
}

impl Convert for crate::desktop::configuration::DestinationHealthList {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::configuration::DestinationHealthList;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::configuration::DestinationHealthList { values: self.values.iter().map(|v| -> Result<_, Error> { v.to_proto() }).collect::<Result<_, _>>()? })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            values: value
                .values
                .into_iter()
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::configuration::DestinationHealth as Convert>::from_proto(v)
                })
                .collect::<Result<_, _>>()?,
        })
    }
}

impl Convert for crate::desktop::configuration::TransportRow {
    type Proto =
        crate::encoding::protobuf::generated::newport::desktop::configuration::TransportRow;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::configuration::TransportRow {
                id: Some(self.id.to_string()),
                value: Some(self.value.to_proto()?),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            id: uuid(required(value.id)?)?,
            value: <crate::desktop::configuration::Transport as Convert>::from_proto(required(
                value.value,
            )?)?,
        })
    }
}

impl Convert for crate::desktop::configuration::RuntimeRow {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::configuration::RuntimeRow;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::configuration::RuntimeRow {
                id: Some(self.id.to_string()),
                value: Some(self.value.to_proto()?),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            id: uuid(required(value.id)?)?,
            value: <crate::desktop::configuration::RuntimeSnapshot as Convert>::from_proto(
                required(value.value)?,
            )?,
        })
    }
}

impl Convert for crate::desktop::configuration::PermissionsRow {
    type Proto =
        crate::encoding::protobuf::generated::newport::desktop::configuration::PermissionsRow;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::configuration::PermissionsRow {
                id: Some(self.id.to_string()),
                value: Some(self.value.to_proto()?),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            id: uuid(required(value.id)?)?,
            value: <crate::desktop::configuration::PermissionsSnapshot as Convert>::from_proto(
                required(value.value)?,
            )?,
        })
    }
}

impl Convert for crate::desktop::configuration::InstallRow {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::configuration::InstallRow;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::configuration::InstallRow {
                id: Some(self.id.to_string()),
                value: Some(self.value.to_proto()?),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            id: uuid(required(value.id)?)?,
            value: <crate::desktop::configuration::InstallProgress as Convert>::from_proto(
                required(value.value)?,
            )?,
        })
    }
}

impl Convert for crate::desktop::configuration::InstallationRow {
    type Proto =
        crate::encoding::protobuf::generated::newport::desktop::configuration::InstallationRow;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::configuration::InstallationRow { id: Some(self.id.to_string()), value: Some(self.value.to_proto()?) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            id: uuid(required(value.id)?)?,
            value: <crate::desktop::configuration::InstallationStatus as Convert>::from_proto(
                required(value.value)?,
            )?,
        })
    }
}

impl Convert for crate::desktop::configuration::ConnectionRow {
    type Proto =
        crate::encoding::protobuf::generated::newport::desktop::configuration::ConnectionRow;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::configuration::ConnectionRow {
                id: Some(self.id.to_string()),
                value: Some(self.value.to_proto()?),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            id: uuid(required(value.id)?)?,
            value: <crate::desktop::configuration::ConnectionState as Convert>::from_proto(
                required(value.value)?,
            )?,
        })
    }
}

impl Convert for crate::desktop::configuration::HealthRow {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::configuration::HealthRow;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::configuration::HealthRow {
                id: Some(self.id.to_string()),
                value: Some(self.value.to_proto()?),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            id: uuid(required(value.id)?)?,
            value: <crate::desktop::configuration::DestinationHealthList as Convert>::from_proto(
                required(value.value)?,
            )?,
        })
    }
}

impl Convert for crate::desktop::configuration::MessageRow {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::configuration::MessageRow;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::configuration::MessageRow {
                id: Some(self.id.to_string()),
                value: Some(self.value.clone()),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            id: uuid(required(value.id)?)?,
            value: required(value.value)?,
        })
    }
}

impl Convert for crate::desktop::configuration::RevisionRow {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::configuration::RevisionRow;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::configuration::RevisionRow {
                id: Some(self.id.to_string()),
                value: Some(self.value),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            id: uuid(required(value.id)?)?,
            value: required(value.value)?,
        })
    }
}

impl Convert for crate::desktop::configuration::Runtime {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::configuration::Runtime;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::configuration::Runtime {
                agent_transport: self
                    .agent_transport
                    .iter()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .collect::<Result<_, _>>()?,
                agent_runtime: self
                    .agent_runtime
                    .iter()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .collect::<Result<_, _>>()?,
                agent_permissions: self
                    .agent_permissions
                    .iter()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .collect::<Result<_, _>>()?,
                agent_status: self
                    .agent_status
                    .iter()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .collect::<Result<_, _>>()?,
                agent_installation: self
                    .agent_installation
                    .iter()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .collect::<Result<_, _>>()?,
                tunnels: self
                    .tunnels
                    .iter()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .collect::<Result<_, _>>()?,
                tunnel_health: self
                    .tunnel_health
                    .iter()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .collect::<Result<_, _>>()?,
                clipboard: self
                    .clipboard
                    .iter()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .collect::<Result<_, _>>()?,
                clipboard_messages: self
                    .clipboard_messages
                    .iter()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .collect::<Result<_, _>>()?,
                connection_revisions: self
                    .connection_revisions
                    .iter()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .collect::<Result<_, _>>()?,
                awaiting_agent: self
                    .awaiting_agent
                    .iter()
                    .map(|v| -> Result<_, Error> { Ok(v.to_string()) })
                    .collect::<Result<_, _>>()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            agent_transport: value
                .agent_transport
                .into_iter()
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::configuration::TransportRow as Convert>::from_proto(v)
                })
                .collect::<Result<_, _>>()?,
            agent_runtime: value
                .agent_runtime
                .into_iter()
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::configuration::RuntimeRow as Convert>::from_proto(v)
                })
                .collect::<Result<_, _>>()?,
            agent_permissions: value
                .agent_permissions
                .into_iter()
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::configuration::PermissionsRow as Convert>::from_proto(v)
                })
                .collect::<Result<_, _>>()?,
            agent_status: value
                .agent_status
                .into_iter()
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::configuration::InstallationRow as Convert>::from_proto(v)
                })
                .collect::<Result<_, _>>()?,
            agent_installation: value
                .agent_installation
                .into_iter()
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::configuration::InstallRow as Convert>::from_proto(v)
                })
                .collect::<Result<_, _>>()?,
            tunnels: value
                .tunnels
                .into_iter()
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::configuration::ConnectionRow as Convert>::from_proto(v)
                })
                .collect::<Result<_, _>>()?,
            tunnel_health: value
                .tunnel_health
                .into_iter()
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::configuration::HealthRow as Convert>::from_proto(v)
                })
                .collect::<Result<_, _>>()?,
            clipboard: value
                .clipboard
                .into_iter()
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::configuration::ConnectionRow as Convert>::from_proto(v)
                })
                .collect::<Result<_, _>>()?,
            clipboard_messages: value
                .clipboard_messages
                .into_iter()
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::configuration::MessageRow as Convert>::from_proto(v)
                })
                .collect::<Result<_, _>>()?,
            connection_revisions: value
                .connection_revisions
                .into_iter()
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::configuration::RevisionRow as Convert>::from_proto(v)
                })
                .collect::<Result<_, _>>()?,
            awaiting_agent: value
                .awaiting_agent
                .into_iter()
                .map(|v| -> Result<_, Error> { uuid(v) })
                .collect::<Result<_, _>>()?,
        })
    }
}

impl Convert for crate::desktop::configuration::Snapshot {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::configuration::Snapshot;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::configuration::Snapshot {
                instance_id: Some(self.instance_id.to_string()),
                revision: Some(self.revision),
                config: Some(self.config.to_proto()?),
                runtime: Some(self.runtime.to_proto()?),
                load_error: self
                    .load_error
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            instance_id: uuid(required(value.instance_id)?)?,
            revision: required(value.revision)?,
            config: <crate::desktop::configuration::Config as Convert>::from_proto(required(
                value.config,
            )?)?,
            runtime: <crate::desktop::configuration::Runtime as Convert>::from_proto(required(
                value.runtime,
            )?)?,
            load_error: value
                .load_error
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
        })
    }
}

impl Convert for crate::desktop::configuration::Tunnels {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::configuration::Tunnels;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::configuration::Tunnels {
                tunnels: self
                    .tunnels
                    .iter()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .collect::<Result<_, _>>()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            tunnels: value
                .tunnels
                .into_iter()
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::configuration::Tunnel as Convert>::from_proto(v)
                })
                .collect::<Result<_, _>>()?,
        })
    }
}

impl Convert for crate::desktop::configuration::SaveServerArgs {
    type Proto =
        crate::encoding::protobuf::generated::newport::desktop::configuration::SaveServerArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::configuration::SaveServerArgs {
                server: Some(self.server.to_proto()?),
                password: self
                    .password
                    .as_ref()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .transpose()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: <crate::desktop::configuration::Server as Convert>::from_proto(required(
                value.server,
            )?)?,
            password: value
                .password
                .map(|v| -> Result<_, Error> { <crate::common::Secret as Convert>::from_proto(v) })
                .transpose()?,
        })
    }
}

impl Convert for crate::desktop::configuration::DeleteServerArgs {
    type Proto =
        crate::encoding::protobuf::generated::newport::desktop::configuration::DeleteServerArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::configuration::DeleteServerArgs { server: Some(self.server.to_string()) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
        })
    }
}

impl Convert for crate::desktop::configuration::ForwardDiscoveredPortArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::configuration::ForwardDiscoveredPortArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::configuration::ForwardDiscoveredPortArgs { server: Some(self.server.to_string()), address: Some(self.address.clone()), port: Some(u32::from(self.port)) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            address: required(value.address)?,
            port: u16::try_from(required(value.port)?).map_err(|_| Invalid("u16 range"))?,
        })
    }
}

impl Convert for crate::desktop::configuration::ForwardTunnelBatchArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::configuration::ForwardTunnelBatchArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::configuration::ForwardTunnelBatchArgs { server: Some(self.server.to_string()), create: self.create.iter().map(|v| -> Result<_, Error> { v.to_proto() }).collect::<Result<_, _>>()?, connect: self.connect.iter().map(|v| -> Result<_, Error> { v.to_proto() }).collect::<Result<_, _>>()?, expected: self.expected.as_ref().map(|v| -> Result<_, Error> { v.to_proto() }).transpose()? })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            create: value
                .create
                .into_iter()
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::configuration::Tunnel as Convert>::from_proto(v)
                })
                .collect::<Result<_, _>>()?,
            connect: value
                .connect
                .into_iter()
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::configuration::Tunnel as Convert>::from_proto(v)
                })
                .collect::<Result<_, _>>()?,
            expected: value
                .expected
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::configuration::ConnectionScope as Convert>::from_proto(v)
                })
                .transpose()?,
        })
    }
}

impl Convert for crate::desktop::configuration::SaveTunnelArgs {
    type Proto =
        crate::encoding::protobuf::generated::newport::desktop::configuration::SaveTunnelArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::configuration::SaveTunnelArgs {
                tunnel: Some(self.tunnel.to_proto()?),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            tunnel: <crate::desktop::configuration::Tunnel as Convert>::from_proto(required(
                value.tunnel,
            )?)?,
        })
    }
}

impl Convert for crate::desktop::configuration::UpdateTunnelArgs {
    type Proto =
        crate::encoding::protobuf::generated::newport::desktop::configuration::UpdateTunnelArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::configuration::UpdateTunnelArgs { tunnel: Some(self.tunnel.to_proto()?), expected: Some(self.expected.to_proto()?) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            tunnel: <crate::desktop::configuration::Tunnel as Convert>::from_proto(required(
                value.tunnel,
            )?)?,
            expected: <crate::desktop::configuration::Tunnel as Convert>::from_proto(required(
                value.expected,
            )?)?,
        })
    }
}

impl Convert for crate::desktop::configuration::DeleteTunnelArgs {
    type Proto =
        crate::encoding::protobuf::generated::newport::desktop::configuration::DeleteTunnelArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::configuration::DeleteTunnelArgs { tunnel: Some(self.tunnel.to_string()) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            tunnel: uuid(required(value.tunnel)?)?,
        })
    }
}

impl Convert for crate::desktop::configuration::DeleteTunnelsArgs {
    type Proto =
        crate::encoding::protobuf::generated::newport::desktop::configuration::DeleteTunnelsArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::configuration::DeleteTunnelsArgs { expected: self.expected.iter().map(|v| -> Result<_, Error> { v.to_proto() }).collect::<Result<_, _>>()? })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            expected: value
                .expected
                .into_iter()
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::configuration::Tunnel as Convert>::from_proto(v)
                })
                .collect::<Result<_, _>>()?,
        })
    }
}

impl Convert for crate::desktop::configuration::SetTunnelConnectedArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::configuration::SetTunnelConnectedArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::configuration::SetTunnelConnectedArgs { tunnel: Some(self.tunnel.to_string()), connected: Some(self.connected) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            tunnel: uuid(required(value.tunnel)?)?,
            connected: required(value.connected)?,
        })
    }
}

impl Convert for crate::desktop::configuration::SetClipboardEnabledArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::configuration::SetClipboardEnabledArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::configuration::SetClipboardEnabledArgs { server: Some(self.server.to_string()), enabled: Some(self.enabled), expected: Some(self.expected.to_proto()?) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            enabled: required(value.enabled)?,
            expected: <crate::desktop::configuration::ConnectionScope as Convert>::from_proto(
                required(value.expected)?,
            )?,
        })
    }
}

impl Convert for crate::desktop::configuration::SetIntegrationEnabledArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::configuration::SetIntegrationEnabledArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::configuration::SetIntegrationEnabledArgs { server: Some(self.server.to_string()), feature: Some(self.feature.to_proto()?), enabled: Some(self.enabled), expected: Some(self.expected.to_proto()?) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            feature: <crate::desktop::configuration::Feature as Convert>::from_proto(required(
                value.feature,
            )?)?,
            enabled: required(value.enabled)?,
            expected: <crate::desktop::configuration::ConnectionScope as Convert>::from_proto(
                required(value.expected)?,
            )?,
        })
    }
}

impl Convert for crate::desktop::configuration::OpenTunnelArgs {
    type Proto =
        crate::encoding::protobuf::generated::newport::desktop::configuration::OpenTunnelArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::configuration::OpenTunnelArgs {
                tunnel: Some(self.tunnel.to_string()),
                remote_port: self
                    .remote_port
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(u32::from(*v)) })
                    .transpose()?,
                expected: self
                    .expected
                    .as_ref()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .transpose()?,
                expected_connection: self
                    .expected_connection
                    .as_ref()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .transpose()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            tunnel: uuid(required(value.tunnel)?)?,
            remote_port: value
                .remote_port
                .map(|v| -> Result<_, Error> {
                    Ok(u16::try_from(v).map_err(|_| Invalid("u16 range"))?)
                })
                .transpose()?,
            expected: value
                .expected
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::configuration::Tunnel as Convert>::from_proto(v)
                })
                .transpose()?,
            expected_connection: value
                .expected_connection
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::configuration::ConnectionScope as Convert>::from_proto(v)
                })
                .transpose()?,
        })
    }
}

impl Convert for crate::desktop::configuration::OpenTerminalLinkArgs {
    type Proto =
        crate::encoding::protobuf::generated::newport::desktop::configuration::OpenTerminalLinkArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::configuration::OpenTerminalLinkArgs { terminal: Some(self.terminal.to_string()), url: Some(self.url.clone()) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            terminal: uuid(required(value.terminal)?)?,
            url: required(value.url)?,
        })
    }
}

impl Convert for crate::desktop::configuration::Operation {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::configuration::Operation;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::configuration::Operation { value: Some(match self { crate::desktop::configuration::Operation::SaveServer(v) => crate::encoding::protobuf::generated::newport::desktop::configuration::operation::Value::SaveServer(Box::new(v.as_ref().to_proto()?)), crate::desktop::configuration::Operation::DeleteServer(v) => crate::encoding::protobuf::generated::newport::desktop::configuration::operation::Value::DeleteServer(Box::new(v.as_ref().to_proto()?)), crate::desktop::configuration::Operation::ForwardDiscoveredPort(v) => crate::encoding::protobuf::generated::newport::desktop::configuration::operation::Value::ForwardDiscoveredPort(Box::new(v.as_ref().to_proto()?)), crate::desktop::configuration::Operation::ForwardTunnelBatch(v) => crate::encoding::protobuf::generated::newport::desktop::configuration::operation::Value::ForwardTunnelBatch(Box::new(v.as_ref().to_proto()?)), crate::desktop::configuration::Operation::SaveTunnel(v) => crate::encoding::protobuf::generated::newport::desktop::configuration::operation::Value::SaveTunnel(Box::new(v.as_ref().to_proto()?)), crate::desktop::configuration::Operation::UpdateTunnel(v) => crate::encoding::protobuf::generated::newport::desktop::configuration::operation::Value::UpdateTunnel(Box::new(v.as_ref().to_proto()?)), crate::desktop::configuration::Operation::DeleteTunnel(v) => crate::encoding::protobuf::generated::newport::desktop::configuration::operation::Value::DeleteTunnel(Box::new(v.as_ref().to_proto()?)), crate::desktop::configuration::Operation::DeleteTunnels(v) => crate::encoding::protobuf::generated::newport::desktop::configuration::operation::Value::DeleteTunnels(Box::new(v.as_ref().to_proto()?)), crate::desktop::configuration::Operation::SetTunnelConnected(v) => crate::encoding::protobuf::generated::newport::desktop::configuration::operation::Value::SetTunnelConnected(Box::new(v.as_ref().to_proto()?)), crate::desktop::configuration::Operation::SetClipboardEnabled(v) => crate::encoding::protobuf::generated::newport::desktop::configuration::operation::Value::SetClipboardEnabled(Box::new(v.as_ref().to_proto()?)), crate::desktop::configuration::Operation::SetIntegrationEnabled(v) => crate::encoding::protobuf::generated::newport::desktop::configuration::operation::Value::SetIntegrationEnabled(Box::new(v.as_ref().to_proto()?)), crate::desktop::configuration::Operation::OpenTunnel(v) => crate::encoding::protobuf::generated::newport::desktop::configuration::operation::Value::OpenTunnel(Box::new(v.as_ref().to_proto()?)), crate::desktop::configuration::Operation::OpenTerminalLink(v) => crate::encoding::protobuf::generated::newport::desktop::configuration::operation::Value::OpenTerminalLink(Box::new(v.as_ref().to_proto()?)), crate::desktop::configuration::Operation::Snapshot(v) => crate::encoding::protobuf::generated::newport::desktop::configuration::operation::Value::Snapshot(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::configuration::operation::Value::SaveServer(v) => crate::desktop::configuration::Operation::SaveServer(Box::new(<crate::desktop::configuration::SaveServerArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::configuration::operation::Value::DeleteServer(v) => crate::desktop::configuration::Operation::DeleteServer(Box::new(<crate::desktop::configuration::DeleteServerArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::configuration::operation::Value::ForwardDiscoveredPort(v) => crate::desktop::configuration::Operation::ForwardDiscoveredPort(Box::new(<crate::desktop::configuration::ForwardDiscoveredPortArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::configuration::operation::Value::ForwardTunnelBatch(v) => crate::desktop::configuration::Operation::ForwardTunnelBatch(Box::new(<crate::desktop::configuration::ForwardTunnelBatchArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::configuration::operation::Value::SaveTunnel(v) => crate::desktop::configuration::Operation::SaveTunnel(Box::new(<crate::desktop::configuration::SaveTunnelArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::configuration::operation::Value::UpdateTunnel(v) => crate::desktop::configuration::Operation::UpdateTunnel(Box::new(<crate::desktop::configuration::UpdateTunnelArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::configuration::operation::Value::DeleteTunnel(v) => crate::desktop::configuration::Operation::DeleteTunnel(Box::new(<crate::desktop::configuration::DeleteTunnelArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::configuration::operation::Value::DeleteTunnels(v) => crate::desktop::configuration::Operation::DeleteTunnels(Box::new(<crate::desktop::configuration::DeleteTunnelsArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::configuration::operation::Value::SetTunnelConnected(v) => crate::desktop::configuration::Operation::SetTunnelConnected(Box::new(<crate::desktop::configuration::SetTunnelConnectedArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::configuration::operation::Value::SetClipboardEnabled(v) => crate::desktop::configuration::Operation::SetClipboardEnabled(Box::new(<crate::desktop::configuration::SetClipboardEnabledArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::configuration::operation::Value::SetIntegrationEnabled(v) => crate::desktop::configuration::Operation::SetIntegrationEnabled(Box::new(<crate::desktop::configuration::SetIntegrationEnabledArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::configuration::operation::Value::OpenTunnel(v) => crate::desktop::configuration::Operation::OpenTunnel(Box::new(<crate::desktop::configuration::OpenTunnelArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::configuration::operation::Value::OpenTerminalLink(v) => crate::desktop::configuration::Operation::OpenTerminalLink(Box::new(<crate::desktop::configuration::OpenTerminalLinkArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::configuration::operation::Value::Snapshot(v) => crate::desktop::configuration::Operation::Snapshot(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)) })
    }
}

impl Convert for crate::desktop::configuration::Reply {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::configuration::Reply;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::configuration::Reply { value: Some(match self { crate::desktop::configuration::Reply::SaveServer(v) => crate::encoding::protobuf::generated::newport::desktop::configuration::reply::Value::SaveServer(Box::new(v.as_ref().to_proto()?)), crate::desktop::configuration::Reply::DeleteServer(v) => crate::encoding::protobuf::generated::newport::desktop::configuration::reply::Value::DeleteServer(Box::new(v.as_ref().to_proto()?)), crate::desktop::configuration::Reply::ForwardDiscoveredPort(v) => crate::encoding::protobuf::generated::newport::desktop::configuration::reply::Value::ForwardDiscoveredPort(Box::new(v.as_ref().to_proto()?)), crate::desktop::configuration::Reply::ForwardTunnelBatch(v) => crate::encoding::protobuf::generated::newport::desktop::configuration::reply::Value::ForwardTunnelBatch(Box::new(v.as_ref().to_proto()?)), crate::desktop::configuration::Reply::SaveTunnel(v) => crate::encoding::protobuf::generated::newport::desktop::configuration::reply::Value::SaveTunnel(Box::new(v.as_ref().to_proto()?)), crate::desktop::configuration::Reply::UpdateTunnel(v) => crate::encoding::protobuf::generated::newport::desktop::configuration::reply::Value::UpdateTunnel(Box::new(v.as_ref().to_proto()?)), crate::desktop::configuration::Reply::DeleteTunnel(v) => crate::encoding::protobuf::generated::newport::desktop::configuration::reply::Value::DeleteTunnel(Box::new(v.as_ref().to_proto()?)), crate::desktop::configuration::Reply::DeleteTunnels(v) => crate::encoding::protobuf::generated::newport::desktop::configuration::reply::Value::DeleteTunnels(Box::new(v.as_ref().to_proto()?)), crate::desktop::configuration::Reply::SetTunnelConnected(v) => crate::encoding::protobuf::generated::newport::desktop::configuration::reply::Value::SetTunnelConnected(Box::new(v.as_ref().to_proto()?)), crate::desktop::configuration::Reply::SetClipboardEnabled(v) => crate::encoding::protobuf::generated::newport::desktop::configuration::reply::Value::SetClipboardEnabled(Box::new(v.as_ref().to_proto()?)), crate::desktop::configuration::Reply::SetIntegrationEnabled(v) => crate::encoding::protobuf::generated::newport::desktop::configuration::reply::Value::SetIntegrationEnabled(Box::new(v.as_ref().to_proto()?)), crate::desktop::configuration::Reply::OpenTunnel(v) => crate::encoding::protobuf::generated::newport::desktop::configuration::reply::Value::OpenTunnel(Box::new(v.as_ref().to_proto()?)), crate::desktop::configuration::Reply::OpenTerminalLink(v) => crate::encoding::protobuf::generated::newport::desktop::configuration::reply::Value::OpenTerminalLink(Box::new(v.as_ref().to_proto()?)), crate::desktop::configuration::Reply::Snapshot(v) => crate::encoding::protobuf::generated::newport::desktop::configuration::reply::Value::Snapshot(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::configuration::reply::Value::SaveServer(v) => crate::desktop::configuration::Reply::SaveServer(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::configuration::reply::Value::DeleteServer(v) => crate::desktop::configuration::Reply::DeleteServer(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::configuration::reply::Value::ForwardDiscoveredPort(v) => crate::desktop::configuration::Reply::ForwardDiscoveredPort(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::configuration::reply::Value::ForwardTunnelBatch(v) => crate::desktop::configuration::Reply::ForwardTunnelBatch(Box::new(<crate::desktop::configuration::Tunnels as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::configuration::reply::Value::SaveTunnel(v) => crate::desktop::configuration::Reply::SaveTunnel(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::configuration::reply::Value::UpdateTunnel(v) => crate::desktop::configuration::Reply::UpdateTunnel(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::configuration::reply::Value::DeleteTunnel(v) => crate::desktop::configuration::Reply::DeleteTunnel(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::configuration::reply::Value::DeleteTunnels(v) => crate::desktop::configuration::Reply::DeleteTunnels(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::configuration::reply::Value::SetTunnelConnected(v) => crate::desktop::configuration::Reply::SetTunnelConnected(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::configuration::reply::Value::SetClipboardEnabled(v) => crate::desktop::configuration::Reply::SetClipboardEnabled(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::configuration::reply::Value::SetIntegrationEnabled(v) => crate::desktop::configuration::Reply::SetIntegrationEnabled(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::configuration::reply::Value::OpenTunnel(v) => crate::desktop::configuration::Reply::OpenTunnel(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::configuration::reply::Value::OpenTerminalLink(v) => crate::desktop::configuration::Reply::OpenTerminalLink(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::configuration::reply::Value::Snapshot(v) => crate::desktop::configuration::Reply::Snapshot(Box::new(<crate::desktop::configuration::Snapshot as Convert>::from_proto(*v)?)) })
    }
}
