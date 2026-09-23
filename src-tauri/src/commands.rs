use crate::{
    manager::{Shared, Snapshot},
    model::{Server, Status, Tunnel},
    ssh,
};
use tauri::State;
use uuid::Uuid;

#[tauri::command]
pub(crate) async fn get_metrics_cache(
    db: State<'_, crate::metrics_db::MetricsDb>,
) -> Result<crate::metrics_db::MetricsCache, String> {
    db.cache_info().await
}
#[tauri::command]
pub(crate) async fn clear_metrics_cache(
    state: State<'_, Shared>,
    db: State<'_, crate::metrics_db::MetricsDb>,
    sampler: State<'_, crate::metrics_sampler::Sampler>,
) -> Result<crate::metrics_db::MetricsCache, String> {
    let _history = crate::manager::history_guard(&state).await;
    let result = db.clear_cache().await;
    sampler.clear_cache().await;
    result
}
#[tauri::command]
pub(crate) async fn ssh_agent_keys() -> crate::agent_keys::KeyList {
    crate::agent_keys::list().await
}
#[tauri::command]
pub(crate) async fn cockpit_history(
    state: State<'_, Shared>,
    db: State<'_, crate::metrics_db::MetricsDb>,
    id: Uuid,
) -> Result<Vec<crate::metrics_db::SavedSample>, String> {
    let server = state.lock().await.server(id)?;
    db.history(&server, crate::metrics_db::now_ms()).await
}
#[tauri::command]
pub(crate) async fn cockpit_collect(
    state: State<'_, Shared>,
    sampler: State<'_, crate::metrics_sampler::Sampler>,
    db: State<'_, crate::metrics_db::MetricsDb>,
    refresh: Option<bool>,
    id: Uuid,
    section: crate::cockpit::Section,
) -> Result<serde_json::Value, String> {
    let (server, revision) = {
        let manager = state.lock().await;
        (manager.server(id)?, manager.connection_revision(id))
    };
    if matches!(section, crate::cockpit::Section::Overview) {
        if refresh.unwrap_or(false) {
            sampler.sample(&state, &db, &server, revision).await
        } else {
            sampler.first_reading(&server, revision).await
        }
    } else {
        let started = std::time::Instant::now();
        let result = crate::cockpit::collect(&server, section).await;
        if result.is_ok() {
            state
                .lock()
                .await
                .observe_connection(&server, revision, started, Ok(()));
        }
        result
    }
}

