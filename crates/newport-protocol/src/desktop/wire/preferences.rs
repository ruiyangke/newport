use super::*;
impl Convert for crate::desktop::preferences::Appearance {
    type Proto = i32;
    fn to_proto(&self) -> Result<i32, Error> {
        Ok(match self {
            Self::System => 1,
            Self::Light => 2,
            Self::Dark => 3,
        })
    }
    fn from_proto(value: i32) -> Result<Self, Error> {
        Ok(match value {
            1 => Self::System,
            2 => Self::Light,
            3 => Self::Dark,
            _ => return Err(Invalid("Appearance").into()),
        })
    }
}

impl Convert for crate::desktop::preferences::Codec {
    type Proto = i32;
    fn to_proto(&self) -> Result<i32, Error> {
        Ok(match self {
            Self::Auto => 1,
            Self::Lossless => 2,
            Self::Fast => 3,
        })
    }
    fn from_proto(value: i32) -> Result<Self, Error> {
        Ok(match value {
            1 => Self::Auto,
            2 => Self::Lossless,
            3 => Self::Fast,
            _ => return Err(Invalid("Codec").into()),
        })
    }
}

impl Convert for crate::desktop::preferences::WindowPatch {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::preferences::WindowPatch;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::preferences::WindowPatch {
                sidebar_width: self
                    .sidebar_width
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(*v) })
                    .transpose()?,
                sidebar_compact: self
                    .sidebar_compact
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(*v) })
                    .transpose()?,
                appearance: self
                    .appearance
                    .as_ref()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .transpose()?,
                terminal_option_as_meta: self
                    .terminal_option_as_meta
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(*v) })
                    .transpose()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            sidebar_width: value
                .sidebar_width
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            sidebar_compact: value
                .sidebar_compact
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            appearance: value
                .appearance
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::preferences::Appearance as Convert>::from_proto(v)
                })
                .transpose()?,
            terminal_option_as_meta: value
                .terminal_option_as_meta
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
        })
    }
}

impl Convert for crate::desktop::preferences::Project {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::preferences::Project;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::preferences::Project {
                id: Some(self.id.to_string()),
                server: Some(self.server.to_string()),
                name: Some(self.name.clone()),
                path: Some(self.path.to_proto()?),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            id: uuid(required(value.id)?)?,
            server: uuid(required(value.server)?)?,
            name: required(value.name)?,
            path: <crate::git::Path as Convert>::from_proto(required(value.path)?)?,
        })
    }
}

impl Convert for crate::desktop::preferences::Projects {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::preferences::Projects;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::preferences::Projects {
                projects: self
                    .projects
                    .iter()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .collect::<Result<_, _>>()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            projects: value
                .projects
                .into_iter()
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::preferences::Project as Convert>::from_proto(v)
                })
                .collect::<Result<_, _>>()?,
        })
    }
}

impl Convert for crate::desktop::preferences::Settings {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::preferences::Settings;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::preferences::Settings {
                audio: Some(self.audio),
                codec: Some(self.codec.to_proto()?),
                fps_goal: Some(self.fps_goal),
                hud: Some(self.hud),
                separate_apps: Some(self.separate_apps),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            audio: required(value.audio)?,
            codec: <crate::desktop::preferences::Codec as Convert>::from_proto(required(
                value.codec,
            )?)?,
            fps_goal: required(value.fps_goal)?,
            hud: required(value.hud)?,
            separate_apps: required(value.separate_apps)?,
        })
    }
}

impl Convert for crate::desktop::preferences::Installed {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::preferences::Installed;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::preferences::Installed {
                servers: self
                    .servers
                    .iter()
                    .map(|v| -> Result<_, Error> { Ok(v.to_string()) })
                    .collect::<Result<_, _>>()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            servers: value
                .servers
                .into_iter()
                .map(|v| -> Result<_, Error> { uuid(v) })
                .collect::<Result<_, _>>()?,
        })
    }
}

