//! Existing business fixtures use JSON to describe persisted models and assert
//! UI projections. Their transport is exclusively the typed desktop protocol.
use newport_protocol::{
    common::{Present, Secret},
    desktop::{
        self, configuration as config, connection, containers, files, git, host, metrics,
        preferences, services,
    },
};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::{path::PathBuf, time::Duration};
fn field<'a>(value: &'a Value, key: &str) -> Result<&'a Value, String> {
    value.get(key).ok_or_else(|| format!("Missing {key}"))
}
fn get<T: DeserializeOwned>(value: &Value, key: &str) -> Result<T, String> {
    serde_json::from_value(field(value, key)?.clone()).map_err(|_| format!("Invalid {key}"))
}
fn optional<T: DeserializeOwned>(value: &Value, key: &str) -> Result<Option<T>, String> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => serde_json::from_value(value.clone())
            .map(Some)
            .map_err(|_| format!("Invalid {key}")),
    }
}
fn defaulted<T: DeserializeOwned + Default>(value: &Value, key: &str) -> Result<T, String> {
    optional(value, key).map(Option::unwrap_or_default)
}
fn model_server(value: &Value, key: &str) -> Result<config::Server, String> {
    get(value, key).map(|server: crate::model::Server| config::Server::from(&server))
}
fn model_tunnel(value: &Value, key: &str) -> Result<config::Tunnel, String> {
    get(value, key).map(|tunnel: crate::model::Tunnel| config::Tunnel::from(&tunnel))
}
fn tunnels(value: &Value, key: &str) -> Result<Vec<config::Tunnel>, String> {
    get::<Vec<crate::model::Tunnel>>(value, key)
        .map(|rows: Vec<crate::model::Tunnel>| rows.iter().map(config::Tunnel::from).collect())
}
fn scope(value: &Value) -> Result<config::ConnectionScope, String> {
    let decode = || -> Result<_, String> {
        let rows = value
            .as_array()
            .filter(|rows| rows.len() == 11)
            .ok_or("Invalid connection tuple")?;
        fn at<T: DeserializeOwned>(rows: &[Value], i: usize) -> Result<T, String> {
            serde_json::from_value(rows[i].clone()).map_err(|_| "Invalid connection field".into())
        }
        let auth: crate::model::AuthMethod = at(rows, 4)?;
        let proxy: Option<crate::model::Proxy> = at(rows, 8)?;
        Ok(config::ConnectionScope {
            settings: config::ConnectionSettings {
                server_id: at(rows, 0)?,
                ssh_host: at(rows, 1)?,
                ssh_user: at(rows, 2)?,
                ssh_port: at(rows, 3)?,
                auth_method: match auth {
                    crate::model::AuthMethod::PublicKey => config::AuthMethod::PublicKey,
                    crate::model::AuthMethod::Password => config::AuthMethod::Password,
                },
                identity_file: at(rows, 5)?,
                agent_source: at(rows, 6)?,
                agent_key_fingerprint: at(rows, 7)?,
                proxy: proxy.as_ref().map(config::Proxy::from),
            },
            connection_revision: at(rows, 9)?,
            backend_instance: at(rows, 10)?,
        })
    };
    decode().map_err(|_| "The server connection changed. Reopen the project.".into())
}
/// The list form `scope` reads, for fixtures that describe calls in JSON.
pub(crate) fn tuple(scope: &config::ConnectionScope) -> Value {
    let settings = &scope.settings;
    serde_json::json!([
        settings.server_id,
        settings.ssh_host,
        settings.ssh_user,
        settings.ssh_port,
        match settings.auth_method {
            config::AuthMethod::PublicKey => "publicKey",
            config::AuthMethod::Password => "password",
        },
        settings.identity_file,
        settings.agent_source,
        settings.agent_key_fingerprint,
        match &settings.proxy {
            None => Value::Null,
            Some(config::Proxy::Command(command)) => serde_json::json!({"command": **command}),
            Some(config::Proxy::Jump(id)) => serde_json::json!({"jump": **id}),
        },
        scope.connection_revision,
        scope.backend_instance,
    ])
}
fn expected(args: &Value) -> Result<config::ConnectionScope, String> {
    scope(field(args, "expectedConnection")?)
}
fn maybe_expected(args: &Value) -> Result<Option<config::ConnectionScope>, String> {
    args.get("expectedConnection").map(scope).transpose()
}
fn local_path(args: &Value) -> Result<files::NativePath, String> {
    let path: std::ffi::OsString = get(args, "localPath")?;
    if !std::path::Path::new(&path).is_absolute() {
        return Err("Invalid selected local path".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(files::NativePath::Unix(Box::new(path.into_vec().into())))
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        Ok(files::NativePath::Windows(Box::new(
            path.encode_wide().collect(),
        )))
    }
}
pub(crate) fn project(value: &Value) -> Result<preferences::Project, String> {
    let project: crate::preferences::projects::Project =
        serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
    Ok(preferences::Project {
        id: project.id,
        server: project.server_id,
        name: project.name,
        path: project.path,
    })
}

pub(crate) fn operation(method: &str, a: Value) -> Result<desktop::Operation, String> {
    use desktop::Operation as O;
    Ok(match method {
        "save_server" => O::Configuration(Box::new(config::Operation::SaveServer(Box::new(
            config::SaveServerArgs {
                server: model_server(&a, "server")?,
                password: optional::<String>(&a, "password")?.map(|value| Secret {
                    value: value.into_bytes().into(),
                }),
            },
        )))),
        "delete_server" => O::Configuration(Box::new(config::Operation::DeleteServer(Box::new(
            config::DeleteServerArgs {
                server: get(&a, "id")?,
            },
        )))),
        "forward_discovered_port" => O::Configuration(Box::new(
            config::Operation::ForwardDiscoveredPort(Box::new(config::ForwardDiscoveredPortArgs {
                server: get(&a, "serverId")?,
                address: get(&a, "address")?,
                port: get(&a, "port")?,
            })),
        )),
        "forward_tunnel_batch" => O::Configuration(Box::new(
            config::Operation::ForwardTunnelBatch(Box::new(config::ForwardTunnelBatchArgs {
                server: get(&a, "serverId")?,
                create: tunnels(&a, "create")?,
                connect: tunnels(&a, "connect")?,
                expected: maybe_expected(&a)?,
            })),
        )),
        "save_tunnel" => O::Configuration(Box::new(config::Operation::SaveTunnel(Box::new(
            config::SaveTunnelArgs {
                tunnel: model_tunnel(&a, "tunnel")?,
            },
        )))),
        "update_tunnel" => O::Configuration(Box::new(config::Operation::UpdateTunnel(Box::new(
            config::UpdateTunnelArgs {
                tunnel: model_tunnel(&a, "tunnel")?,
                expected: model_tunnel(&a, "expected")?,
            },
        )))),
        "delete_tunnel" => O::Configuration(Box::new(config::Operation::DeleteTunnel(Box::new(
            config::DeleteTunnelArgs {
                tunnel: get(&a, "id")?,
            },
        )))),
        "delete_tunnels" => O::Configuration(Box::new(config::Operation::DeleteTunnels(Box::new(
            config::DeleteTunnelsArgs {
                expected: tunnels(&a, "tunnels")?,
            },
        )))),
        "set_tunnel_connected" => O::Configuration(Box::new(
            config::Operation::SetTunnelConnected(Box::new(config::SetTunnelConnectedArgs {
                tunnel: get(&a, "id")?,
                connected: get(&a, "connected")?,
            })),
        )),
        "set_clipboard_enabled" => O::Configuration(Box::new(
            config::Operation::SetClipboardEnabled(Box::new(config::SetClipboardEnabledArgs {
                server: get(&a, "id")?,
                enabled: get(&a, "enabled")?,
                expected: expected(&a)?,
            })),
        )),
        "set_integration_enabled" => O::Configuration(Box::new(
            config::Operation::SetIntegrationEnabled(Box::new(config::SetIntegrationEnabledArgs {
                server: get(&a, "id")?,
                enabled: get(&a, "enabled")?,
                expected: expected(&a)?,
                feature: match get::<String>(&a, "feature")?.as_str() {
                    "browser" => config::Feature::Browser,
                    "clipboard" => config::Feature::Clipboard,
                    _ => return Err("Invalid feature".into()),
                },
            })),
        )),
        "open_tunnel" => O::Configuration(Box::new(config::Operation::OpenTunnel(Box::new(
            config::OpenTunnelArgs {
                tunnel: get(&a, "id")?,
                remote_port: optional(&a, "remotePort")?,
                expected: a
                    .get("expected")
                    .map(|v| {
                        serde_json::from_value(v.clone())
                            .map(|tunnel: crate::model::Tunnel| config::Tunnel::from(&tunnel))
                            .map_err(|e| e.to_string())
                    })
                    .transpose()?,
                expected_connection: maybe_expected(&a)?,
            },
        )))),
        "open_terminal_link" => O::Configuration(Box::new(config::Operation::OpenTerminalLink(
            Box::new(config::OpenTerminalLinkArgs {
                terminal: get(&a, "terminalId")?,
                url: get(&a, "url")?,
            }),
        ))),
        "setup_inspect" => O::Connection(Box::new(connection::Operation::SetupInspect(Box::new(
            connection::SetupInspectArgs {
                expected_server: model_server(&a, "expectedConnection")?,
            },
        )))),
        "setup_install" => O::Connection(Box::new(connection::Operation::SetupInstall(Box::new(
            connection::SetupInstallArgs {
                expected_server: model_server(&a, "expectedConnection")?,
            },
        )))),
        "setup_finish" => O::Connection(Box::new(connection::Operation::SetupFinish(Box::new(
            connection::SetupFinishArgs {
                expected_server: model_server(&a, "expectedConnection")?,
                browser: get(&a, "browser")?,
                clipboard: get(&a, "clipboard")?,
            },
        )))),
        "setup_skip" => O::Connection(Box::new(connection::Operation::SetupSkip(Box::new(
            connection::SetupSkipArgs {
                expected_server: model_server(&a, "expectedConnection")?,
            },
        )))),
        "inspect_agent" => O::Connection(Box::new(connection::Operation::InspectAgent(Box::new(
            connection::InspectAgentArgs {
                expected_server: model_server(&a, "expectedConnection")?,
            },
        )))),
        "uninstall_agent" => O::Connection(Box::new(connection::Operation::UninstallAgent(
            Box::new(connection::UninstallAgentArgs {
                expected_server: model_server(&a, "expectedConnection")?,
            }),
        ))),
        "test_connection" => O::Connection(Box::new(connection::Operation::TestConnection(
            Box::new(connection::TestConnectionArgs {
                server: get(&a, "id")?,
            }),
        ))),
        "connections_query" => O::Connection(Box::new(connection::Operation::ConnectionsQuery(
            Box::new(connection::ConnectionsQueryArgs {
                server: get(&a, "id")?,
                refresh: defaulted(&a, "refresh")?,
            }),
        ))),
        "set_window_preferences" => {
            let p = field(&a, "preferences")?;
            O::Preferences(Box::new(preferences::Operation::SetWindow(Box::new(
                preferences::SetWindowArgs {
                    patch: preferences::WindowPatch {
                        sidebar_width: optional(p, "sidebarWidth")?,
                        sidebar_compact: optional(p, "sidebarCompact")?,
                        appearance: optional::<String>(p, "appearance")?
                            .map(|value| match value.as_str() {
                                "system" => Ok(preferences::Appearance::System),
                                "dark" => Ok(preferences::Appearance::Dark),
                                "light" => Ok(preferences::Appearance::Light),
                                _ => Err("Invalid appearance".to_string()),
                            })
                            .transpose()?,
                        terminal_option_as_meta: optional(p, "terminalOptionAsMeta")?,
                    },
                },
            ))))
        }
        "git_projects_list" => O::Preferences(Box::new(preferences::Operation::ProjectsList(
            Box::new(preferences::ProjectsListArgs {
                server: get(&a, "serverId")?,
            }),
        ))),
        "git_projects_save" => O::Preferences(Box::new(preferences::Operation::ProjectsSave(
            Box::new(preferences::ProjectsSaveArgs {
                project: project(field(&a, "project")?)?,
            }),
        ))),
        "git_projects_remove" => O::Preferences(Box::new(preferences::Operation::ProjectsRemove(
            Box::new(preferences::ProjectsRemoveArgs {
                server: get(&a, "serverId")?,
                project: get(&a, "projectId")?,
            }),
        ))),
        "git_request" => O::Git(Box::new(git::Operation::Request(Box::new(
            git::RequestArgs {
                server: get(&a, "serverId")?,
                request: get(&a, "request")?,
                retry_connection: defaulted(&a, "retryConnection")?,
            },
        )))),
        "git_refresh_repository" => O::Git(Box::new(git::Operation::RefreshRepository(Box::new(
            git::RefreshRepositoryArgs {
                server: get(&a, "serverId")?,
                repo_id: get(&a, "repoId")?,
            },
        )))),
        "git_pending_operations" => O::Git(Box::new(git::Operation::PendingOperations(Box::new(
            git::PendingOperationsArgs {
                server: get(&a, "serverId")?,
            },
        )))),
        "git_acknowledge_operation" => O::Git(Box::new(git::Operation::AcknowledgeOperation(
            Box::new(git::AcknowledgeOperationArgs {
                server: get(&a, "serverId")?,
                operation_id: get(&a, "operationId")?,
            }),
        ))),
        "git_review_operation" => O::Git(Box::new(git::Operation::ReviewOperation(Box::new(
            git::ReviewOperationArgs {
                server: get(&a, "serverId")?,
                operation_id: get(&a, "operationId")?,
            },
        )))),
        "cockpit_history" => O::Metrics(Box::new(metrics::Operation::History(Box::new(
            metrics::HistoryArgs {
                server: get(&a, "id")?,
                minutes: optional(&a, "minutes")?,
            },
        )))),
        "get_metrics_cache" => {
            O::Metrics(Box::new(metrics::Operation::Cache(Box::new(Present {}))))
        }
        "clear_metrics_cache" => O::Metrics(Box::new(metrics::Operation::ClearCache(Box::new(
            Present {},
        )))),
        "files_list" => O::Files(Box::new(files::Operation::List(Box::new(
            files::ListArgs {
                server: get(&a, "id")?,
                operation: get(&a, "operation")?,
                path: get(&a, "path")?,
                expected: maybe_expected(&a)?,
                refresh: defaulted(&a, "refresh")?,
                project_root: optional(&a, "projectRoot")?,
            },
        )))),
        "files_preview" => O::Files(Box::new(files::Operation::Preview(Box::new(
            files::PreviewArgs {
                server: get(&a, "id")?,
                operation: get(&a, "operation")?,
                path: get(&a, "path")?,
                expected: maybe_expected(&a)?,
            },
        )))),
        "files_cancel" => O::Files(Box::new(files::Operation::Cancel(Box::new(
            files::CancelArgs {
                operation: get(&a, "operation")?,
            },
        )))),
        "files_progress" => O::Files(Box::new(files::Operation::Progress(Box::new(
            files::ProgressArgs {
                operation: get(&a, "operation")?,
            },
        )))),
        "native.files_upload" => {
            let local_path = local_path(&a)?;
            O::Files(Box::new(files::Operation::Upload(Box::new(
                files::UploadArgs {
                    server: get(&a, "id")?,
                    operation: get(&a, "operation")?,
                    path: get(&a, "path")?,
                    local_path,
                    expected: expected(&a)?,
                },
            ))))
        }
        "native.files_download" => {
            let local_path = local_path(&a)?;
            O::Files(Box::new(files::Operation::Download(Box::new(
                files::DownloadArgs {
                    server: get(&a, "id")?,
                    operation: get(&a, "operation")?,
                    path: get(&a, "path")?,
                    local_path,
                    expected: expected(&a)?,
                },
            ))))
        }
        "services_snapshot" => O::Services(Box::new(services::Operation::Snapshot(Box::new(
            services::SnapshotArgs {
                server: get(&a, "id")?,
                refresh: defaulted(&a, "refresh")?,
            },
        )))),
        "service_action" => O::Services(Box::new(services::Operation::Action(Box::new(
            services::ActionArgs {
                server: get(&a, "id")?,
                unit: get(&a, "unit")?,
                scope: get(&a, "scope")?,
                verb: get(&a, "verb")?,
            },
        )))),
        "service_logs" => O::Services(Box::new(services::Operation::Logs(Box::new(
            services::LogsArgs {
                server: get(&a, "id")?,
                request: serde_json::from_value(a.clone()).map_err(|e| e.to_string())?,
            },
        )))),
        "service_unit_file" => O::Services(Box::new(services::Operation::UnitFile(Box::new(
            services::UnitFileArgs {
                server: get(&a, "id")?,
                unit: get(&a, "unit")?,
                scope: get(&a, "scope")?,
                refresh: defaulted(&a, "refresh")?,
            },
        )))),
        "container_action" => O::Containers(Box::new(containers::Operation::Action(Box::new(
            containers::ActionArgs {
                server: get(&a, "id")?,
                container: get(&a, "container")?,
                verb: get(&a, "verb")?,
            },
        )))),
        "containers_remove" => O::Containers(Box::new(containers::Operation::Remove(Box::new(
            containers::RemoveArgs {
                server: get(&a, "id")?,
                containers: get(&a, "containers")?,
            },
        )))),
        "docker_prune" => O::Containers(Box::new(containers::Operation::Prune(Box::new(
            containers::PruneArgs {
                server: get(&a, "id")?,
                what: get(&a, "what")?,
            },
        )))),
        "compose_run" => O::Containers(Box::new(containers::Operation::ComposeRun(Box::new(
            containers::ComposeRunArgs {
                server: get(&a, "id")?,
                project: get(&a, "project")?,
                action: get(&a, "action")?,
            },
        )))),
        "compose_job" => O::Containers(Box::new(containers::Operation::ComposeJob(Box::new(
            containers::ComposeJobArgs {
                server: get(&a, "id")?,
                job: get(&a, "job")?,
                offset: defaulted(&a, "offset")?,
            },
        )))),
        "activate_window" => {
            return Ok(O::Host(Box::new(host::Operation::Activate(Box::new(
                Present {},
            )))));
        }
        _ => return Err("Unknown command".into()),
    })
}
/// The backend's configuration snapshot, read typed as the app reads it.
pub(crate) fn snapshot(endpoint: PathBuf, timeout: Duration) -> Result<config::Snapshot, String> {
    let reply = newport_client::backend::Backend::at_path(endpoint)
        .execute_with_timeout(
            desktop::Operation::Configuration(Box::new(config::Operation::Snapshot(Box::new(
                Present {},
            )))),
            timeout,
        )
        .map_err(|e| format!("{}: {e}", e.code()))?;
    newport_client::backend::snapshot_reply(reply).map_err(|e| e.to_string())
}
/// A fixture method's decoded reply, for replies the app reads typed.
pub(crate) fn execute(
    endpoint: PathBuf,
    method: &str,
    args: Value,
    timeout: Duration,
) -> Result<desktop::Reply, String> {
    newport_client::backend::Backend::at_path(endpoint)
        .execute_with_timeout(operation(method, args)?, timeout)
        .map_err(|e| format!("{}: {e}", e.code()))
}
pub(crate) fn call(
    endpoint: PathBuf,
    method: &str,
    args: Value,
    timeout: Duration,
) -> Result<Value, String> {
    let operation = operation(method, args)?;
    let reply = newport_client::backend::Backend::at_path(endpoint)
        .execute_with_timeout(operation, timeout)
        .map_err(|e| format!("{}: {e}", e.code()))?;
    if let desktop::Reply::Git(command) = &reply
        && let git::Reply::Request(result) = command.as_ref()
    {
        return crate::git::project_result(result.as_ref()).map_err(|e| e.to_string());
    }
    // Fixtures see the destination the app's native transfer adapter would
    // report, and a listing's folder and entries.
    if let desktop::Reply::Files(command) = reply {
        return match *command {
            files::Reply::Upload(result) | files::Reply::Download(result) => {
                match result.path.ok_or("File transfer returned no destination")? {
                    files::TransferPath::Remote(path) => Ok(Value::String(*path)),
                    files::TransferPath::Local(path) => {
                        newport_client::desktop_inputs::local_path(*path)
                            .map(|path| Value::String(path.display().to_string()))
                            .map_err(|e| e.to_string())
                    }
                }
            }
            files::Reply::List(listing) => Ok(serde_json::json!({
                "path": listing.path,
                "entries": listing
                    .entries
                    .iter()
                    .map(|entry| serde_json::json!({"name": entry.name, "path": entry.path}))
                    .collect::<Vec<_>>(),
            })),
            files::Reply::Cancel(_) => Ok(Value::Null),
            _ => Err("This file reply is read as a typed reply".into()),
        };
    }
    acknowledged(reply)
}

/// What fixtures read back from a write: a forward batch's saved tunnels, or
/// null for an acknowledgement.
fn acknowledged(reply: desktop::Reply) -> Result<Value, String> {
    let typed = || Err("This reply is read as a typed reply".to_owned());
    match reply {
        desktop::Reply::Configuration(reply) => match *reply {
            config::Reply::Snapshot(_) => typed(),
            config::Reply::ForwardTunnelBatch(batch) => serde_json::to_value(
                batch
                    .tunnels
                    .iter()
                    .map(crate::model::Tunnel::from)
                    .collect::<Vec<_>>(),
            )
            .map_err(|e| e.to_string()),
            _ => Ok(Value::Null),
        },
        // Setup fixtures check only that the server reported a plan.
        desktop::Reply::Connection(reply) => match *reply {
            connection::Reply::SetupInspect(inspection) => Ok(serde_json::json!({
                "installed": inspection.installed,
                "total": inspection.total,
            })),
            connection::Reply::SetupInstall(_)
            | connection::Reply::SetupFinish(_)
            | connection::Reply::SetupSkip(_)
            | connection::Reply::TestConnection(_) => Ok(Value::Null),
            _ => typed(),
        },
        desktop::Reply::Host(reply) => match *reply {
            host::Reply::Activate(_) | host::Reply::OpenRemoteApp(_) => Ok(Value::Null),
            _ => typed(),
        },
        desktop::Reply::Preferences(reply) => match *reply {
            preferences::Reply::SetWindow(_) => Ok(Value::Null),
            _ => typed(),
        },
        desktop::Reply::Git(reply) => match *reply {
            git::Reply::AcknowledgeOperation(_) | git::Reply::ReviewOperation(_) => Ok(Value::Null),
            _ => typed(),
        },
        _ => typed(),
    }
}

/// Test-only bridge for existing business fixture syntax. No method names or
/// JSON data enter the transport.
pub(crate) struct Client(PathBuf);
impl Client {
    pub(crate) fn at_path(endpoint: PathBuf) -> Self {
        Self(endpoint)
    }
    pub(crate) fn call(&self, method: &str, args: Value) -> Result<Value, String> {
        call(self.0.clone(), method, args, Duration::from_secs(30))
    }
}
