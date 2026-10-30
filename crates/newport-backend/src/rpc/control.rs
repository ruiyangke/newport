//! Native server, forwarding and integration routes with explicit host services.
use super::arguments::{argument, encode, error};
use crate::{desktop_integration::Desktop, manager::Shared, metrics_db::MetricsDb};
use serde::Deserialize;
use serde_json::Value;
#[cfg(test)]
mod tests;
pub struct Services<'a> {
    pub integration_access: &'a crate::integration_permissions::Access,
    pub manager: &'a Shared,
    pub metrics: &'a MetricsDb,
    pub files: &'a crate::files::Operations,
    pub terminals: &'a crate::terminal::Sessions,
    pub preferences: &'a crate::preferences::Preferences,
    pub desktop: Desktop,
}
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Command {
    SetupInspect,
    SetupInstall,
    SetupFinish,
    SetupSkip,
    SaveServer,
    DeleteServer,
    ForwardDiscoveredPort,
    ForwardTunnelBatch,
    SaveTunnel,
    UpdateTunnel,
    DeleteTunnel,
    DeleteTunnels,
    SetTunnelConnected,
    SetClipboardEnabled,
    SetIntegrationEnabled,
    TestConnection,
    ConnectionsQuery,
    ReinstallAgent,
    UninstallAgent,
    InspectAgent,
    OpenTunnel,
    OpenTerminalLink,
}
impl Command {
    pub fn parse(method: &str) -> Option<Self> {
        Self::deserialize(serde::de::value::StrDeserializer::<serde::de::value::Error>::new(method))
            .ok()
    }
    pub async fn dispatch(self, services: &Services<'_>, args: &Value) -> Result<Value, Value> {
        match self {
            Self::SetupInspect => encode(
                crate::server_setup::inspect(
                    services.manager,
                    argument(args, "expectedConnection")?,
                )
                .await
                .map_err(error)?,
            ),
            Self::SetupInstall => encode(
                crate::server_setup::install(
                    services.manager,
                    argument(args, "expectedConnection")?,
                )
                .await
                .map_err(error)?,
            ),
            Self::SetupFinish => encode(
                crate::server_setup::finish(
                    services.manager,
                    services.desktop.clone(),
                    argument(args, "expectedConnection")?,
                    argument(args, "browser")?,
                    argument(args, "clipboard")?,
                )
                .await
                .map_err(error)?,
            ),
            Self::SetupSkip => encode(
                crate::server_setup::skip(services.manager, argument(args, "expectedConnection")?)
                    .await
                    .map_err(error)?,
            ),
            Self::SaveServer => encode(
                crate::commands::save_server(
                    services.files,
                    services.terminals,
                    services.manager,
                    services.desktop.clone(),
                    argument(args, "server")?,
                    argument(args, "password")?,
                )
                .await
                .map_err(error)?,
            ),
            Self::DeleteServer => encode(
                crate::commands::delete_server(
                    services.files,
                    services.terminals,
                    services.manager,
                    services.metrics,
                    services.preferences,
                    argument(args, "id")?,
                )
                .await
                .map_err(error)?,
            ),
            Self::ForwardDiscoveredPort => encode(
                crate::commands::forward_discovered_port(
                    services.manager,
                    argument(args, "serverId")?,
                    argument(args, "address")?,
                    argument(args, "port")?,
                )
                .await
                .map_err(error)?,
            ),
            Self::ForwardTunnelBatch => encode(
                crate::commands::forward_tunnel_batch(
                    services.manager,
                    argument(args, "serverId")?,
                    argument(args, "create")?,
                    argument(args, "connect")?,
                    argument(args, "expectedConnection")?,
                )
                .await
                .map_err(error)?,
            ),
            Self::SaveTunnel => encode(
                crate::commands::save_tunnel(services.manager, argument(args, "tunnel")?)
                    .await
                    .map_err(error)?,
            ),
            Self::UpdateTunnel => encode(
                crate::commands::update_tunnel(
                    services.manager,
                    argument(args, "tunnel")?,
                    argument(args, "expected")?,
                )
                .await
                .map_err(error)?,
            ),
            Self::DeleteTunnel => encode(
                crate::commands::delete_tunnel(services.manager, argument(args, "id")?)
                    .await
                    .map_err(error)?,
            ),
            Self::DeleteTunnels => encode(
                crate::commands::delete_tunnels(services.manager, argument(args, "tunnels")?)
                    .await
                    .map_err(error)?,
            ),
            Self::SetTunnelConnected => encode(
                crate::commands::set_tunnel_connected(
                    services.manager,
                    argument(args, "id")?,
                    argument(args, "connected")?,
                )
                .await
                .map_err(error)?,
            ),
            Self::SetClipboardEnabled => encode(
                crate::commands::set_clipboard_enabled(
                    services.manager,
                    services.desktop.clone(),
                    argument(args, "id")?,
                    argument(args, "enabled")?,
                    argument(args, "expectedConnection")?,
                    services.integration_access,
                )
                .await
                .map_err(error)?,
            ),
            Self::SetIntegrationEnabled => encode(
                crate::commands::set_integration_enabled(
                    services.manager,
                    services.desktop.clone(),
                    argument(args, "id")?,
                    argument(args, "feature")?,
                    argument(args, "enabled")?,
                    argument(args, "expectedConnection")?,
                    services.integration_access,
                )
                .await
                .map_err(error)?,
            ),
            Self::TestConnection => encode(
                crate::commands::test_connection(services.manager, argument(args, "id")?)
                    .await
                    .map_err(error)?,
            ),
            Self::ConnectionsQuery => encode(
                crate::commands::connections_query(
                    services.manager,
                    argument(args, "id")?,
                    argument::<Option<bool>>(args, "refresh")?.unwrap_or(false),
                )
                .await
                .map_err(error)?,
            ),
            Self::UninstallAgent => encode(
                crate::commands::uninstall_agent(
                    services.manager,
                    argument(args, "expectedConnection")?,
                )
                .await
                .map_err(error)?,
            ),
            Self::InspectAgent => encode(
                crate::commands::inspect_agent(
                    services.manager,
                    argument(args, "expectedConnection")?,
                )
                .await
                .map_err(error)?,
            ),
            Self::ReinstallAgent => encode(
                crate::commands::reinstall_agent(
                    services.manager,
                    services.desktop.clone(),
                    argument(args, "id")?,
                )
                .await
                .map_err(error)?,
            ),
            Self::OpenTunnel => encode(
                crate::commands::open_tunnel(
                    services.manager,
                    services.desktop.clone(),
                    argument(args, "id")?,
                    argument(args, "remotePort")?,
                    argument(args, "expected")?,
                    argument(args, "expectedConnection")?,
                )
                .await
                .map_err(error)?,
            ),
            Self::OpenTerminalLink => encode(
                crate::commands::open_terminal_link(
                    services.manager,
                    services.terminals,
                    services.desktop.clone(),
                    argument(args, "terminalId")?,
                    argument(args, "url")?,
                )
                .await
                .map_err(error)?,
            ),
        }
    }
}
