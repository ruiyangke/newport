use super::*;
impl Convert for crate::desktop::host::Startup {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::host::Startup;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::host::Startup {
                enabled: Some(self.enabled),
                available: Some(self.available),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            enabled: required(value.enabled)?,
            available: required(value.available)?,
        })
    }
}

impl Convert for crate::desktop::host::BuildStamp {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::host::BuildStamp;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::host::BuildStamp {
                commit: self
                    .commit
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
                local: Some(self.local),
                built_at: self
                    .built_at
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(*v) })
                    .transpose()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            commit: value
                .commit
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            local: required(value.local)?,
            built_at: value
                .built_at
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
        })
    }
}

impl Convert for crate::desktop::host::UpdatePhase {
    type Proto = i32;
    fn to_proto(&self) -> Result<i32, Error> {
        Ok(match self {
            Self::Idle => 1,
            Self::Checking => 2,
            Self::Downloading => 3,
            Self::Ready => 4,
            Self::Installing => 5,
            Self::Current => 6,
            Self::Error => 7,
        })
    }
    fn from_proto(value: i32) -> Result<Self, Error> {
        Ok(match value {
            1 => Self::Idle,
            2 => Self::Checking,
            3 => Self::Downloading,
            4 => Self::Ready,
            5 => Self::Installing,
            6 => Self::Current,
            7 => Self::Error,
            _ => return Err(Invalid("UpdatePhase").into()),
        })
    }
}

impl Convert for crate::desktop::host::UpdateStatus {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::host::UpdateStatus;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::host::UpdateStatus {
                enabled: Some(self.enabled),
                current_version: Some(self.current_version.clone()),
                build: Some(self.build.to_proto()?),
                phase: Some(self.phase.to_proto()?),
                version: self
                    .version
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
                downloaded: Some(self.downloaded),
                total: self
                    .total
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(*v) })
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
            enabled: required(value.enabled)?,
            current_version: required(value.current_version)?,
            build: <crate::desktop::host::BuildStamp as Convert>::from_proto(required(
                value.build,
            )?)?,
            phase: <crate::desktop::host::UpdatePhase as Convert>::from_proto(required(
                value.phase,
            )?)?,
            version: value
                .version
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            downloaded: required(value.downloaded)?,
            total: value
                .total
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            error: value
                .error
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
        })
    }
}

impl Convert for crate::desktop::host::OpenRemoteAppArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::host::OpenRemoteAppArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::host::OpenRemoteAppArgs {
                id: Some(self.id.clone()),
                action: self
                    .action
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            id: required(value.id)?,
            action: value
                .action
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
        })
    }
}

impl Convert for crate::desktop::host::SetLaunchAtLoginArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::host::SetLaunchAtLoginArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::host::SetLaunchAtLoginArgs {
                enabled: Some(self.enabled),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            enabled: required(value.enabled)?,
        })
    }
}

impl Convert for crate::desktop::host::InstallUpdateArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::host::InstallUpdateArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::host::InstallUpdateArgs {
                confirmed_version: Some(self.confirmed_version.clone()),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            confirmed_version: required(value.confirmed_version)?,
        })
    }
}

