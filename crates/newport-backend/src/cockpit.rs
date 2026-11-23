use crate::{monitor, monitoring, server_session::ServerSession};
use serde::Deserialize;
use serde_json::Value;

/// What the cockpit can collect. The enum keeps the UI's argument shape.
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
    serde_json::to_value(monitoring::parse_overview(reading)?).map_err(|e| e.to_string())
}

/// An Overview reading whose processes match `search`.
pub async fn search(
    server: &std::sync::Arc<ServerSession>,
    search: String,
) -> Result<Value, String> {
    let reading = monitor::search(server, search).await?;
    serde_json::to_value(monitoring::parse_overview(reading)?).map_err(|e| e.to_string())
}
