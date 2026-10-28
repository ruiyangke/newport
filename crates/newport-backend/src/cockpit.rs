use crate::{monitor, monitoring, server_session::ServerSession};
use serde::Deserialize;
use serde_json::Value;

/// What the cockpit can collect. Containers moved to their own commands
/// (containers.rs); the section stays so the UI's argument keeps its shape.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Section {
    Overview,
}

pub async fn collect(
    server: &std::sync::Arc<ServerSession>,
    section: Section,
) -> Result<Value, String> {
    match section {
        Section::Overview => overview(server).await,
    }
}

/// The server's agent reading, validated before it reaches the UI or sampler.
async fn overview(server: &std::sync::Arc<ServerSession>) -> Result<Value, String> {
    let reading = monitor::snapshot(server).await?;
    serde_json::to_value(monitoring::parse_overview(&reading.to_string())?)
        .map_err(|e| e.to_string())
}
