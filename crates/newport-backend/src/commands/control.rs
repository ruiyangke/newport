//! Server lifecycle, forwarding and integration commands shared by application hosts.
use crate::{
    manager::Shared,
    model::{Server, Tunnel},
};
use uuid::Uuid;

pub async fn save_server(
    _files: &crate::files::Operations,
    terminals: &crate::terminal::Sessions,
    state: &Shared,
    _desktop: crate::desktop_integration::Desktop,
    server: Server,
    password: Option<String>,
) -> Result<(), String> {
    server.validate()?;
    let expected_owner = state.owner(server.id).ok();
    // Only UUID creation/reuse must wait for old local history removal. Editing
    // an existing owner must not stall its metrics writes during encryption.
    // Retirement checks below prevent a stale editor from recreating a deletion.
    let history = if expected_owner.is_none() {
        Some(crate::manager::server_history_guard(state, server.id).await)
    } else {
        None
    };
    let password = password
        .filter(|password| !password.is_empty())
        .map(|password| (server.id, zeroize::Zeroizing::new(password)));
    let password_changed = password.is_some();
    let terminals = terminals.clone();
    let cleanup = state
        .edit_configuration_with_commit(
            password,
            move |manager| {
                match &expected_owner {
                    Some(owner)
                        if !owner.is_revoked()
                            && manager
                                .sessions
                                .get(server.id)
                                .is_some_and(|current| std::sync::Arc::ptr_eq(&current, owner)) => {
                    }
                    None if !manager
                        .config
                        .servers
                        .iter()
                        .any(|saved| saved.id == server.id) => {}
                    _ => {
                        return Err(
                            "Server connection changed. Reopen its settings before saving.".into(),
                        );
                    }
                }
                let mut config = manager.config.clone();
                let known = config.servers.iter().find(|saved| saved.id == server.id);
                let reconnect =
                    password_changed || known.is_some_and(|saved| !saved.same_connection(&server));
                let mut saved = server.clone();
                // Editors cannot grant consent or overwrite preferences committed while
                // encryption was running. Derive those fields from the latest profile.
                saved.agent_install_approved =
                    known.is_some_and(|saved| !reconnect && saved.agent_install_approved);
                saved.setup_complete =
                    known.is_some_and(|saved| !reconnect && saved.setup_complete);
                saved.clipboard_enabled =
                    known.is_some_and(|saved| !reconnect && saved.clipboard_enabled);
                saved.browser_enabled =
                    known.is_some_and(|saved| !reconnect && saved.browser_enabled);
                let retired = reconnect.then(|| manager.sessions.get(server.id)).flatten();
                let active = if reconnect {
                    manager.active_tunnels(server.id)
                } else {
                    Vec::new()
                };
                // Consent has already been derived for this exact connection.
                // The generic editor helper preserves destination-only consent,
                // which would restore it after a credential-only replacement.
                if let Some(existing) = config.servers.iter_mut().find(|s| s.id == saved.id) {
                    *existing = saved;
                } else {
                    config.servers.push(saved);
                }
                Ok((config, (retired, active)))
            },
            move |manager, (retired, active)| {
                // Publication retires the old owner, cancelling its file operations.
                // Capture only that owner's tabs; new-owner work can already be running.
                let terminals = retired
                    .as_ref()
                    .map(|owner| terminals.begin_close_owner(owner));
                let tunnels: Vec<_> = active
                    .into_iter()
                    .filter_map(|id| manager.tunnel_cleanup(id))
                    .collect();
                drop(history);
                manager.track_profile_cleanup(async move {
                    if let Some(terminals) = terminals {
                        terminals.wait().await;
                    }
                    for tunnel in tunnels {
                        tunnel.wait().await;
                    }
                    Ok(())
                })
            },
        )
        .await?;
    cleanup.wait().await
}

