//! Explicit conversions between the backend's models and the desktop contract.
use super::failure::invalid_result;
use crate::model;
use newport_protocol::desktop::{Failure, configuration as dto};

pub(super) fn model_proxy(value: &dto::Proxy) -> model::Proxy {
    match value {
        dto::Proxy::Command(value) => model::Proxy::Command(value.as_ref().clone()),
        dto::Proxy::Jump(value) => model::Proxy::Jump(**value),
    }
}
pub(super) fn model_server(value: &dto::Server) -> model::Server {
    model::Server {
        id: value.id,
        name: value.name.clone(),
        ssh_user: value.ssh_user.clone(),
        ssh_host: value.ssh_host.clone(),
        ssh_port: value.ssh_port,
        identity_file: value.identity_file.clone(),
        agent_source: value.agent_source.clone(),
        agent_key_fingerprint: value.agent_key_fingerprint.clone(),
        auth_method: match value.auth_method {
            dto::AuthMethod::PublicKey => model::AuthMethod::PublicKey,
            dto::AuthMethod::Password => model::AuthMethod::Password,
        },
        clipboard_enabled: value.clipboard_enabled,
        browser_enabled: value.browser_enabled,
        agent_install_approved: value.agent_install_approved,
        setup_complete: value.setup_complete,
        proxy: value.proxy.as_ref().map(model_proxy),
    }
}
pub(super) fn connection_state(value: model::ConnectionState) -> dto::ConnectionState {
    dto::ConnectionState {
        status: match value.status {
            model::Status::Disconnected => dto::ConnectionStatus::Disconnected,
            model::Status::Connecting => dto::ConnectionStatus::Connecting,
            model::Status::Connected => dto::ConnectionStatus::Connected,
            model::Status::Reconnecting => dto::ConnectionStatus::Reconnecting,
            model::Status::Error => dto::ConnectionStatus::Error,
        },
        error_message: value.error_message,
        reconnect_attempt: value.reconnect_attempt,
    }
}
pub(super) fn destination_health(value: model::DestinationHealth) -> dto::DestinationHealth {
    dto::DestinationHealth {
        local_port: value.local_port,
        remote_port: value.remote_port,
        status: match value.status {
            model::DestinationStatus::Checking => dto::DestinationStatus::Checking,
            model::DestinationStatus::Reachable => dto::DestinationStatus::Reachable,
            model::DestinationStatus::Unavailable => dto::DestinationStatus::Unavailable,
            model::DestinationStatus::Blocked => dto::DestinationStatus::Blocked,
            model::DestinationStatus::Unknown => dto::DestinationStatus::Unknown,
        },
        message: value.message,
        checked_at: value.checked_at,
    }
}
pub(crate) fn update_outcome(value: crate::agent_lifecycle::UpdateOutcome) -> dto::UpdateOutcome {
    dto::UpdateOutcome {
        at: value.at,
        from: value.from,
        to: value.to,
        error: value.error,
    }
}
pub(crate) fn build_relation(value: crate::agent_lifecycle::BuildRelation) -> dto::BuildRelation {
    use crate::agent_lifecycle::BuildRelation as Relation;
    match value {
        Relation::Unknown => dto::BuildRelation::Unknown,
        Relation::Missing => dto::BuildRelation::Missing,
        Relation::Bundled => dto::BuildRelation::Bundled,
        Relation::Different => dto::BuildRelation::Different,
        Relation::Older => dto::BuildRelation::Older,
        Relation::Newer => dto::BuildRelation::Newer,
        Relation::SameRelease => dto::BuildRelation::SameRelease,
    }
}
pub(super) fn installation_status(
    value: crate::agent_lifecycle::Status,
) -> dto::InstallationStatus {
    dto::InstallationStatus {
        installed: value.installed,
        update_available: value.update_available,
        build_relation: build_relation(value.build_relation),
        installed_hash: value.installed_hash,
        bundled_hash: value.bundled_hash,
        installed_version: value.installed_version,
        bundled_version: value.bundled_version,
        installed_variant: value.installed_variant,
        bundled_variant: value.bundled_variant,
        installed_running: value.installed_running,
        bundled_staged: value.bundled_staged,
        checked_at: value.checked_at,
        error: value.error,
        automatic_update: value.automatic_update.map(update_outcome),
    }
}
pub(super) fn permissions_snapshot(
    value: crate::integration_permissions::Snapshot,
) -> dto::PermissionsSnapshot {
    dto::PermissionsSnapshot {
        desired: value.desired,
        effective: value.effective,
        error: value.error,
        clipboard_epoch: value.clipboard_epoch,
        browser_epoch: value.browser_epoch,
        session_epoch: value.session_epoch,
    }
}
pub(super) fn runtime_instance(
    value: crate::agent_runtime::RuntimeInstance,
) -> Result<dto::RuntimeInstance, Failure> {
    Ok(dto::RuntimeInstance {
        instance_id: value
            .instance_id
            .parse()
            .map_err(|_| invalid_result("runtime instance"))?,
        build_sha256: value.build_sha256,
        generation: value.generation,
        retiring: value.retiring,
        draining: value.draining,
        connected: value.connected,
        registrations: u64::try_from(value.registrations)
            .map_err(|_| invalid_result("registrations"))?,
        worker_leases: u64::try_from(value.worker_leases)
            .map_err(|_| invalid_result("worker leases"))?,
    })
}
pub(super) fn transport(
    value: crate::connection_state::Transport,
) -> Result<dto::Transport, Failure> {
    Ok(dto::Transport {
        status: match value.status {
            "disconnected" => dto::TransportStatus::Disconnected,
            "connecting" => dto::TransportStatus::Connecting,
            "connected" => dto::TransportStatus::Connected,
            "recovering" => dto::TransportStatus::Recovering,
            "failed" => dto::TransportStatus::Failed,
            _ => return Err(invalid_result("Transport status")),
        },
        generation: value.generation,
        waiting: u64::try_from(value.waiting).map_err(|_| invalid_result("waiting count"))?,
    })
}
pub(super) fn runtime_snapshot(
    value: crate::agent_runtime::Snapshot,
) -> Result<dto::RuntimeSnapshot, Failure> {
    Ok(dto::RuntimeSnapshot {
        status: match value.status {
            "unknown" => dto::RuntimeStatus::Unknown,
            "starting" => dto::RuntimeStatus::Starting,
            "ready" => dto::RuntimeStatus::Ready,
            "failed" => dto::RuntimeStatus::Failed,
            _ => return Err(invalid_result("RuntimeSnapshot status")),
        },
        generation: value.generation,
        hello: value.hello,
        error: value.error,
        retryable: value.retryable,
        failures: value.failures,
        instances: value
            .instances
            .into_iter()
            .map(runtime_instance)
            .collect::<Result<_, _>>()?,
    })
}
fn install_stage(value: &str) -> Result<dto::InstallStage, Failure> {
    Ok(match value {
        "preparing" => dto::InstallStage::Preparing,
        "uploading" => dto::InstallStage::Uploading,
        "installing" => dto::InstallStage::Installing,
        "verifying" => dto::InstallStage::Verifying,
        "activating" => dto::InstallStage::Activating,
        "ready" => dto::InstallStage::Ready,
        "error" => dto::InstallStage::Error,
        _ => return Err(invalid_result("install stage")),
    })
}
pub(super) fn install_progress(
    value: crate::agent_install::Progress,
) -> Result<dto::InstallProgress, Failure> {
    Ok(dto::InstallProgress {
        stage: install_stage(value.stage)?,
        sent: u64::try_from(value.sent).map_err(|_| invalid_result("sent bytes"))?,
        total: u64::try_from(value.total).map_err(|_| invalid_result("total bytes"))?,
        error: value.error,
        failed_stage: value
            .failed_stage
            .as_deref()
            .map(install_stage)
            .transpose()?,
        bytes_per_second: value.bytes_per_second,
    })
}
pub(super) fn snapshot(value: crate::manager::Snapshot) -> Result<dto::Snapshot, Failure> {
    let runtime = value.runtime;
    Ok(dto::Snapshot {
        instance_id: value.instance_id,
        revision: value.revision,
        config: dto::Config {
            servers: value.config.servers.iter().map(dto::Server::from).collect(),
            tunnels: value.config.tunnels.iter().map(dto::Tunnel::from).collect(),
        },
        load_error: value.load_error,
        runtime: dto::Runtime {
            agent_transport: runtime
                .agent_transport
                .into_iter()
                .map(|(id, value)| {
                    Ok(dto::TransportRow {
                        id,
                        value: transport(value)?,
                    })
                })
                .collect::<Result<_, Failure>>()?,
            agent_runtime: runtime
                .agent_runtime
                .into_iter()
                .map(|(id, value)| {
                    Ok(dto::RuntimeRow {
                        id,
                        value: runtime_snapshot(value)?,
                    })
                })
                .collect::<Result<_, Failure>>()?,
            agent_permissions: runtime
                .agent_permissions
                .into_iter()
                .map(|(id, value)| dto::PermissionsRow {
                    id,
                    value: permissions_snapshot(value),
                })
                .collect(),
            agent_status: runtime
                .agent_status
                .into_iter()
                .map(|(id, value)| dto::InstallationRow {
                    id,
                    value: installation_status(value),
                })
                .collect(),
            agent_installation: runtime
                .agent_installation
                .into_iter()
                .map(|(id, value)| {
                    Ok(dto::InstallRow {
                        id,
                        value: install_progress(value)?,
                    })
                })
                .collect::<Result<_, Failure>>()?,
            tunnels: runtime
                .tunnels
                .into_iter()
                .map(|(id, value)| dto::ConnectionRow {
                    id,
                    value: connection_state(value),
                })
                .collect(),
            clipboard: runtime
                .clipboard
                .into_iter()
                .map(|(id, value)| dto::ConnectionRow {
                    id,
                    value: connection_state(value),
                })
                .collect(),
            tunnel_health: runtime
                .tunnel_health
                .into_iter()
                .map(|(id, values)| dto::HealthRow {
                    id,
                    value: dto::DestinationHealthList {
                        values: values.into_iter().map(destination_health).collect(),
                    },
                })
                .collect(),
            clipboard_messages: runtime
                .clipboard_messages
                .into_iter()
                .map(|(id, value)| dto::MessageRow { id, value })
                .collect(),
            connection_revisions: runtime
                .connection_revisions
                .into_iter()
                .map(|(id, value)| dto::RevisionRow { id, value })
                .collect(),
            awaiting_agent: runtime.awaiting_agent,
        },
    })
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    /// Each read of the same state must encode the same rows, so the desktop
    /// can tell a refresh that changed nothing from one that did.
    #[test]
    fn snapshot_rows_come_in_server_order() {
        let mut ids: Vec<Uuid> = (0..8).map(|_| Uuid::new_v4()).collect();
        let mut runtime = crate::manager::Runtime::default();
        for (revision, id) in ids.iter().enumerate() {
            runtime.connection_revisions.insert(*id, revision as u64);
            runtime.clipboard_messages.insert(*id, "message".into());
        }
        let snapshot = super::snapshot(crate::manager::Snapshot {
            instance_id: Uuid::new_v4(),
            revision: 1,
            config: Default::default(),
            runtime,
            load_error: None,
        })
        .unwrap();
        ids.sort();
        let order = |rows: Vec<Uuid>| assert_eq!(rows, ids);
        order(
            snapshot
                .runtime
                .connection_revisions
                .iter()
                .map(|row| row.id)
                .collect(),
        );
        order(
            snapshot
                .runtime
                .clipboard_messages
                .iter()
                .map(|row| row.id)
                .collect(),
        );
    }
}