impl Convert for crate::desktop::preferences::SetWindowArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::preferences::SetWindowArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::preferences::SetWindowArgs {
                patch: Some(self.patch.to_proto()?),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            patch: <crate::desktop::preferences::WindowPatch as Convert>::from_proto(required(
                value.patch,
            )?)?,
        })
    }
}

impl Convert for crate::desktop::preferences::ProjectsListArgs {
    type Proto =
        crate::encoding::protobuf::generated::newport::desktop::preferences::ProjectsListArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::preferences::ProjectsListArgs {
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

impl Convert for crate::desktop::preferences::ProjectsSaveArgs {
    type Proto =
        crate::encoding::protobuf::generated::newport::desktop::preferences::ProjectsSaveArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::preferences::ProjectsSaveArgs {
                project: Some(self.project.to_proto()?),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            project: <crate::desktop::preferences::Project as Convert>::from_proto(required(
                value.project,
            )?)?,
        })
    }
}

impl Convert for crate::desktop::preferences::ProjectsRemoveArgs {
    type Proto =
        crate::encoding::protobuf::generated::newport::desktop::preferences::ProjectsRemoveArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::preferences::ProjectsRemoveArgs { server: Some(self.server.to_string()), project: Some(self.project.to_string()) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            project: uuid(required(value.project)?)?,
        })
    }
}

impl Convert for crate::desktop::preferences::SettingsSaveArgs {
    type Proto =
        crate::encoding::protobuf::generated::newport::desktop::preferences::SettingsSaveArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::preferences::SettingsSaveArgs {
                settings: Some(self.settings.to_proto()?),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            settings: <crate::desktop::preferences::Settings as Convert>::from_proto(required(
                value.settings,
            )?)?,
        })
    }
}

impl Convert for crate::desktop::preferences::InstalledSaveArgs {
    type Proto =
        crate::encoding::protobuf::generated::newport::desktop::preferences::InstalledSaveArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::preferences::InstalledSaveArgs { server: Some(self.server.to_string()), installed: Some(self.installed) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            installed: required(value.installed)?,
        })
    }
}