pub async fn delete_server(
    _files: &crate::files::Operations,
    terminals: &crate::terminal::Sessions,
    state: &Shared,
    db: &crate::metrics_db::MetricsDb,
    id: Uuid,
) -> Result<(), String> {
    let history = crate::manager::server_history_guard(state, id).await;
    let db = db.clone();
    let terminals = terminals.clone();
    let cleanup = state
        .edit_configuration_with_commit(
            None,
            move |manager| {
                manager.server(id)?;
                let owner = manager.sessions.get(id).ok_or("Server no longer exists.")?;
                let mut config = manager.config.clone();
                let tunnels: Vec<_> = config
                    .tunnels
                    .iter()
                    .filter(|tunnel| tunnel.server_id == id)
                    .map(|tunnel| tunnel.id)
                    .collect();
                config.servers.retain(|server| server.id != id);
                config.tunnels.retain(|tunnel| tunnel.server_id != id);
                Ok((config, (owner, tunnels)))
            },
            move |manager, (retired, tunnels)| {
                let terminals = terminals.begin_close_owner(&retired);
                let tunnels: Vec<_> = tunnels
                    .into_iter()
                    .filter_map(|id| manager.tunnel_cleanup(id))
                    .collect();
                // Register cleanup in the commit worker, before a cancelled response can
                // release the server's history barrier or abandon its local-data removal.
                manager.track_profile_cleanup(async move {
                    let result = db.delete_server(id).await.map_err(|error| {
                        format!("Server removed, but local data cleanup failed: {error}")
                    });
                    drop(history);
                    terminals.wait().await;
                    for tunnel in tunnels {
                        tunnel.wait().await;
                    }
                    result
                })
            },
        )
        .await?;
    cleanup.wait().await
}

pub async fn save_tunnel(state: &Shared, tunnel: Tunnel) -> Result<(), String> {
    save_tunnel_inner(state, tunnel, None, false).await
}

/// An editor may remain open while another window changes or deletes its target.
/// Recheck the original definition at commit so an edit cannot recreate a
/// deleted forward or overwrite another editor's changes while encrypting.
pub async fn update_tunnel(state: &Shared, tunnel: Tunnel, expected: Tunnel) -> Result<(), String> {
    save_tunnel_inner(state, tunnel, Some(expected), false).await
}

/// Save first, then start only the committed definition and connection owner.
/// Connection errors retain the saved configuration; callers must not retry blindly.
pub async fn forward_port(state: &Shared, tunnel: Tunnel) -> Result<(), String> {
    save_tunnel_inner(state, tunnel, None, true).await
}

pub async fn forward_discovered_port(
    state: &Shared,
    server_id: Uuid,
    address: String,
    port: u16,
) -> Result<(), String> {
    if state.connect_discovered(server_id, &address, port)? {
        return Ok(());
    }
    let owner = state.owner(server_id)?;
    let proposed_id = Uuid::new_v4();
    let (tunnel, connection) = state
        .edit_configuration(None, move |manager| {
            validate_forward_owner(manager, &owner)?;
            let mut config = manager.config.clone();
            let mut tunnel = newport_domain::forwarding::discovered_tunnel(
                &config.tunnels,
                server_id,
                &address,
                port,
            )?;
            if !config.tunnels.iter().any(|saved| saved.id == tunnel.id) {
                // Plans are replayed at commit. Keep the requested identity stable.
                tunnel.id = proposed_id;
                config.tunnels.push(tunnel.clone());
            }
            Ok((config, (tunnel, manager.connection_scope(server_id)?)))
        })
        .await?;
    state
        .connect_tunnel_batch(server_id, &[tunnel], Some(&connection))
        .map(|_| ())
        .map_err(|error| format!("Forward saved, but connection failed: {error}"))
}

