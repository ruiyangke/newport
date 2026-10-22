//! Feature command dispatch: each command gets the services it uses, nothing more.
use super::arguments::{argument, encode, error};
use crate::{manager::Shared, metrics_db::MetricsDb, terminal::Sessions};
use serde::Deserialize;
use serde_json::Value;
#[cfg(test)]
use serde_json::json;

pub struct Services<'a> {
    pub manager: &'a Shared,
    pub metrics: &'a MetricsDb,
    pub files: &'a crate::files::Operations,
    pub terminals: &'a Sessions,
}

/// Feature command allowlist. Terminals open only through the terminal lease
/// protocol, which ties each one to the connection that opened it.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Command {
    FilesList,
    FilesWatch,
    FilesPreview,
    FilesReadText,
    FilesProjectIndex,
    FilesWriteText,
    FilesCancel,
    FilesProgress,
    #[serde(rename = "native.files_upload")]
    NativeFilesUpload,
    #[serde(rename = "native.files_download")]
    NativeFilesDownload,
    SshAgentKeys,
    SshConfigHosts,
    CockpitCollect,
    CockpitHistory,
    GetMetricsCache,
    ClearMetricsCache,
    ServicesSnapshot,
    RemoteAppsStatus,
    RemoteAppsCatalogue,
    ServiceLogs,
    ServiceUnitFile,
    ServiceAction,
    ContainersSnapshot,
    ContainerInspect,
    ContainerLogs,
    ContainerAction,
    ProjectAction,
    ContainersRemove,
    DockerDisk,
    DockerPrune,
    ComposeProject,
    ComposeRun,
    ComposeJob,
    ComposeCancel,
    ComposeJobs,
    TerminalRead,
    TerminalWrite,
    TerminalResize,
    TerminalClose,
    Snapshot,
}
impl Command {
    pub fn parse(method: &str) -> Option<Self> {
        Self::deserialize(serde::de::value::StrDeserializer::<serde::de::value::Error>::new(method))
            .ok()
    }
    pub async fn dispatch(self, services: &Services<'_>, args: &Value) -> Result<Value, Value> {
        // Explicit refresh bypasses freshness without invalidating other keys.
        // Ordinary reads share the server-owned query and retain the RPC shape.
        if args.get("refresh").and_then(Value::as_bool) == Some(true) {
            use crate::server_queries::Read;
            let key = match self {
                Self::ServicesSnapshot => Some(Read::Services),
                Self::ServiceUnitFile => Some(Read::UnitFile {
                    unit: argument(args, "unit")?,
                    user: argument::<crate::services::Scope>(args, "scope")?
                        == crate::services::Scope::User,
                }),
                Self::ContainersSnapshot => Some(Read::Containers),
                Self::ContainerInspect => Some(Read::Inspect(argument(args, "container")?)),
                Self::DockerDisk => Some(Read::Disk(
                    crate::server_queries::DiskPart::parse(
                        argument::<Option<String>>(args, "part")?.as_deref(),
                    )
                    .map_err(error)?,
                )),
                Self::ComposeProject => Some(Read::Compose(argument(args, "project")?)),
                _ => None,
            };
            if let Some(key) = key {
                let owner = crate::server_session::resolve(services.manager, argument(args, "id")?)
                    .await
                    .map_err(error)?;
                return crate::server_queries::read(&owner, key, true)
                    .await
                    .map_err(error);
            }
        }
        match self {
            Self::FilesWatch => encode(
                crate::files::vfs::dispatch(
                    services.manager,
                    services.files,
                    serde_json::from_value(args.clone()).map_err(|e| error(e.to_string()))?,
                )
                .await
                .map_err(error)?,
            ),
            Self::FilesList => encode(
                crate::files::files_list_query(
                    services.manager,
                    services.files,
                    argument(args, "id")?,
                    argument(args, "operation")?,
                    argument(args, "path")?,
                    argument(args, "expectedConnection")?,
                    argument::<Option<bool>>(args, "refresh")?.unwrap_or(false),
                    argument::<Option<String>>(args, "projectRoot")?,
                )
                .await
                .map_err(error)?,
            ),
            Self::FilesProjectIndex => encode(
                crate::files::project_index::dispatch(
                    services.manager,
                    services.files,
                    serde_json::from_value(args.clone()).map_err(|e| error(e.to_string()))?,
                )
                .await
                .map_err(error)?,
            ),
            Self::FilesReadText | Self::FilesWriteText => encode(
                crate::files::editor::dispatch(
                    services.manager,
                    services.files,
                    serde_json::from_value(args.clone()).map_err(|e| error(e.to_string()))?,
                    matches!(self, Self::FilesWriteText),
                )
                .await
                .map_err(error)?,
            ),
            Self::FilesPreview => encode(
                crate::files::files_preview(
                    services.manager,
                    services.files,
                    argument(args, "id")?,
                    argument(args, "operation")?,
                    argument(args, "path")?,
                    argument(args, "expectedConnection")?,
                )
                .await
                .map_err(error)?,
            ),
            Self::NativeFilesUpload | Self::NativeFilesDownload => encode(
                crate::files::transfer::native_transfer(
                    services.manager,
                    services.files,
                    serde_json::from_value(args.clone()).map_err(|e| error(e.to_string()))?,
                    matches!(self, Self::NativeFilesUpload),
                )
                .await
                .map_err(error)?,
            ),
            Self::FilesCancel => {
                crate::files::operations::files_cancel(
                    services.files,
                    argument(args, "operation")?,
                );
                encode(())
            }
            Self::FilesProgress => encode(crate::files::operations::files_progress(
                services.files,
                argument(args, "operation")?,
            )),

            Self::SshAgentKeys => encode(crate::commands::ssh_agent_keys().await),
            Self::SshConfigHosts => {
                encode(crate::commands::ssh_config_hosts().await.map_err(error)?)
            }
            Self::CockpitCollect => encode(
                crate::commands::cockpit_collect(
                    services.manager,
                    services.metrics,
                    argument(args, "refresh")?,
                    argument(args, "id")?,
                    argument(args, "section")?,
                )
                .await
                .map_err(error)?,
            ),
            Self::CockpitHistory => encode(
                crate::commands::cockpit_history(
                    services.manager,
                    services.metrics,
                    argument(args, "id")?,
                    argument(args, "minutes")?,
                )
                .await
                .map_err(error)?,
            ),
            Self::GetMetricsCache => encode(
                crate::commands::get_metrics_cache(services.metrics)
                    .await
                    .map_err(error)?,
            ),
            Self::ClearMetricsCache => encode(
                crate::commands::clear_metrics_cache(services.manager, services.metrics)
                    .await
                    .map_err(error)?,
            ),
            Self::RemoteAppsStatus | Self::RemoteAppsCatalogue => {
                let key = if matches!(self, Self::RemoteAppsStatus) {
                    crate::server_queries::Read::RemoteAppsStatus
                } else {
                    crate::server_queries::Read::RemoteAppsCatalogue
                };
                let force = args.get("refresh").and_then(Value::as_bool) == Some(true);
                let owner = crate::server_session::resolve(services.manager, argument(args, "id")?)
                    .await
                    .map_err(error)?;
                crate::server_queries::read(&owner, key, force)
                    .await
                    .map_err(error)
            }
            Self::ServicesSnapshot => encode(
                crate::commands::services_snapshot(services.manager, argument(args, "id")?)
                    .await
                    .map_err(error)?,
            ),
            Self::ServiceLogs => encode(
                crate::commands::service_logs(
                    services.manager,
                    argument(args, "id")?,
                    argument(args, "unit")?,
                    argument(args, "scope")?,
                    argument(args, "priority")?,
                    argument(args, "cursor")?,
                    argument(args, "lines")?,
                )
                .await
                .map_err(error)?,
            ),
            Self::ServiceUnitFile => encode(
                crate::commands::service_unit_file(
                    services.manager,
                    argument(args, "id")?,
                    argument(args, "unit")?,
                    argument(args, "scope")?,
                )
                .await
                .map_err(error)?,
            ),
            Self::ServiceAction => encode(
                crate::commands::service_action(
                    services.manager,
                    argument(args, "id")?,
                    argument(args, "unit")?,
                    argument(args, "scope")?,
                    argument(args, "verb")?,
                )
                .await
                .map_err(error)?,
            ),
            Self::ContainersSnapshot => encode(
                crate::commands::containers_snapshot(services.manager, argument(args, "id")?)
                    .await
                    .map_err(error)?,
            ),
            Self::ContainerInspect => encode(
                crate::commands::container_inspect(
                    services.manager,
                    argument(args, "id")?,
                    argument(args, "container")?,
                )
                .await
                .map_err(error)?,
            ),
            Self::ContainerLogs => encode(
                crate::commands::container_logs(
                    services.manager,
                    argument(args, "id")?,
                    argument(args, "container")?,
                    argument(args, "project")?,
                    argument(args, "cursor")?,
                    argument(args, "lines")?,
                )
                .await
                .map_err(error)?,
            ),
            Self::ContainerAction => encode(
                crate::commands::container_action(
                    services.manager,
                    argument(args, "id")?,
                    argument(args, "container")?,
                    argument(args, "verb")?,
                )
                .await
                .map_err(error)?,
            ),
            Self::ProjectAction => encode(
                crate::commands::project_action(
                    services.manager,
                    argument(args, "id")?,
                    argument(args, "project")?,
                    argument(args, "verb")?,
                    argument(args, "expectedIds")?,
                )
                .await
                .map_err(error)?,
            ),
            Self::ContainersRemove => encode(
                crate::commands::containers_remove(
                    services.manager,
                    argument(args, "id")?,
                    argument(args, "containers")?,
                )
                .await
                .map_err(error)?,
            ),
            Self::DockerDisk => encode(
                crate::commands::docker_disk(
                    services.manager,
                    argument(args, "id")?,
                    argument(args, "part")?,
                )
                .await
                .map_err(error)?,
            ),
            Self::DockerPrune => encode(
                crate::commands::docker_prune(
                    services.manager,
                    argument(args, "id")?,
                    argument(args, "what")?,
                )
                .await
                .map_err(error)?,
            ),
            Self::ComposeProject => encode(
                crate::commands::compose_project(
                    services.manager,
                    argument(args, "id")?,
                    argument(args, "project")?,
                )
                .await
                .map_err(error)?,
            ),
            Self::ComposeRun => encode(
                crate::commands::compose_run(
                    services.manager,
                    argument(args, "id")?,
                    argument(args, "project")?,
                    argument(args, "action")?,
                )
                .await
                .map_err(error)?,
            ),
            Self::ComposeJob => encode(
                crate::commands::compose_job(
                    services.manager,
                    argument(args, "id")?,
                    argument(args, "job")?,
                    argument(args, "offset")?,
                )
                .await
                .map_err(error)?,
            ),
            Self::ComposeCancel => encode(
                crate::commands::compose_cancel(
                    services.manager,
                    argument(args, "id")?,
                    argument(args, "job")?,
                )
                .await
                .map_err(error)?,
            ),
            Self::ComposeJobs => encode(
                crate::commands::compose_jobs(services.manager, argument(args, "id")?)
                    .await
                    .map_err(error)?,
            ),
            Self::TerminalRead => encode(
                crate::commands::terminal_read(services.terminals, argument(args, "session")?)
                    .await
                    .map_err(error)?,
            ),
            Self::TerminalWrite => encode(
                crate::commands::terminal_write(
                    services.terminals,
                    argument(args, "session")?,
                    argument(args, "data")?,
                )
                .await
                .map_err(error)?,
            ),
            Self::TerminalResize => encode(
                crate::commands::terminal_resize(
                    services.terminals,
                    argument(args, "session")?,
                    argument(args, "cols")?,
                    argument(args, "rows")?,
                )
                .await
                .map_err(error)?,
            ),
            Self::TerminalClose => encode(
                crate::commands::terminal_close(services.terminals, argument(args, "session")?)
                    .await
                    .map_err(error)?,
            ),
            Self::Snapshot => encode(
                crate::commands::snapshot(services.manager)
                    .await
                    .map_err(error)?,
            ),
        }
    }
}

#[cfg(test)]
mod tests;
