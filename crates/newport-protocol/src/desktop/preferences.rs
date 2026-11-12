//! Typed desktop preferences contracts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Appearance {
    System,
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    Auto,
    Lossless,
    Fast,
}

#[derive(Clone, Debug)]
pub struct WindowPatch {
    pub sidebar_width: Option<f32>,
    pub sidebar_compact: Option<bool>,
    pub appearance: Option<crate::desktop::preferences::Appearance>,
    pub terminal_option_as_meta: Option<bool>,
}

#[derive(Clone, Debug)]
pub struct Project {
    pub id: uuid::Uuid,
    pub server: uuid::Uuid,
    pub name: String,
    pub path: crate::git::Path,
}

#[derive(Clone, Debug)]
pub struct Projects {
    pub projects: Vec<crate::desktop::preferences::Project>,
}

#[derive(Clone, Debug)]
pub struct Settings {
    pub audio: bool,
    pub codec: crate::desktop::preferences::Codec,
    pub fps_goal: u32,
    pub hud: bool,
    pub separate_apps: bool,
}

#[derive(Clone, Debug)]
pub struct Installed {
    pub servers: Vec<uuid::Uuid>,
}

#[derive(Clone, Debug)]
pub struct SetWindowArgs {
    pub patch: crate::desktop::preferences::WindowPatch,
}

#[derive(Clone, Debug)]
pub struct ProjectsListArgs {
    pub server: uuid::Uuid,
}

#[derive(Clone, Debug)]
pub struct ProjectsSaveArgs {
    pub project: crate::desktop::preferences::Project,
}

#[derive(Clone, Debug)]
pub struct ProjectsRemoveArgs {
    pub server: uuid::Uuid,
    pub project: uuid::Uuid,
}

#[derive(Clone, Debug)]
pub struct SettingsSaveArgs {
    pub settings: crate::desktop::preferences::Settings,
}

#[derive(Clone, Debug)]
pub struct InstalledSaveArgs {
    pub server: uuid::Uuid,
    pub installed: bool,
}

#[derive(Clone, Debug)]
pub enum Operation {
    SetWindow(Box<crate::desktop::preferences::SetWindowArgs>),
    ProjectsList(Box<crate::desktop::preferences::ProjectsListArgs>),
    ProjectsSave(Box<crate::desktop::preferences::ProjectsSaveArgs>),
    ProjectsRemove(Box<crate::desktop::preferences::ProjectsRemoveArgs>),
    Settings(Box<crate::common::Present>),
    SettingsSave(Box<crate::desktop::preferences::SettingsSaveArgs>),
    Installed(Box<crate::common::Present>),
    InstalledSave(Box<crate::desktop::preferences::InstalledSaveArgs>),
}

#[derive(Clone, Debug)]
pub enum Reply {
    SetWindow(Box<crate::common::Present>),
    ProjectsList(Box<crate::desktop::preferences::Projects>),
    ProjectsSave(Box<crate::desktop::preferences::Project>),
    ProjectsRemove(Box<crate::common::Present>),
    Settings(Box<crate::desktop::preferences::Settings>),
    SettingsSave(Box<crate::desktop::preferences::Settings>),
    Installed(Box<crate::desktop::preferences::Installed>),
    InstalledSave(Box<crate::desktop::preferences::Installed>),
}
