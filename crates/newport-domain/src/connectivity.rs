//! Authoritative server connectivity, shared by every application surface.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    #[default]
    Disconnected,
    Connecting,
    AwaitingApproval,
    Connected,
    Reconnecting,
    Failed,
}
impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Self::Disconnected => "Disconnected",
            Self::Connecting => "Connecting",
            Self::AwaitingApproval => "Waiting for SSH approval",
            Self::Connected => "Connected",
            Self::Reconnecting => "Reconnecting",
            Self::Failed => "Connection failed",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerState {
    pub connection_revision: u64,
    pub generation: u64,
    pub phase: Phase,
    pub attempt: u32,
    pub error: Option<String>,
    pub retryable: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub instance_id: Uuid,
    pub revision: u64,
    pub servers: BTreeMap<Uuid, ServerState>,
}
