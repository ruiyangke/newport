//! Command handlers behind the RPC routes; callers pass the services they use.
mod control;
mod terminal_links;
use crate::manager::{Shared, Snapshot};
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

pub async fn terminal_open(
    state: &Shared,
    terminals: &crate::terminal::Sessions,
    id: Uuid,
    session: Uuid,
    cols: u32,
    rows: u32,
) -> Result<crate::terminal::Io, String> {
    let owner = state.owner(id)?;
    let server = owner.server().clone();
    terminals.open_bound(session, server, cols, rows, owner)
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