impl Convert for crate::desktop::preferences::Operation {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::preferences::Operation;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::preferences::Operation { value: Some(match self { crate::desktop::preferences::Operation::SetWindow(v) => crate::encoding::protobuf::generated::newport::desktop::preferences::operation::Value::SetWindow(Box::new(v.as_ref().to_proto()?)), crate::desktop::preferences::Operation::ProjectsList(v) => crate::encoding::protobuf::generated::newport::desktop::preferences::operation::Value::ProjectsList(Box::new(v.as_ref().to_proto()?)), crate::desktop::preferences::Operation::ProjectsSave(v) => crate::encoding::protobuf::generated::newport::desktop::preferences::operation::Value::ProjectsSave(Box::new(v.as_ref().to_proto()?)), crate::desktop::preferences::Operation::ProjectsRemove(v) => crate::encoding::protobuf::generated::newport::desktop::preferences::operation::Value::ProjectsRemove(Box::new(v.as_ref().to_proto()?)), crate::desktop::preferences::Operation::Settings(v) => crate::encoding::protobuf::generated::newport::desktop::preferences::operation::Value::Settings(Box::new(v.as_ref().to_proto()?)), crate::desktop::preferences::Operation::SettingsSave(v) => crate::encoding::protobuf::generated::newport::desktop::preferences::operation::Value::SettingsSave(Box::new(v.as_ref().to_proto()?)), crate::desktop::preferences::Operation::Installed(v) => crate::encoding::protobuf::generated::newport::desktop::preferences::operation::Value::Installed(Box::new(v.as_ref().to_proto()?)), crate::desktop::preferences::Operation::InstalledSave(v) => crate::encoding::protobuf::generated::newport::desktop::preferences::operation::Value::InstalledSave(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::preferences::operation::Value::SetWindow(v) => crate::desktop::preferences::Operation::SetWindow(Box::new(<crate::desktop::preferences::SetWindowArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::preferences::operation::Value::ProjectsList(v) => crate::desktop::preferences::Operation::ProjectsList(Box::new(<crate::desktop::preferences::ProjectsListArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::preferences::operation::Value::ProjectsSave(v) => crate::desktop::preferences::Operation::ProjectsSave(Box::new(<crate::desktop::preferences::ProjectsSaveArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::preferences::operation::Value::ProjectsRemove(v) => crate::desktop::preferences::Operation::ProjectsRemove(Box::new(<crate::desktop::preferences::ProjectsRemoveArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::preferences::operation::Value::Settings(v) => crate::desktop::preferences::Operation::Settings(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::preferences::operation::Value::SettingsSave(v) => crate::desktop::preferences::Operation::SettingsSave(Box::new(<crate::desktop::preferences::SettingsSaveArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::preferences::operation::Value::Installed(v) => crate::desktop::preferences::Operation::Installed(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::preferences::operation::Value::InstalledSave(v) => crate::desktop::preferences::Operation::InstalledSave(Box::new(<crate::desktop::preferences::InstalledSaveArgs as Convert>::from_proto(*v)?)) })
    }
}

impl Convert for crate::desktop::preferences::Reply {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::preferences::Reply;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::preferences::Reply { value: Some(match self { crate::desktop::preferences::Reply::SetWindow(v) => crate::encoding::protobuf::generated::newport::desktop::preferences::reply::Value::SetWindow(Box::new(v.as_ref().to_proto()?)), crate::desktop::preferences::Reply::ProjectsList(v) => crate::encoding::protobuf::generated::newport::desktop::preferences::reply::Value::ProjectsList(Box::new(v.as_ref().to_proto()?)), crate::desktop::preferences::Reply::ProjectsSave(v) => crate::encoding::protobuf::generated::newport::desktop::preferences::reply::Value::ProjectsSave(Box::new(v.as_ref().to_proto()?)), crate::desktop::preferences::Reply::ProjectsRemove(v) => crate::encoding::protobuf::generated::newport::desktop::preferences::reply::Value::ProjectsRemove(Box::new(v.as_ref().to_proto()?)), crate::desktop::preferences::Reply::Settings(v) => crate::encoding::protobuf::generated::newport::desktop::preferences::reply::Value::Settings(Box::new(v.as_ref().to_proto()?)), crate::desktop::preferences::Reply::SettingsSave(v) => crate::encoding::protobuf::generated::newport::desktop::preferences::reply::Value::SettingsSave(Box::new(v.as_ref().to_proto()?)), crate::desktop::preferences::Reply::Installed(v) => crate::encoding::protobuf::generated::newport::desktop::preferences::reply::Value::Installed(Box::new(v.as_ref().to_proto()?)), crate::desktop::preferences::Reply::InstalledSave(v) => crate::encoding::protobuf::generated::newport::desktop::preferences::reply::Value::InstalledSave(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::preferences::reply::Value::SetWindow(v) => crate::desktop::preferences::Reply::SetWindow(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::preferences::reply::Value::ProjectsList(v) => crate::desktop::preferences::Reply::ProjectsList(Box::new(<crate::desktop::preferences::Projects as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::preferences::reply::Value::ProjectsSave(v) => crate::desktop::preferences::Reply::ProjectsSave(Box::new(<crate::desktop::preferences::Project as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::preferences::reply::Value::ProjectsRemove(v) => crate::desktop::preferences::Reply::ProjectsRemove(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::preferences::reply::Value::Settings(v) => crate::desktop::preferences::Reply::Settings(Box::new(<crate::desktop::preferences::Settings as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::preferences::reply::Value::SettingsSave(v) => crate::desktop::preferences::Reply::SettingsSave(Box::new(<crate::desktop::preferences::Settings as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::preferences::reply::Value::Installed(v) => crate::desktop::preferences::Reply::Installed(Box::new(<crate::desktop::preferences::Installed as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::preferences::reply::Value::InstalledSave(v) => crate::desktop::preferences::Reply::InstalledSave(Box::new(<crate::desktop::preferences::Installed as Convert>::from_proto(*v)?)) })
    }
}