/// Rebase allocations against current configuration, persist once, and validate
/// the committed definitions again when admitting live listener owners.
pub async fn forward_tunnel_batch(
    state: &Shared,
    server_id: Uuid,
    create: Vec<Tunnel>,
    connect: Vec<Tunnel>,
    expected_connection: Option<serde_json::Value>,
) -> Result<Vec<Tunnel>, String> {
    if create.is_empty() {
        return state.connect_tunnel_batch(server_id, &connect, expected_connection.as_ref());
    }
    let owner = state.owner(server_id)?;
    let (tunnels, connection) = state
        .edit_configuration(None, move |m| {
            validate_forward_owner(m, &owner)?;
            validate_forward_connection(m, server_id, expected_connection.as_ref())?;
            if expected_connection.is_some() {
                for expected in &connect {
                    newport_domain::forwarding::validate_update(
                        &m.config.tunnels,
                        expected,
                        expected,
                    )?;
                }
            }
            let resolved = newport_domain::forwarding_batch::resolve_batch(
                server_id,
                &create,
                &connect,
                &m.config.tunnels,
            )?;
            let tunnels = resolved
                .connect
                .iter()
                .filter_map(|id| resolved.tunnels.iter().find(|t| t.id == *id).cloned())
                .collect::<Vec<_>>();
            let mut config = m.config.clone();
            config.tunnels = resolved.tunnels;
            Ok((config, (tunnels, m.connection_scope(server_id)?)))
        })
        .await?;
    state
        .connect_tunnel_batch(server_id, &tunnels, Some(&connection))
        .map_err(|error| format!("Forwards saved, but a connection could not start: {error}"))
}

pub(super) fn validate_forward_owner(
    manager: &crate::manager::Manager,
    owner: &std::sync::Arc<crate::server_session::ServerSession>,
) -> Result<(), String> {
    if owner.is_revoked()
        || !manager
            .sessions
            .get(owner.server().id)
            .is_some_and(|current| std::sync::Arc::ptr_eq(&current, owner))
    {
        return Err(
            "The server connection changed before the forwarding action could finish.".into(),
        );
    }
    Ok(())
}

/// Optional native handover precondition; validation and the following effect
/// are repeated at commit so a changed profile cannot receive a stale action.
pub(super) fn validate_forward_connection(
    manager: &crate::manager::Manager,
    server_id: Uuid,
    expected: Option<&serde_json::Value>,
) -> Result<(), String> {
    let Some(expected) = expected else {
        return Ok(());
    };
    if manager.load_error.is_some() {
        return Err("Server configuration is unavailable.".into());
    }
    if manager.connection_scope(server_id)? != *expected {
        return Err(
            "The server connection changed before the forwarding action could finish.".into(),
        );
    }
    Ok(())
}

