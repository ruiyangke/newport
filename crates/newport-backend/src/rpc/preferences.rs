//! Preference routes; each one is a transaction on the shared preferences file.
use super::arguments::{argument, encode, error};
use crate::preferences::{self, Preferences, projects, remote_apps};
use serde::Deserialize;
use serde_json::Value;

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Command {
    SetWindowPreferences,
    GitProjectsList,
    GitProjectsSave,
    GitProjectsRemove,
    RemoteAppsSettings,
    RemoteAppsSettingsSave,
    RemoteAppsInstalled,
    RemoteAppsInstalledSave,
}
impl Command {
    pub fn parse(method: &str) -> Option<Self> {
        Self::deserialize(serde::de::value::StrDeserializer::<serde::de::value::Error>::new(method))
            .ok()
    }
    pub async fn dispatch(self, state: &Preferences, args: &Value) -> Result<Value, Value> {
        match self {
            Self::SetWindowPreferences => encode(
                preferences::set_window_preferences(state, argument(args, "preferences")?)
                    .await
                    .map_err(error)?,
            ),
            Self::GitProjectsList => encode(
                projects::git_projects_list(state, argument(args, "serverId")?)
                    .await
                    .map_err(error)?,
            ),
            Self::GitProjectsSave => encode(
                projects::git_projects_save(state, argument(args, "project")?)
                    .await
                    .map_err(error)?,
            ),
            Self::GitProjectsRemove => encode(
                projects::git_projects_remove(
                    state,
                    argument(args, "serverId")?,
                    argument(args, "projectId")?,
                )
                .await
                .map_err(error)?,
            ),
            Self::RemoteAppsSettings => encode(
                remote_apps::remote_apps_settings(state)
                    .await
                    .map_err(error)?,
            ),
            Self::RemoteAppsSettingsSave => encode(
                remote_apps::remote_apps_settings_save(state, argument(args, "settings")?)
                    .await
                    .map_err(error)?,
            ),
            Self::RemoteAppsInstalled => encode(
                remote_apps::remote_apps_installed(state)
                    .await
                    .map_err(error)?,
            ),
            Self::RemoteAppsInstalledSave => encode(
                remote_apps::remote_apps_installed_save(
                    state,
                    argument(args, "serverId")?,
                    argument(args, "installed")?,
                )
                .await
                .map_err(error)?,
            ),
        }
    }
}
