//! Typed server, forwarding and integration routes.
use super::{
    configuration as convert,
    failure::{operation, remote},
    projection as p,
};
use crate::{desktop_integration::Desktop, manager::Shared, metrics_db::MetricsDb};
use newport_protocol::{
    common::Present,
    desktop::{Failure, configuration as config, connection},
};
pub struct Services<'a> {
    pub integration_access: &'a crate::integration_permissions::Access,
    pub manager: &'a Shared,
    pub metrics: &'a MetricsDb,
    pub files: &'a crate::files::Operations,
    pub terminals: &'a crate::terminal::Sessions,
    pub preferences: &'a crate::preferences::Preferences,
    pub desktop: Desktop,
}

pub async fn configuration(
    command: &config::Operation,
    services: &Services<'_>,
) -> Result<config::Reply, Failure> {
    use config::{Operation as O, Reply as R};
    Ok(match command {
        O::SaveServer(args) => {
            crate::commands::save_server(
                services.files,
                services.terminals,
                services.manager,
                services.desktop.clone(),
                convert::model_server(&args.server),
                args.password
                    .as_ref()
                    .map(|password| String::from_utf8(password.value.to_vec()).map_err(operation))
                    .transpose()?,
            )
            .await
            .map_err(operation)?;
            R::SaveServer(Box::new(Present {}))
        }
        O::DeleteServer(args) => {
            crate::commands::delete_server(
                services.files,
                services.terminals,
                services.manager,
                services.metrics,
                services.preferences,
                services.desktop.clone(),
                args.server,
            )
            .await
            .map_err(operation)?;
            R::DeleteServer(Box::new(Present {}))
        }
        O::ForwardDiscoveredPort(args) => {
            crate::commands::forward_discovered_port(
                services.manager,
                args.server,
                args.address.clone(),
                args.port,
            )
            .await
            .map_err(operation)?;
            R::ForwardDiscoveredPort(Box::new(Present {}))
        }
        O::ForwardTunnelBatch(args) => {
            let tunnels = crate::commands::forward_tunnel_batch(
                services.manager,
                args.server,
                args.create.iter().map(crate::model::Tunnel::from).collect(),
                args.connect
                    .iter()
                    .map(crate::model::Tunnel::from)
                    .collect(),
                args.expected.clone(),
            )
            .await
            .map_err(operation)?;
            R::ForwardTunnelBatch(Box::new(config::Tunnels {
                tunnels: tunnels.iter().map(config::Tunnel::from).collect(),
            }))
        }
        O::SaveTunnel(args) => {
            crate::commands::save_tunnel(
                services.manager,
                crate::model::Tunnel::from(&args.tunnel),
            )
            .await
            .map_err(operation)?;
            R::SaveTunnel(Box::new(Present {}))
        }
        O::UpdateTunnel(args) => {
            crate::commands::update_tunnel(
                services.manager,
                crate::model::Tunnel::from(&args.tunnel),
                crate::model::Tunnel::from(&args.expected),
            )
            .await
            .map_err(operation)?;
            R::UpdateTunnel(Box::new(Present {}))
        }
        O::DeleteTunnel(args) => {
            crate::commands::delete_tunnel(services.manager, args.tunnel)
                .await
                .map_err(operation)?;
            R::DeleteTunnel(Box::new(Present {}))
        }
        O::DeleteTunnels(args) => {
            crate::commands::delete_tunnels(
                services.manager,
                args.expected
                    .iter()
                    .map(crate::model::Tunnel::from)
                    .collect(),
            )
            .await
            .map_err(operation)?;
            R::DeleteTunnels(Box::new(Present {}))
        }
        O::SetTunnelConnected(args) => {
            crate::commands::set_tunnel_connected(services.manager, args.tunnel, args.connected)
                .await
                .map_err(operation)?;
            R::SetTunnelConnected(Box::new(Present {}))
        }
        O::SetClipboardEnabled(args) => {
            crate::commands::set_clipboard_enabled(
                services.manager,
                services.desktop.clone(),
                args.server,
                args.enabled,
                args.expected.clone(),
                services.integration_access,
            )
            .await
            .map_err(operation)?;
            R::SetClipboardEnabled(Box::new(Present {}))
        }
        O::SetIntegrationEnabled(args) => {
            crate::commands::set_integration_enabled(
                services.manager,
                services.desktop.clone(),
                args.server,
                match args.feature {
                    config::Feature::Clipboard => crate::commands::IntegrationFeature::Clipboard,
                    config::Feature::Browser => crate::commands::IntegrationFeature::Browser,
                },
                args.enabled,
                args.expected.clone(),
                services.integration_access,
            )
            .await
            .map_err(operation)?;
            R::SetIntegrationEnabled(Box::new(Present {}))
        }
        O::OpenTunnel(args) => {
            crate::commands::open_tunnel(
                services.manager,
                services.desktop.clone(),
                args.tunnel,
                args.remote_port,
                args.expected.as_ref().map(crate::model::Tunnel::from),
                args.expected_connection.clone(),
            )
            .await
            .map_err(operation)?;
            R::OpenTunnel(Box::new(Present {}))
        }
        O::OpenTerminalLink(args) => {
            crate::commands::open_terminal_link(
                services.manager,
                services.terminals,
                services.desktop.clone(),
                args.terminal,
                args.url.clone(),
            )
            .await
            .map_err(operation)?;
            R::OpenTerminalLink(Box::new(Present {}))
        }
        O::Snapshot(_) => R::Snapshot(Box::new(convert::snapshot(
            crate::commands::snapshot(services.manager)
                .await
                .map_err(operation)?,
        )?)),
    })
}