async fn save_tunnel_inner(
    state: &Shared,
    tunnel: Tunnel,
    expected: Option<Tunnel>,
    connect: bool,
) -> Result<(), String> {
    tunnel.pairs()?;
    let server_id = tunnel.server_id;
    let owner = state.owner(server_id)?;
    let (saved, connection) = state
        .edit_configuration(None, move |m| {
            validate_forward_owner(m, &owner)?;
            m.server(tunnel.server_id)?;
            if let Some(expected) = &expected {
                newport_domain::forwarding::validate_update(&m.config.tunnels, expected, &tunnel)?;
            }
            tunnel.validate_unique(&m.config.tunnels)?;
            let mut config = m.config.clone();
            if let Some(t) = config.tunnels.iter_mut().find(|t| t.id == tunnel.id) {
                *t = tunnel.clone();
            } else {
                config.tunnels.push(tunnel.clone());
            }
            Ok((config, (tunnel.clone(), m.connection_scope(server_id)?)))
        })
        .await?;
    // Publication restarts only forwards still active at commit time. A stop
    // admitted during storage must not be undone by a stale pre-save snapshot.
    if connect {
        state
            .connect_tunnel_batch(server_id, &[saved], Some(&connection))
            .map_err(|error| format!("Forward saved, but connection failed: {error}"))?;
    }
    Ok(())
}
pub async fn delete_tunnel(state: &Shared, id: Uuid) -> Result<(), String> {
    let Some(expected) = state
        .snapshot()
        .config
        .tunnels
        .into_iter()
        .find(|t| t.id == id)
    else {
        return Ok(());
    };
    let owner = state.owner(expected.server_id)?;
    state
        .edit_configuration(None, move |m| {
            validate_forward_owner(m, &owner)?;
            newport_domain::forwarding::validate_update(&m.config.tunnels, &expected, &expected)?;
            let mut config = m.config.clone();
            config.tunnels.retain(|t| t.id != id);
            Ok((config, ()))
        })
        .await?;
    // Publication already retired this generation. Do not disconnect by ID
    // again: another command could have recreated it after our commit.
    if let Some(cleanup) = state.tunnel_cleanup(id) {
        cleanup.wait().await;
    }
    Ok(())
}
pub async fn set_tunnel_connected(state: &Shared, id: Uuid, connected: bool) -> Result<(), String> {
    if connected {
        state.connect_tunnel(id)
    } else {
        let cleanup = state.disconnect_tunnel(id);
        if let Some(cleanup) = cleanup {
            cleanup.wait().await;
        }
        Ok(())
    }
}
pub async fn set_clipboard_enabled(
    state: &Shared,
    desktop: crate::desktop_integration::Desktop,
    id: Uuid,
    enabled: bool,
    expected_connection: serde_json::Value,
    access: &crate::integration_permissions::Access,
) -> Result<(), String> {
    set_integration_enabled(
        state,
        desktop,
        id,
        IntegrationFeature::Clipboard,
        enabled,
        expected_connection,
        access,
    )
    .await
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IntegrationFeature {
    Clipboard,
    Browser,
}
pub async fn set_integration_enabled(
    state: &Shared,
    desktop: crate::desktop_integration::Desktop,
    id: Uuid,
    feature: IntegrationFeature,
    enabled: bool,
    expected_connection: serde_json::Value,
    access: &crate::integration_permissions::Access,
) -> Result<(), String> {
    // Revoke live access before waiting for a profile save or unrelated work.
    let (owner, intent) = access.begin(
        id,
        matches!(feature, IntegrationFeature::Browser),
        enabled,
        &expected_connection,
    )?;
    let captured = owner.clone();
    state.edit_configuration(None, move |manager| {
        if manager.connection_scope(id)? != expected_connection
            || captured.is_revoked()
            || !manager.sessions.get(id).is_some_and(|current| std::sync::Arc::ptr_eq(&current, &captured))
        {
            return Err("The server connection changed. Review its integrations and try again.".into());
        }
        if !captured.permissions.preference_is_current(intent) {
            return Err("Integration preference was superseded by a newer choice.".into());
        }
        let mut config = manager.config.clone();
        let server = config.servers.iter_mut().find(|server| server.id == id)
            .ok_or("Server no longer exists.")?;
        if intent.browser { server.browser_enabled = enabled; }
        else { server.clipboard_enabled = enabled; }
        Ok((config, ()))
    }).await.map_err(|error| {
        if enabled { error } else {
            format!("Access is disabled for this connection, but the preference could not be saved. Retry before restarting Newport: {error}")
        }
    })?;
    if owner.is_revoked() || !owner.permissions.commit_preference(intent) {
        return Err(
            "Integration preference was superseded by a newer choice or connection.".into(),
        );
    }
    if !enabled {
        // The permission gate already notified a live bridge. Turning off must
        // not wait for admission, reconnect a service, or fail because removal
        // temporarily blocks agent startup.
        return Ok(());
    }
    // An existing bridge applies the revision over its live channel. With both
    // permissions off it remains available for control; no SSH reconnect or install.
    let start = state.prepare_integration(id).await?;
    if !std::sync::Arc::ptr_eq(&start.owner, &owner) {
        return Err("The server connection changed. Review its integrations and try again.".into());
    }
    start.run(desktop).await
}

pub async fn test_connection(state: &Shared, id: Uuid) -> Result<(), String> {
    probe_connection(state, id, true).await
}

pub(crate) async fn probe_connection(state: &Shared, id: Uuid, retry: bool) -> Result<(), String> {
    let (server, owner) = state.current(id)?;
    let revision = owner.revision();
    let started = std::time::Instant::now();
    // Verify the transport used by the pages. A separate connection could
    // succeed while their shared transport was still awaiting agent consent.
    // Explicit tests may release backoff; background health probes must not.
    if retry {
        owner.retry_transport();
    }
    let result = async {
        let lease = owner.acquire(&server).await?;
        let text = lease.execute("printf ok", None).await?;
        if text.trim() == "ok" {
            Ok(())
        } else {
            Err("Unexpected connection test response".into())
        }
    }
    .await;
    state.observe_connection(&server, revision, started, result.clone());
    result
}
pub async fn discover_ports(
    state: &Shared,
    id: Uuid,
) -> Result<Vec<crate::ports::DiscoveredPort>, String> {
    let owner = crate::server_session::resolve(state, id).await?;
    crate::ports::query(&owner).refresh().await
}

/// Cheap cache observation. It never waits for SSH or another observer's fetch.
pub async fn connections_query(
    state: &Shared,
    id: Uuid,
    refresh: bool,
) -> Result<crate::query::Snapshot<Vec<crate::ports::DiscoveredPort>>, String> {
    let owner = crate::server_session::resolve(state, id).await?;
    if owner.is_revoked() {
        return Err("Server session closed.".into());
    }
    let query = crate::ports::query(&owner);
    // Retirement may race lazy initialization; do not create new demand on it.
    if owner.is_revoked() {
        query.close();
    }
    Ok(query.observe(refresh))
}
pub async fn reinstall_agent(
    state: &Shared,
    desktop: crate::desktop_integration::Desktop,
    id: Uuid,
) -> Result<(), String> {
    reinstall_agent_with_id(state, desktop, id, None).await
}
pub async fn reinstall_agent_with_id(
    state: &Shared,
    desktop: crate::desktop_integration::Desktop,
    id: Uuid,
    operation_id: Option<Uuid>,
) -> Result<(), String> {
    let (server, owner) = state.current(id)?;
    owner.retry_transport();
    // Healthy integrations keep their pinned runtime throughout staging and
    // activation. Deployment uses the shared managed lifecycle.
    let installed = async {
        let session = owner.acquire(&server).await?;
        // Updating the shared agent must not rewrite optional shell configuration.
        // Existing combined operations are resumed only through resume_agent.
        match operation_id {
            Some(id) => session.install_operation(true, false, id).await,
            None => session.install(true, false).await,
        }
        .map(|_| ())
    }
    .await;
    let resumed = if !owner.is_revoked() {
        let start = state.prepare_integration(id).await?;
        if std::sync::Arc::ptr_eq(&start.owner, &owner) {
            start.run(desktop).await
        } else {
            Ok(())
        }
    } else {
        Ok(())
    };
    installed.and(resumed)
}

pub async fn resume_agent(state: &Shared, id: Uuid, operation_id: Uuid) -> Result<String, String> {
    let (server, _) = state.current(id)?;
    let session = crate::server_session::acquire(state, &server).await?;
    session.resume_installation(operation_id).await
}

pub async fn recover_agent(state: &Shared, id: Uuid, operation_id: Uuid) -> Result<String, String> {
    let (server, _) = state.current(id)?;
    let session = crate::server_session::acquire(state, &server).await?;
    session.recover_installation(operation_id).await
}

pub async fn manage_agent_removal(
    state: &Shared,
    expected: Server,
    operation_id: Uuid,
    build: Option<String>,
) -> Result<String, String> {
    if operation_id.is_nil()
        || build
            .as_deref()
            .is_some_and(|b| !crate::agent_runtime::protocol::valid_build(b))
    {
        return Err("Invalid Needle removal request".into());
    }
    let (server, owner) = {
        let (server, owner) = state.current(expected.id)?;
        if !server.same_connection(&expected) {
            return Err("Server connection changed. Review Needle removal again.".into());
        }
        if build.is_none() {
            owner
                .lifecycle
                .receipts
                .removal(&server, operation_id)?
                .ok_or("Needle removal not found")?;
        }
        (server, owner)
    };
    owner.retry_transport();
    let session = owner.acquire(&server).await?;
    match build {
        Some(build) => session.remove_agent(operation_id, &build).await,
        None => session.restore_removed_agent(operation_id).await,
    }
}
pub async fn review_agent(
    state: &Shared,
    id: Uuid,
    operation_id: Uuid,
    keep: Option<(usize, String)>,
) -> Result<crate::lifecycle_receipts::protocol::HelperReview, String> {
    let (server, _) = state.current(id)?;
    let session = crate::server_session::acquire(state, &server).await?;
    match keep {
        None => session.review_helpers(operation_id).await,
        Some((index, fingerprint)) => session.keep_helper(operation_id, index, fingerprint).await,
    }
}

pub async fn restore_standalone_helpers(
    state: &Shared,
    expected: Server,
    operation_id: Uuid,
    build: String,
    fingerprint: String,
) -> Result<String, String> {
    let valid = crate::agent_runtime::protocol::valid_build;
    if operation_id.is_nil() || !valid(&build) || !valid(&fingerprint) {
        return Err("Invalid standalone helper restoration request".into());
    }
    let (server, owner) = {
        let (server, owner) = state.current(expected.id)?;
        if !server.same_connection(&expected) {
            return Err("Server connection changed. Review its helper history again.".into());
        }
        if let Some(record) = owner
            .lifecycle
            .receipts
            .helper_recovery(&server, operation_id)?
            && (record.build != build || record.fingerprint != fingerprint)
        {
            return Err("Helper restoration identity changed".into());
        }
        (server, owner)
    };
    owner.retry_transport();
    let lease = owner.acquire(&server).await?;
    lease
        .restore_standalone_helpers(operation_id, &build, &fingerprint)
        .await
}

pub async fn inspect_agent(
    state: &Shared,
    expected: Server,
) -> Result<crate::agent_lifecycle::Status, String> {
    let (server, owner) = {
        let (server, owner) = state.current(expected.id)?;
        if !server.same_connection(&expected) {
            return Err(
                "Server connection changed. Inspect Needle from the current server.".into(),
            );
        }
        (server, owner)
    };
    owner.retry_transport();
    owner.acquire(&server).await?.check_installation().await
}

pub async fn install_clipboard_helper(state: &Shared, id: Uuid) -> Result<String, String> {
    let (server, _) = state.current(id)?;
    let session = crate::server_session::acquire(state, &server).await?;

    session.configure_integrations().await
}
pub async fn open_tunnel(
    state: &Shared,
    desktop: crate::desktop_integration::Desktop,
    id: Uuid,
    remote_port: Option<u16>,
    expected: Option<Tunnel>,
    expected_connection: Option<serde_json::Value>,
) -> Result<(), String> {
    let endpoint = state.forward_open_endpoint(
        id,
        remote_port,
        expected.as_ref(),
        expected_connection.as_ref(),
    )?;
    let url = format!("http://{endpoint}");
    // The validated endpoint is now immutable. External opener work must not
    // hold up snapshots, configuration changes, or unrelated tunnel operations.
    tokio::task::spawn_blocking(move || desktop.open_url(&url))
        .await
        .map_err(|error| format!("Browser opening task failed: {error}"))?
        .map_err(|error| format!("Could not open the browser: {error}"))
}

#[cfg(test)]
fn forward_open_endpoint(
    manager: &crate::manager::Manager,
    id: Uuid,
    remote_port: Option<u16>,
    expected: Option<&Tunnel>,
    expected_connection: Option<&serde_json::Value>,
) -> Result<std::net::SocketAddrV4, String> {
    manager.forward_open_endpoint(id, remote_port, expected, expected_connection)
}

pub async fn open_url(
    desktop: crate::desktop_integration::Desktop,
    url: String,
) -> Result<(), String> {
    // Terminal output is untrusted; only a deliberate http(s) link is opened.
    let url = newport_domain::external_link::http_url(&url)
        .ok_or("Only http and https links can be opened.")?;
    tokio::task::spawn_blocking(move || desktop.open_url(&url))
        .await
        .map_err(|error| format!("Link opening task failed: {error}"))?
        .map_err(|error| format!("Could not open the link: {error}"))
}

#[cfg(test)]
mod forwarding_precondition_tests {
    use super::{forward_open_endpoint, validate_forward_connection};
    use crate::{
        manager::Manager,
        model::{ConnectionState, Status, Tunnel},
        profile::Store,
    };
    use serde_json::{Value, json};
    use uuid::Uuid;

    fn fixture() -> (tempfile::TempDir, Manager, Tunnel, Value) {
        let directory = tempfile::tempdir().unwrap();
        let mut manager = Manager::new(Store::for_test(directory.path().into()));
        let server = Uuid::new_v4();
        manager.config.servers.push(serde_json::from_value(json!({"id":server,"name":"Fixture","sshHost":"unconnected.invalid","sshUser":"fixture","sshPort":22})).unwrap());
        let mut tunnel =
            newport_domain::forwarding::discovered_tunnel(&[], server, "127.0.0.1", 3000).unwrap();
        tunnel.local_port = 9000;
        tunnel.local_port_end = Some(9005);
        tunnel.remote_port_end = Some(3005);
        manager.config.tunnels.push(tunnel.clone());
        manager
            .runtime
            .lock()
            .unwrap()
            .tunnels
            .insert(tunnel.id, ConnectionState::new(Status::Connected));
        let snapshot = serde_json::to_value(manager.snapshot()).unwrap();
        let connection = newport_domain::connection_scope::connection(
            &snapshot["config"]["servers"][0],
            &snapshot,
        );
        (directory, manager, tunnel, connection)
    }

    #[test]
    fn conditional_open_checks_exact_saved_tunnel_range_and_connected_runtime() {
        let (_directory, mut manager, tunnel, connection) = fixture();
        assert_eq!(
            forward_open_endpoint(
                &manager,
                tunnel.id,
                Some(3003),
                Some(&tunnel),
                Some(&connection)
            )
            .unwrap()
            .port(),
            9003
        );
        assert_eq!(
            forward_open_endpoint(&manager, tunnel.id, None, None, None)
                .unwrap()
                .port(),
            9000
        );
        assert!(
            forward_open_endpoint(
                &manager,
                tunnel.id,
                Some(3006),
                Some(&tunnel),
                Some(&connection)
            )
            .is_err()
        );
        manager.config.tunnels[0].local_port = 9100;
        manager.config.tunnels[0].local_port_end = Some(9105);
        assert!(
            forward_open_endpoint(
                &manager,
                tunnel.id,
                Some(3003),
                Some(&tunnel),
                Some(&connection)
            )
            .unwrap_err()
            .contains("changed")
        );
        manager.config.tunnels[0] = tunnel.clone();
        for status in [
            Status::Disconnected,
            Status::Connecting,
            Status::Reconnecting,
            Status::Error,
        ] {
            manager
                .runtime
                .lock()
                .unwrap()
                .tunnels
                .insert(tunnel.id, ConnectionState::new(status));
            assert!(
                forward_open_endpoint(
                    &manager,
                    tunnel.id,
                    Some(3003),
                    Some(&tunnel),
                    Some(&connection)
                )
                .is_err()
            );
        }
        manager.config.tunnels.clear();
        assert!(
            forward_open_endpoint(
                &manager,
                tunnel.id,
                Some(3003),
                Some(&tunnel),
                Some(&connection)
            )
            .unwrap_err()
            .contains("no longer exists")
        );
    }

    #[test]
    fn conditional_forward_and_open_reject_changed_connection_instance_and_profile_outage() {
        for change in 0..5 {
            let (directory, mut manager, tunnel, connection) = fixture();
            match change {
                0 => manager.advance_connection_revision(tunnel.server_id),
                1 => manager.config.servers[0].ssh_host = "changed.invalid".into(),
                2 => manager.config.servers.clear(),
                3 => manager.load_error = Some("fixture unavailable".into()),
                _ => {
                    let mut replacement = Manager::new(Store::for_test(directory.path().into()));
                    replacement.config = manager.config.clone();
                    manager = replacement;
                }
            }
            assert!(
                validate_forward_connection(&manager, tunnel.server_id, Some(&connection)).is_err(),
                "change {change}"
            );
            assert!(
                forward_open_endpoint(
                    &manager,
                    tunnel.id,
                    Some(3003),
                    Some(&tunnel),
                    Some(&connection)
                )
                .is_err()
            );
        }
    }

    #[test]
    fn presentation_only_changes_preserve_connection_and_omitted_preconditions_are_compatible() {
        let (_directory, mut manager, tunnel, connection) = fixture();
        manager.config.servers[0].name = "Renamed".into();
        manager.config.servers[0].browser_enabled = true;
        assert!(validate_forward_connection(&manager, tunnel.server_id, Some(&connection)).is_ok());
        assert!(validate_forward_connection(&manager, tunnel.server_id, None).is_ok());
        assert!(
            forward_open_endpoint(
                &manager,
                tunnel.id,
                Some(3003),
                Some(&tunnel),
                Some(&connection)
            )
            .is_ok()
        );
    }
}

#[cfg(test)]
mod connection_tests {
    use super::*;

    #[tokio::test]
    #[ignore = "Run with the SSH harness and its ephemeral fixture"]
    async fn connection_test_and_background_probe_use_the_pages_transport() {
        use crate::{
            manager::{Manager, share},
            model::Config,
            profile::Store,
        };
        let directory = tempfile::tempdir().unwrap();
        let server: Server = serde_json::from_value(serde_json::json!({
            "id": Uuid::new_v4(), "name":"Fixture", "sshHost":"127.0.0.1",
            "sshUser":"fixture", "sshPort": std::env::var("NEWPORT_TEST_SSH_PORT").unwrap().parse::<u16>().unwrap(),
            "identityFile":std::env::var("NEWPORT_TEST_IDENTITY").unwrap()
        })).unwrap();
        let mut manager = Manager::new(Store::for_test(directory.path().into()));
        manager
            .save(Config {
                servers: vec![server.clone()],
                ..Default::default()
            })
            .await
            .unwrap();
        let state = share(manager);
        let owner = state.owner(server.id).unwrap();
        assert_eq!(owner.transport_snapshot().generation, 0);
        test_connection(&state, server.id).await.unwrap();
        assert_eq!(owner.transport_snapshot().status, "connected");
        let generation = owner.transport_snapshot().generation;
        assert_eq!(generation, 1);
        let lease = owner.acquire(&server).await.unwrap();
        assert_eq!(lease.execute("printf ok", None).await.unwrap(), "ok");
        for _ in 0..3 {
            probe_connection(&state, server.id, false).await.unwrap();
            test_connection(&state, server.id).await.unwrap();
            assert_eq!(owner.transport_snapshot().generation, generation);
        }
        assert_eq!(state.snapshot().runtime.health[&server.id], "reachable");
        assert!(!state.snapshot().runtime.awaiting_agent.contains(&server.id));
        state.sessions.shutdown().await;
    }
}