#[tauri::command]
pub(crate) async fn cockpit_project_action(
    state: State<'_, Shared>,
    id: Uuid,
    project: String,
    action: crate::cockpit::ProjectAction,
    expected_ids: Vec<String>,
) -> Result<(), String> {
    let server = state.lock().await.server(id)?;
    crate::cockpit::project_action(&server, &project, action, expected_ids).await
}
#[tauri::command]
pub(crate) async fn cockpit_logs(
    state: State<'_, Shared>,
    id: Uuid,
    source: crate::cockpit::LogSource,
    target: String,
) -> Result<String, String> {
    let server = state.lock().await.server(id)?;
    crate::cockpit::logs(&server, source, &target).await
}
#[tauri::command]
pub(crate) async fn terminal_open(
    state: State<'_, Shared>,
    terminals: State<'_, crate::terminal::Sessions>,
    id: Uuid,
    session: Uuid,
    cols: u32,
    rows: u32,
) -> Result<(), String> {
    let manager = state.lock().await;
    terminals.open(session, manager.server(id)?, cols, rows)
}
#[tauri::command]
pub(crate) async fn terminal_read(
    terminals: State<'_, crate::terminal::Sessions>,
    session: Uuid,
) -> Result<Option<crate::terminal::Event>, String> {
    terminals.read(session).await
}
#[tauri::command]
pub(crate) fn terminal_write(
    terminals: State<'_, crate::terminal::Sessions>,
    session: Uuid,
    data: Vec<u8>,
) -> Result<(), String> {
    if data.len() > 65536 {
        return Err("Terminal input is too large.".into());
    }
    terminals.send(session, crate::terminal::Input::Data(data))
}
#[tauri::command]
pub(crate) fn terminal_resize(
    terminals: State<'_, crate::terminal::Sessions>,
    session: Uuid,
    cols: u32,
    rows: u32,
) -> Result<(), String> {
    crate::terminal::size(cols, rows)?;
    terminals.send(session, crate::terminal::Input::Resize(cols, rows))
}
#[tauri::command]
pub(crate) async fn terminal_close(
    terminals: State<'_, crate::terminal::Sessions>,
    session: Uuid,
) -> Result<(), String> {
    terminals.close(session).await;
    Ok(())
}
#[tauri::command]
pub(crate) async fn snapshot(state: State<'_, Shared>) -> Result<Snapshot, String> {
    Ok(state.lock().await.snapshot())
}
#[tauri::command]
pub(crate) async fn save_server(
    files: State<'_, crate::files::Operations>,
    terminals: State<'_, crate::terminal::Sessions>,
    state: State<'_, Shared>,
    app: tauri::AppHandle,
    server: Server,
    password: Option<String>,
) -> Result<(), String> {
    server.validate()?;
    let _history = crate::manager::history_guard(&state).await;
    let mut m = state.lock().await;
    let mut config = m.config.clone();
    let is_new = !m.config.servers.iter().any(|s| s.id == server.id);
    let reconnect = password.as_ref().is_some_and(|p| !p.is_empty())
        || m.config
            .servers
            .iter()
            .find(|s| s.id == server.id)
            .is_some_and(|s| !s.same_connection(&server));
    config.upsert_server(server.clone());
    m.save_with_password(
        config,
        password
            .filter(|p| !p.is_empty())
            .map(|p| (server.id, zeroize::Zeroizing::new(p))),
    )
    .await?;
    if reconnect || is_new {
        m.advance_connection_revision(server.id);
    }
    if reconnect {
        files.cancel_server(Some(server.id));
        terminals.close_server(Some(server.id)).await;
        let active = m.active_tunnels(server.id);
        m.stop_integration(server.id).await;
        for id in active {
            m.disconnect(id).await;
            m.connect(id).await?;
        }
        if m.server(server.id)?.clipboard_enabled || m.server(server.id)?.browser_enabled {
            m.start_integration(server.id, app).await?;
        }
        m.runtime.lock().unwrap().health.remove(&server.id);
    }
    Ok(())
}
#[tauri::command]
pub(crate) async fn delete_server(
    files: State<'_, crate::files::Operations>,
    terminals: State<'_, crate::terminal::Sessions>,
    state: State<'_, Shared>,
    db: State<'_, crate::metrics_db::MetricsDb>,
    id: Uuid,
) -> Result<(), String> {
    let _history = crate::manager::history_guard(&state).await;
    let mut m = state.lock().await;
    m.server(id)?;
    let db = db.inner().clone();
    let mut config = m.config.clone();
    config.servers.retain(|s| s.id != id);
    config.tunnels.retain(|t| t.server_id != id);
    let ids: Vec<_> = m
        .config
        .tunnels
        .iter()
        .filter(|t| t.server_id == id)
        .map(|t| t.id)
        .collect();
    m.save(config).await?;
    for tunnel in ids {
        m.disconnect(tunnel).await;
        m.runtime.lock().unwrap().tunnels.remove(&tunnel);
    }
    files.cancel_server(Some(id));
    terminals.close_server(Some(id)).await;
    m.stop_integration(id).await;
    m.runtime.lock().unwrap().clipboard.remove(&id);
    m.runtime.lock().unwrap().clipboard_messages.remove(&id);
    m.runtime.lock().unwrap().health.remove(&id);
    // Commit the profile change before irreversible cleanup. A failed profile save
    // must not delete its metric history or disconnect a still-saved server.
    // Keep the manager lock until cleanup ends so the UUID cannot be recreated
    // while its history/password is being removed.
    let cleanup = db.delete_server(id).await;
    cleanup.map_err(|e| format!("Server removed, but local data cleanup failed: {e}"))
}
#[tauri::command]
pub(crate) async fn save_tunnel(state: State<'_, Shared>, tunnel: Tunnel) -> Result<(), String> {
    tunnel.pairs()?;
    let mut m = state.lock().await;
    m.server(tunnel.server_id)?;
    tunnel.validate_unique(&m.config.tunnels)?;
    let active = m
        .runtime
        .lock()
        .unwrap()
        .tunnels
        .get(&tunnel.id)
        .is_some_and(|s| {
            matches!(
                s.status,
                Status::Connecting | Status::Connected | Status::Reconnecting
            )
        });
    let mut config = m.config.clone();
    if let Some(t) = config.tunnels.iter_mut().find(|t| t.id == tunnel.id) {
        *t = tunnel.clone();
    } else {
        config.tunnels.push(tunnel.clone());
    }
    m.save(config).await?;
    if active {
        m.disconnect(tunnel.id).await;
        m.connect(tunnel.id).await?;
    }
    Ok(())
}
#[tauri::command]
pub(crate) async fn delete_tunnel(state: State<'_, Shared>, id: Uuid) -> Result<(), String> {
    let mut m = state.lock().await;
    let mut c = m.config.clone();
    c.tunnels.retain(|t| t.id != id);
    m.save(c).await?;
    m.disconnect(id).await;
    m.runtime.lock().unwrap().tunnels.remove(&id);
    Ok(())
}
#[tauri::command]
pub(crate) async fn set_tunnel_connected(
    state: State<'_, Shared>,
    id: Uuid,
    connected: bool,
) -> Result<(), String> {
    let mut m = state.lock().await;
    if connected {
        m.connect(id).await
    } else {
        m.disconnect(id).await;
        Ok(())
    }
}
#[tauri::command]
pub(crate) async fn set_clipboard_enabled(
    state: State<'_, Shared>,
    app: tauri::AppHandle,
    id: Uuid,
    enabled: bool,
) -> Result<(), String> {
    set_integration_enabled(state, app, id, IntegrationFeature::Clipboard, enabled).await
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum IntegrationFeature {
    Clipboard,
    Browser,
}
#[tauri::command]
pub(crate) async fn set_integration_enabled(
    state: State<'_, Shared>,
    app: tauri::AppHandle,
    id: Uuid,
    feature: IntegrationFeature,
    enabled: bool,
) -> Result<(), String> {
    let mut m = state.lock().await;
    let before = m.server(id)?;
    match feature {
        IntegrationFeature::Clipboard => m.remember_clipboard(id, enabled).await?,
        IntegrationFeature::Browser => m.remember_integration(id, true, enabled).await?,
    }
    let after = m.server(id)?;
    if before.clipboard_enabled != after.clipboard_enabled
        || before.browser_enabled != after.browser_enabled
    {
        m.stop_integration(id).await;
    }
    if after.clipboard_enabled || after.browser_enabled {
        m.start_integration(id, app).await
    } else {
        m.stop_integration(id).await;
        Ok(())
    }
}

#[tauri::command]
pub(crate) async fn test_connection(state: State<'_, Shared>, id: Uuid) -> Result<(), String> {
    let (server, revision) = {
        let m = state.lock().await;
        (m.server(id)?, m.connection_revision(id))
    };
    let started = std::time::Instant::now();
    let result = ssh::execute(&server, "printf ok", None)
        .await
        .and_then(|text| {
            if text.trim() == "ok" {
                Ok(())
            } else {
                Err("Unexpected connection test response".into())
            }
        });
    state
        .lock()
        .await
        .observe_connection(&server, revision, started, result.clone());
    result
}
#[tauri::command]
pub(crate) async fn discover_ports(
    state: State<'_, Shared>,
    id: Uuid,
) -> Result<Vec<crate::ports::DiscoveredPort>, String> {
    let (server, revision) = {
        let m = state.lock().await;
        (m.server(id)?, m.connection_revision(id))
    };
    let started = std::time::Instant::now();
    let result = crate::ports::discover(&server).await;
    if result.is_ok() {
        state
            .lock()
            .await
            .observe_connection(&server, revision, started, Ok(()));
    }
    result
}
#[tauri::command]
pub(crate) async fn reinstall_agent(
    state: State<'_, Shared>,
    app: tauri::AppHandle,
    id: Uuid,
) -> Result<(), String> {
    let (server, revision) = {
        let mut manager = state.lock().await;
        let server = manager.server(id)?;
        let revision = manager.connection_revision(id);
        manager.stop_integration(id).await;
        (server, revision)
    };
    // Upload without holding the app-wide lock: other servers remain usable.
    let installed = async {
        let session = ssh::ExecSession::connect(&server).await?;
        let result = crate::agent::reinstall(&session).await;
        session.close().await;
        result.map(|_| ())
    }
    .await;
    let mut manager = state.lock().await;
    // Never revive a removed profile or reconnect using edited credentials.
    let resumed = if manager.matches_connection(&server, revision) {
        manager.start_integration(id, app).await
    } else {
        Ok(())
    };
    // Keep preferences and resume even after a failed upload; the atomic
    // installer leaves the previous binary available when verification fails.
    installed.and(resumed)
}

#[tauri::command]
pub(crate) async fn install_clipboard_helper(
    state: State<'_, Shared>,
    id: Uuid,
) -> Result<String, String> {
    let server = { state.lock().await.server(id)? };
    let session = ssh::ExecSession::connect(&server).await?;
    let result = crate::agent::install(&session).await;
    session.close().await;
    result
}
#[tauri::command]
pub(crate) async fn open_tunnel(
    state: State<'_, Shared>,
    app: tauri::AppHandle,
    id: Uuid,
) -> Result<(), String> {
    let m = state.lock().await;
    let t = m
        .config
        .tunnels
        .iter()
        .find(|t| t.id == id)
        .ok_or("Tunnel no longer exists")?;
    if !m
        .runtime
        .lock()
        .unwrap()
        .tunnels
        .get(&id)
        .is_some_and(|s| s.status == Status::Connected)
    {
        return Err("Connect the tunnel before opening it.".into());
    }
    let url = format!("http://127.0.0.1:{}", t.local_port);
    drop(m);
    use tauri_plugin_opener::OpenerExt;
    tokio::task::spawn_blocking(move || app.opener().open_url(url, None::<&str>))
        .await
        .map_err(|error| format!("Browser opening task failed: {error}"))?
        .map_err(|error| format!("Could not open the browser: {error}"))
}
