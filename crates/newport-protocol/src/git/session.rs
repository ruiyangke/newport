use super::{Binary, CommandLog, Error, Reply, Request};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Configure {
    pub id: String,
    pub client_id: String,
    pub client_version: String,
    pub command_logs: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Limits {
    pub max_frame_bytes: u64,
    pub max_chunk_bytes: u64,
    pub max_inflight: u32,
    pub max_page_items: u32,
    pub stream_window: u32,
    pub command_logs: bool,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_frame_bytes: super::MAX_FRAME as u64,
            max_chunk_bytes: super::CHUNK_SIZE as u64,
            max_inflight: 1,
            max_page_items: 200,
            stream_window: super::STREAM_WINDOW,
            command_logs: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Capabilities {
    pub methods: Vec<String>,
    pub object_formats: Vec<String>,
    pub features: Vec<String>,
    pub actions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ready {
    pub id: String,
    pub limits: Limits,
    pub capabilities: Capabilities,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    pub id: String,
    pub request: Box<Request>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub id: String,
    pub result: Result<Reply, Error>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Begin {
    pub id: String,
    pub stream_id: String,
    pub snapshot: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub id: String,
    pub stream_id: String,
    pub seq: u32,
    pub data: Binary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ack {
    pub stream_id: String,
    pub seq: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Log {
    pub id: String,
    pub entry: CommandLog,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientMessage {
    Configure(Configure),
    Call(Call),
    Ping(String),
    Ack(Ack),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerMessage {
    Ready(Ready),
    Response(Response),
    Pong(String),
    Begin(Begin),
    Chunk(Chunk),
    Log(Log),
    Failed(Error),
}
