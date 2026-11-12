//! Explicit typed reply projections into existing UI render state.
use super::product;
use newport_protocol::desktop::configuration::*;
use serde_json::{Value, json};
pub(super) fn auth_method(value: AuthMethod) -> &'static str {
    match value {
        AuthMethod::PublicKey => "publicKey",
        AuthMethod::Password => "password",
    }
}
pub(super) fn server(value: Server) -> Value {
    json!({"id": value.id, "name": value.name, "sshUser": value.ssh_user, "sshHost": value.ssh_host, "sshPort": value.ssh_port, "identityFile": value.identity_file, "agentSource": value.agent_source, "agentKeyFingerprint": value.agent_key_fingerprint, "authMethod": auth_method(value.auth_method), "clipboardEnabled": value.clipboard_enabled, "browserEnabled": value.browser_enabled, "agentInstallApproved": value.agent_install_approved, "setupComplete": value.setup_complete, "proxy": value.proxy.map(proxy)})
}
pub(super) fn direction(value: Direction) -> &'static str {
    match value {
        Direction::Local => "local",
        Direction::Remote => "remote",
    }
}
pub(crate) fn tunnel(value: Tunnel) -> Value {
    json!({"id": value.id, "name": value.name, "serverId": value.server_id, "localPort": value.local_port, "localPortEnd": value.local_port_end, "remoteHost": value.remote_host, "remotePort": value.remote_port, "remotePortEnd": value.remote_port_end, "autoConnect": value.auto_connect, "autoReconnect": value.auto_reconnect, "direction": direction(value.direction), "project": value.project})
}
pub(super) fn config(value: Config) -> Value {
    json!({"servers": value.servers.into_iter().map(server).collect::<Vec<_>>(), "tunnels": value.tunnels.into_iter().map(tunnel).collect::<Vec<_>>()})
}
pub(super) fn snapshot(value: Snapshot) -> Value {
    json!({"instanceId": value.instance_id, "revision": value.revision, "config": config(value.config), "runtime": runtime(value.runtime), "loadError": value.load_error})
}
pub(super) fn transport_status(value: TransportStatus) -> &'static str {
    match value {
        TransportStatus::Disconnected => "disconnected",
        TransportStatus::Connecting => "connecting",
        TransportStatus::Connected => "connected",
        TransportStatus::Recovering => "recovering",
        TransportStatus::Failed => "failed",
    }
}
pub(super) fn transport(value: Transport) -> Value {
    json!({"status": transport_status(value.status), "generation": value.generation, "waiting": value.waiting})
}
pub(super) fn runtime_instance(value: RuntimeInstance) -> Value {
    json!({"instanceId": value.instance_id, "buildSha256": value.build_sha256, "generation": value.generation, "retiring": value.retiring, "draining": value.draining, "connected": value.connected, "registrations": value.registrations, "workerLeases": value.worker_leases})
}
pub(super) fn install_stage(value: InstallStage) -> &'static str {
    match value {
        InstallStage::Preparing => "preparing",
        InstallStage::Uploading => "uploading",
        InstallStage::Installing => "installing",
        InstallStage::Verifying => "verifying",
        InstallStage::Activating => "activating",
        InstallStage::Ready => "ready",
        InstallStage::Error => "error",
    }
}
pub(super) fn install_progress(value: InstallProgress) -> Value {
    json!({"stage": install_stage(value.stage), "sent": value.sent, "total": value.total, "error": value.error, "failedStage": value.failed_stage.map(install_stage), "bytesPerSecond": value.bytes_per_second})
}
pub(super) fn connection_status(value: ConnectionStatus) -> &'static str {
    match value {
        ConnectionStatus::Disconnected => "disconnected",
        ConnectionStatus::Connecting => "connecting",
        ConnectionStatus::Connected => "connected",
        ConnectionStatus::Reconnecting => "reconnecting",
        ConnectionStatus::Error => "error",
    }
}
pub(super) fn connection_state(value: ConnectionState) -> Value {
    json!({"status": connection_status(value.status), "errorMessage": value.error_message, "reconnectAttempt": value.reconnect_attempt})
}
pub(super) fn destination_status(value: DestinationStatus) -> &'static str {
    match value {
        DestinationStatus::Checking => "checking",
        DestinationStatus::Reachable => "reachable",
        DestinationStatus::Unavailable => "unavailable",
        DestinationStatus::Blocked => "blocked",
        DestinationStatus::Unknown => "unknown",
    }
}
pub(super) fn destination_health(value: DestinationHealth) -> Value {
    json!({"localPort": value.local_port, "remotePort": value.remote_port, "status": destination_status(value.status), "message": value.message, "checkedAt": value.checked_at})
}
pub(super) fn update_outcome(value: UpdateOutcome) -> Value {
    json!({"at": value.at, "from": value.from, "to": value.to, "error": value.error})
}
pub(super) fn build_relation(value: BuildRelation) -> &'static str {
    match value {
        BuildRelation::Unknown => "unknown",
        BuildRelation::Missing => "missing",
        BuildRelation::Bundled => "bundled",
        BuildRelation::Different => "different",
        BuildRelation::Older => "older",
        BuildRelation::Newer => "newer",
        BuildRelation::SameRelease => "sameRelease",
    }
}
pub(super) fn runtime_status(value: RuntimeStatus) -> &'static str {
    match value {
        RuntimeStatus::Unknown => "unknown",
        RuntimeStatus::Starting => "starting",
        RuntimeStatus::Ready => "ready",
        RuntimeStatus::Failed => "failed",
    }
}
pub(super) fn proxy(value: Proxy) -> Value {
    match value {
        Proxy::Command(value) => json!({"command":*value}),
        Proxy::Jump(value) => json!({"jump":*value}),
    }
}
fn permissions(value: newport_protocol::runtime::Permissions) -> Value {
    json!({"revision":value.revision,"clipboard":value.clipboard,"browser":value.browser})
}
pub(super) fn permissions_snapshot(value: PermissionsSnapshot) -> Value {
    json!({"desired":permissions(value.desired),"effective":permissions(value.effective),"error":value.error,"clipboardEpoch":value.clipboard_epoch,"browserEpoch":value.browser_epoch,"sessionEpoch":value.session_epoch})
}
pub(super) fn installation_status(value: InstallationStatus) -> Value {
    json!({"installed":value.installed,"updateAvailable":value.update_available,"buildRelation":build_relation(value.build_relation),"installedHash":value.installed_hash,"bundledHash":value.bundled_hash,"installedVersion":value.installed_version,"bundledVersion":value.bundled_version,"installedVariant":value.installed_variant.map(product),"bundledVariant":product(value.bundled_variant),"installedRunning":value.installed_running,"bundledStaged":value.bundled_staged,"checkedAt":value.checked_at,"error":value.error,"automaticUpdate":value.automatic_update.map(update_outcome)})
}
fn hello(value: newport_protocol::runtime::Hello) -> Value {
    let services: serde_json::Map<String, Value> = value
        .services
        .into_iter()
        .map(|(name, value)| {
            (
                name,
                json!({"versions":value.versions,"unavailable":value.unavailable}),
            )
        })
        .collect();
    let limits = value.limits;
    json!({"versions":value.versions,"instanceId":value.instance_id,"buildSha256":value.build_sha256,"workerSocket":value.worker_socket,"product":product(value.product),"services":services,"dependencies":value.dependencies,"limits":{"maxControlBytes":limits.max_control_bytes,"maxInflight":limits.max_inflight,"maxWorkers":limits.max_workers,"maxRegistrations":limits.max_registrations}})
}
pub(super) fn runtime_snapshot(value: RuntimeSnapshot) -> Value {
    json!({"status":runtime_status(value.status),"generation":value.generation,"hello":value.hello.map(hello),"error":value.error,"retryable":value.retryable,"failures":value.failures,"instances":value.instances.into_iter().map(runtime_instance).collect::<Vec<_>>()})
}
fn runtime(value: Runtime) -> Value {
    let agent_transport: serde_json::Map<String, Value> = value
        .agent_transport
        .into_iter()
        .map(|row| (row.id.to_string(), transport(row.value)))
        .collect();
    let agent_runtime: serde_json::Map<String, Value> = value
        .agent_runtime
        .into_iter()
        .map(|row| (row.id.to_string(), runtime_snapshot(row.value)))
        .collect();
    let agent_permissions: serde_json::Map<String, Value> = value
        .agent_permissions
        .into_iter()
        .map(|row| (row.id.to_string(), permissions_snapshot(row.value)))
        .collect();
    let agent_status: serde_json::Map<String, Value> = value
        .agent_status
        .into_iter()
        .map(|row| (row.id.to_string(), installation_status(row.value)))
        .collect();
    let agent_installation: serde_json::Map<String, Value> = value
        .agent_installation
        .into_iter()
        .map(|row| (row.id.to_string(), install_progress(row.value)))
        .collect();
    let tunnels: serde_json::Map<String, Value> = value
        .tunnels
        .into_iter()
        .map(|row| (row.id.to_string(), connection_state(row.value)))
        .collect();
    let clipboard: serde_json::Map<String, Value> = value
        .clipboard
        .into_iter()
        .map(|row| (row.id.to_string(), connection_state(row.value)))
        .collect();
    let tunnel_health: serde_json::Map<String, Value> = value
        .tunnel_health
        .into_iter()
        .map(|row| {
            (
                row.id.to_string(),
                Value::Array(
                    row.value
                        .values
                        .into_iter()
                        .map(destination_health)
                        .collect(),
                ),
            )
        })
        .collect();
    let clipboard_messages: serde_json::Map<String, Value> = value
        .clipboard_messages
        .into_iter()
        .map(|row| (row.id.to_string(), json!(row.value)))
        .collect();
    let connection_revisions: serde_json::Map<String, Value> = value
        .connection_revisions
        .into_iter()
        .map(|row| (row.id.to_string(), json!(row.value)))
        .collect();
    json!({"agentTransport":agent_transport,"agentRuntime":agent_runtime,"agentPermissions":agent_permissions,"agentStatus":agent_status,"agentInstallation":agent_installation,"tunnels":tunnels,"clipboard":clipboard,"tunnelHealth":tunnel_health,"clipboardMessages":clipboard_messages,"connectionRevisions":connection_revisions,"awaitingAgent":value.awaiting_agent})
}
pub(super) fn project(reply: Reply) -> Value {
    match reply {
        Reply::Snapshot(value) => snapshot(*value),
        Reply::ForwardTunnelBatch(value) => {
            Value::Array(value.tunnels.into_iter().map(tunnel).collect())
        }
        Reply::SaveServer(_)
        | Reply::DeleteServer(_)
        | Reply::ForwardDiscoveredPort(_)
        | Reply::SaveTunnel(_)
        | Reply::UpdateTunnel(_)
        | Reply::DeleteTunnel(_)
        | Reply::DeleteTunnels(_)
        | Reply::SetTunnelConnected(_)
        | Reply::SetClipboardEnabled(_)
        | Reply::SetIntegrationEnabled(_)
        | Reply::OpenTunnel(_)
        | Reply::OpenTerminalLink(_) => Value::Null,
    }
}