impl Convert for crate::desktop::host::Operation {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::host::Operation;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::host::Operation { value: Some(match self { crate::desktop::host::Operation::Activate(v) => crate::encoding::protobuf::generated::newport::desktop::host::operation::Value::Activate(Box::new(v.as_ref().to_proto()?)), crate::desktop::host::Operation::OpenRemoteApp(v) => crate::encoding::protobuf::generated::newport::desktop::host::operation::Value::OpenRemoteApp(Box::new(v.as_ref().to_proto()?)), crate::desktop::host::Operation::StartupSettings(v) => crate::encoding::protobuf::generated::newport::desktop::host::operation::Value::StartupSettings(Box::new(v.as_ref().to_proto()?)), crate::desktop::host::Operation::SetLaunchAtLogin(v) => crate::encoding::protobuf::generated::newport::desktop::host::operation::Value::SetLaunchAtLogin(Box::new(v.as_ref().to_proto()?)), crate::desktop::host::Operation::UpdateStatus(v) => crate::encoding::protobuf::generated::newport::desktop::host::operation::Value::UpdateStatus(Box::new(v.as_ref().to_proto()?)), crate::desktop::host::Operation::CheckForUpdates(v) => crate::encoding::protobuf::generated::newport::desktop::host::operation::Value::CheckForUpdates(Box::new(v.as_ref().to_proto()?)), crate::desktop::host::Operation::InstallUpdate(v) => crate::encoding::protobuf::generated::newport::desktop::host::operation::Value::InstallUpdate(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::host::operation::Value::Activate(v) => crate::desktop::host::Operation::Activate(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::host::operation::Value::OpenRemoteApp(v) => crate::desktop::host::Operation::OpenRemoteApp(Box::new(<crate::desktop::host::OpenRemoteAppArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::host::operation::Value::StartupSettings(v) => crate::desktop::host::Operation::StartupSettings(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::host::operation::Value::SetLaunchAtLogin(v) => crate::desktop::host::Operation::SetLaunchAtLogin(Box::new(<crate::desktop::host::SetLaunchAtLoginArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::host::operation::Value::UpdateStatus(v) => crate::desktop::host::Operation::UpdateStatus(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::host::operation::Value::CheckForUpdates(v) => crate::desktop::host::Operation::CheckForUpdates(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::host::operation::Value::InstallUpdate(v) => crate::desktop::host::Operation::InstallUpdate(Box::new(<crate::desktop::host::InstallUpdateArgs as Convert>::from_proto(*v)?)) })
    }
}

impl Convert for crate::desktop::host::Reply {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::host::Reply;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::host::Reply { value: Some(match self { crate::desktop::host::Reply::Activate(v) => crate::encoding::protobuf::generated::newport::desktop::host::reply::Value::Activate(Box::new(v.as_ref().to_proto()?)), crate::desktop::host::Reply::OpenRemoteApp(v) => crate::encoding::protobuf::generated::newport::desktop::host::reply::Value::OpenRemoteApp(Box::new(v.as_ref().to_proto()?)), crate::desktop::host::Reply::StartupSettings(v) => crate::encoding::protobuf::generated::newport::desktop::host::reply::Value::StartupSettings(Box::new(v.as_ref().to_proto()?)), crate::desktop::host::Reply::SetLaunchAtLogin(v) => crate::encoding::protobuf::generated::newport::desktop::host::reply::Value::SetLaunchAtLogin(Box::new(v.as_ref().to_proto()?)), crate::desktop::host::Reply::UpdateStatus(v) => crate::encoding::protobuf::generated::newport::desktop::host::reply::Value::UpdateStatus(Box::new(v.as_ref().to_proto()?)), crate::desktop::host::Reply::CheckForUpdates(v) => crate::encoding::protobuf::generated::newport::desktop::host::reply::Value::CheckForUpdates(Box::new(v.as_ref().to_proto()?)), crate::desktop::host::Reply::InstallUpdate(v) => crate::encoding::protobuf::generated::newport::desktop::host::reply::Value::InstallUpdate(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::host::reply::Value::Activate(v) => crate::desktop::host::Reply::Activate(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::host::reply::Value::OpenRemoteApp(v) => crate::desktop::host::Reply::OpenRemoteApp(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::host::reply::Value::StartupSettings(v) => crate::desktop::host::Reply::StartupSettings(Box::new(<crate::desktop::host::Startup as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::host::reply::Value::SetLaunchAtLogin(v) => crate::desktop::host::Reply::SetLaunchAtLogin(Box::new(<crate::desktop::host::Startup as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::host::reply::Value::UpdateStatus(v) => crate::desktop::host::Reply::UpdateStatus(Box::new(<crate::desktop::host::UpdateStatus as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::host::reply::Value::CheckForUpdates(v) => crate::desktop::host::Reply::CheckForUpdates(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::host::reply::Value::InstallUpdate(v) => crate::desktop::host::Reply::InstallUpdate(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)) })
    }
}
