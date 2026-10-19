//! Shared command handlers. Desktop adapters supply explicit backend services.
mod control;
mod terminal_links;
use crate::manager::{Shared, Snapshot};
use crate::server_queries::{self, Read};
pub use control::*;
pub use terminal_links::open_terminal_link;
use uuid::Uuid;

pub async fn get_metrics_cache(
    db: &crate::metrics_db::MetricsDb,
) -> Result<crate::metrics_db::MetricsCache, String> {
    db.cache_info().await
}

pub async fn clear_metrics_cache(
    state: &Shared,
    db: &crate::metrics_db::MetricsDb,
) -> Result<crate::metrics_db::MetricsCache, String> {
    let _history = crate::manager::history_guard(state).await;
    let result = db.clear_cache().await;
    crate::metrics_sampler::clear_cache(state);
    result
}

pub async fn ssh_agent_keys() -> crate::agent_keys::KeyList {
    crate::agent_keys::list().await
}

pub async fn ssh_config_hosts() -> Result<crate::ssh_config_import::ConfigHosts, String> {
    tokio::task::spawn_blocking(crate::ssh_config_import::read)
        .await
        .map_err(|error| error.to_string())?
}

pub async fn cockpit_history(
    state: &Shared,
    db: &crate::metrics_db::MetricsDb,
    id: Uuid,
    minutes: Option<u32>,
) -> Result<Vec<crate::metrics_db::SavedSample>, String> {
    let (server, owner) = state.current(id)?;
    let result = db
        .history_range(&server, crate::metrics_db::now_ms(), minutes.unwrap_or(15))
        .await;
    if owner.is_revoked() {
        return Err("Server connection changed. Refresh again.".into());
    }
    result
}

pub async fn cockpit_collect(
    state: &Shared,
    db: &crate::metrics_db::MetricsDb,
    refresh: Option<bool>,
    id: Uuid,
    section: crate::cockpit::Section,
) -> Result<serde_json::Value, String> {
    match section {
        crate::cockpit::Section::Overview => {
            crate::metrics_sampler::read(state, db, id, refresh.unwrap_or(false)).await
        }
    }
}

pub async fn services_snapshot(state: &Shared, id: Uuid) -> Result<serde_json::Value, String> {
    let server = crate::server_session::resolve(state, id).await?;
    server_queries::read(&server, Read::Services, false).await
}

pub async fn service_logs(
    state: &Shared,
    id: Uuid,
    unit: Option<String>,
    scope: crate::services::Scope,
    priority: Option<crate::services::Priority>,
    cursor: Option<String>,
    lines: Option<u32>,
) -> Result<serde_json::Value, String> {
    let server = crate::server_session::resolve(state, id).await?;
    crate::services::logs(
        &server,
        unit.as_deref(),
        scope,
        priority,
        cursor.as_deref(),
        lines,
    )
    .await
}

pub async fn service_unit_file(
    state: &Shared,
    id: Uuid,
    unit: String,
    scope: crate::services::Scope,
) -> Result<serde_json::Value, String> {
    let server = crate::server_session::resolve(state, id).await?;
    server_queries::read(
        &server,
        Read::UnitFile {
            unit,
            user: scope == crate::services::Scope::User,
        },
        false,
    )
    .await
}

pub async fn service_action(
    state: &Shared,
    id: Uuid,
    unit: String,
    scope: crate::services::Scope,
    verb: crate::services::Verb,
) -> Result<serde_json::Value, String> {
    let server = crate::server_session::resolve(state, id).await?;
    let result = crate::services::action(&server, &unit, scope, verb).await;
    server.queries.invalidate_services();
    if let Some(query) = server.ports.get() {
        query.invalidate();
    }
    result
}

pub async fn containers_snapshot(state: &Shared, id: Uuid) -> Result<serde_json::Value, String> {
    let server = crate::server_session::resolve(state, id).await?;
    server_queries::read(&server, Read::Containers, false).await
}

pub async fn container_inspect(
    state: &Shared,
    id: Uuid,
    container: String,
) -> Result<serde_json::Value, String> {
    let server = crate::server_session::resolve(state, id).await?;
    server_queries::read(&server, Read::Inspect(container), false).await
}

pub async fn container_logs(
    state: &Shared,
    id: Uuid,
    container: Option<String>,
    project: Option<String>,
    cursor: Option<String>,
    lines: Option<u32>,
) -> Result<serde_json::Value, String> {
    let server = crate::server_session::resolve(state, id).await?;
    crate::containers::logs(
        &server,
        container.as_deref(),
        project.as_deref(),
        cursor.as_deref(),
        lines,
    )
    .await
}

pub async fn container_action(
    state: &Shared,
    id: Uuid,
    container: String,
    verb: crate::containers::ContainerVerb,
) -> Result<serde_json::Value, String> {
    let server = crate::server_session::resolve(state, id).await?;
    let result = crate::containers::action(&server, &container, verb).await;
    server.queries.invalidate_containers();
    if let Some(query) = server.ports.get() {
        query.invalidate();
    }
    result
}

