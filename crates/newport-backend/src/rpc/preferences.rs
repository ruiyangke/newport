//! Native preference routes share the same transactions as the legacy desktop.
use super::arguments::{argument, encode, error};
use crate::preferences::{self, Preferences, projects};
use serde::Deserialize;
use serde_json::Value;

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Command {
    GetSidebarWidth,
    SetSidebarWidth,
    GitProjectsList,
    GitProjectsSave,
    GitProjectsRemove,
}
impl Command {
    pub fn parse(method: &str) -> Option<Self> {
        Self::deserialize(serde::de::value::StrDeserializer::<serde::de::value::Error>::new(method))
            .ok()
    }
    pub async fn dispatch(self, state: &Preferences, args: &Value) -> Result<Value, Value> {
        match self {
            Self::GetSidebarWidth => {
                let state = state.clone();
                encode(
                    tokio::task::spawn_blocking(move || preferences::get_sidebar_width(&state))
                        .await
                        .map_err(|e| error(e.to_string()))?
                        .map_err(error)?,
                )
            }
            Self::SetSidebarWidth => encode(
                preferences::set_sidebar_width(state, argument(args, "width")?)
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
        }
    }
}