pub async fn connection(
    command: &connection::Operation,
    services: &Services<'_>,
) -> Result<connection::Reply, Failure> {
    use connection::{Operation as O, Reply as R};
    Ok(match command {
        O::SetupInspect(args) => {
            let value = crate::server_setup::inspect(
                services.manager,
                convert::model_server(&args.expected_server),
            )
            .await
            .map_err(remote)?;
            R::SetupInspect(Box::new(connection::SetupInspection {
                os: p::text(p::field(&value, "os")?)?,
                architecture: p::text(p::field(&value, "arch")?)?,
                cores: p::optional(&value, "cores", p::unsigned)?,
                memory_kib: p::optional(&value, "memoryKiB", p::unsigned)?,
                free_kib: p::optional(&value, "freeKiB", p::unsigned)?,
                installed: p::bool_value(p::field(&value, "installed")?)?,
                total: p::unsigned(p::field(&value, "total")?)?,
            }))
        }
        O::SetupInstall(args) => {
            crate::server_setup::install(
                services.manager,
                convert::model_server(&args.expected_server),
            )
            .await
            .map_err(operation)?;
            R::SetupInstall(Box::new(Present {}))
        }
        O::SetupFinish(args) => R::SetupFinish(Box::new(connection::OptionalNotice {
            notice: crate::server_setup::finish(
                services.manager,
                services.desktop.clone(),
                convert::model_server(&args.expected_server),
                args.browser,
                args.clipboard,
            )
            .await
            .map_err(operation)?,
        })),
        O::SetupSkip(args) => {
            crate::server_setup::skip(
                services.manager,
                convert::model_server(&args.expected_server),
            )
            .await
            .map_err(operation)?;
            R::SetupSkip(Box::new(Present {}))
        }
        O::SshAgentKeys(_) => {
            let value = crate::commands::ssh_agent_keys().await;
            R::SshAgentKeys(Box::new(connection::AgentKeyList {
                warnings: value.warnings,
                keys: value
                    .keys
                    .into_iter()
                    .map(|key| connection::AgentKey {
                        source: key.source,
                        comment: key.comment,
                        fingerprint: key.fingerprint,
                        algorithm: key.algorithm,
                    })
                    .collect(),
            }))
        }
        O::SshConfigHosts(_) => {
            let value = crate::commands::ssh_config_hosts()
                .await
                .map_err(operation)?;
            R::SshConfigHosts(Box::new(connection::ConfigHosts {
                path: value.path,
                hosts: value
                    .hosts
                    .into_iter()
                    .map(|host| connection::ConfigHost {
                        id: host.id,
                        alias: host.alias,
                        host: host.host,
                        user: host.user,
                        port: host.port,
                        identity_file: host.identity_file,
                        proxy: host.proxy.as_ref().map(config::Proxy::from),
                        through: host.through,
                        importable: host.importable,
                        suggested: host.suggested,
                        note: host.note,
                    })
                    .collect(),
            }))
        }
        O::SshResolve(args) => {
            use crate::ssh_resolve::Error;
            // Without `ssh`, or for a host it can't be asked about, there is
            // nothing to fill in; that isn't a failure to show.
            let value = match crate::ssh_resolve::resolve(&args.host).await {
                Ok(value) => value,
                Err(Error::Unavailable | Error::InvalidHost) => Default::default(),
                Err(error) => return Err(operation(error)),
            };
            R::SshResolve(Box::new(connection::SshResolved {
                hostname: value.hostname,
                user: value.user,
                port: value.port,
                identity_file: value.identity_file,
                proxy_command: value.proxy_command,
                proxy_jump: value.proxy_jump,
            }))
        }
        O::TestConnection(args) => {
            crate::commands::test_connection(services.manager, args.server)
                .await
                .map_err(remote)?;
            R::TestConnection(Box::new(Present {}))
        }
        O::ConnectionsQuery(args) => {
            let value =
                crate::commands::connections_query(services.manager, args.server, args.refresh)
                    .await
                    .map_err(operation)?;
            R::ConnectionsQuery(Box::new(value))
        }
        O::ReinstallAgent(args) => {
            crate::commands::reinstall_agent(
                services.manager,
                services.desktop.clone(),
                args.server,
            )
            .await
            .map_err(operation)?;
            R::ReinstallAgent(Box::new(Present {}))
        }
        O::UninstallAgent(args) => R::UninstallAgent(Box::new(connection::UninstallOutput {
            text: crate::commands::uninstall_agent(
                services.manager,
                convert::model_server(&args.expected_server),
            )
            .await
            .map_err(operation)?,
        })),
        O::InspectAgent(args) => R::InspectAgent(Box::new(convert::installation_status(
            crate::commands::inspect_agent(
                services.manager,
                convert::model_server(&args.expected_server),
            )
            .await
            .map_err(operation)?,
        ))),
    })
}
#[cfg(test)]
mod tests;