pub async fn project_action(
    state: &Shared,
    id: Uuid,
    project: String,
    verb: crate::containers::ProjectVerb,
    expected_ids: Vec<String>,
) -> Result<serde_json::Value, String> {
    let server = crate::server_session::resolve(state, id).await?;
    let result = crate::containers::project_action(&server, &project, verb, &expected_ids).await;
    server.queries.invalidate_containers();
    if let Some(query) = server.ports.get() {
        query.invalidate();
    }
    result
}

pub async fn containers_remove(
    state: &Shared,
    id: Uuid,
    containers: Vec<String>,
) -> Result<serde_json::Value, String> {
    let server = crate::server_session::resolve(state, id).await?;
    let result = crate::containers::remove(&server, &containers).await;
    server.queries.invalidate_containers();
    if let Some(query) = server.ports.get() {
        query.invalidate();
    }
    result
}

pub async fn docker_disk(
    state: &Shared,
    id: Uuid,
    part: Option<String>,
) -> Result<serde_json::Value, String> {
    let part = server_queries::DiskPart::parse(part.as_deref())?;
    let server = crate::server_session::resolve(state, id).await?;
    server_queries::read(&server, Read::Disk(part), false).await
}

pub async fn docker_prune(
    state: &Shared,
    id: Uuid,
    what: crate::containers::PruneTarget,
) -> Result<serde_json::Value, String> {
    let server = crate::server_session::resolve(state, id).await?;
    let result = crate::containers::prune(&server, what).await;
    server.queries.invalidate_containers();
    if let Some(query) = server.ports.get() {
        query.invalidate();
    }
    result
}

pub async fn compose_project(
    state: &Shared,
    id: Uuid,
    project: String,
) -> Result<serde_json::Value, String> {
    let server = crate::server_session::resolve(state, id).await?;
    server_queries::read(&server, Read::Compose(project), false).await
}

pub async fn compose_run(
    state: &Shared,
    id: Uuid,
    project: String,
    action: crate::compose::ComposeAction,
) -> Result<serde_json::Value, String> {
    let server = crate::server_session::resolve(state, id).await?;
    let result = crate::compose::run(&server, &project, &action).await;
    server.queries.invalidate_containers();
    if let Some(query) = server.ports.get() {
        query.invalidate();
    }
    result
}

pub async fn compose_job(
    state: &Shared,
    id: Uuid,
    job: String,
    offset: Option<u64>,
) -> Result<serde_json::Value, String> {
    let server = crate::server_session::resolve(state, id).await?;
    crate::compose::job(&server, &job, offset.unwrap_or(0)).await
}

pub async fn compose_cancel(
    state: &Shared,
    id: Uuid,
    job: String,
) -> Result<serde_json::Value, String> {
    let server = crate::server_session::resolve(state, id).await?;
    let result = crate::compose::cancel(&server, &job).await;
    server.queries.invalidate_containers();
    if let Some(query) = server.ports.get() {
        query.invalidate();
    }
    result
}

pub async fn compose_jobs(state: &Shared, id: Uuid) -> Result<serde_json::Value, String> {
    let server = crate::server_session::resolve(state, id).await?;
    crate::compose::jobs(&server).await
}

pub async fn terminal_open(
    state: &Shared,
    terminals: &crate::terminal::Sessions,
    id: Uuid,
    session: Uuid,
    cols: u32,
    rows: u32,
) -> Result<(), String> {
    let owner = state.owner(id)?;
    let server = owner.server().clone();
    terminals.open_bound(session, server, cols, rows, owner)
}

pub async fn terminal_read(
    terminals: &crate::terminal::Sessions,
    session: Uuid,
) -> Result<Option<crate::terminal::Event>, String> {
    terminals.read(session).await
}

pub async fn terminal_write(
    terminals: &crate::terminal::Sessions,
    session: Uuid,
    data: Vec<u8>,
) -> Result<(), String> {
    if data.len() > 65536 {
        return Err("Terminal input is too large.".into());
    }
    terminals
        .send(session, crate::terminal::Input::Data(data))
        .await
}

pub async fn terminal_resize(
    terminals: &crate::terminal::Sessions,
    session: Uuid,
    cols: u32,
    rows: u32,
) -> Result<(), String> {
    crate::terminal::size(cols, rows)?;
    terminals
        .send(session, crate::terminal::Input::Resize(cols, rows))
        .await
}

pub async fn terminal_close(
    terminals: &crate::terminal::Sessions,
    session: Uuid,
) -> Result<(), String> {
    terminals.close(session).await;
    Ok(())
}

pub async fn snapshot(state: &Shared) -> Result<Snapshot, String> {
    Ok(state.snapshot())
}
