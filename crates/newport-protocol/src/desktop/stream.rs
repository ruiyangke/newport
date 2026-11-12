//! Typed desktop stream contracts.
pub const MAX_COMMAND_BYTES: usize = 64 * 1024;
pub const MAX_REMOTE_ARGUMENTS: usize = 128;
pub const MAX_REMOTE_ARGUMENT_BYTES: usize = 16 * 1024;
pub const MAX_REMOTE_ARGUMENT_TOTAL_BYTES: usize = 64 * 1024;
pub const MAX_LSP_ROOT_BYTES: usize = 4096;
pub const MAX_LSP_SERVER_NAME_BYTES: usize = 40;

#[derive(Clone, Debug)]
pub struct TerminalOpenArgs {
    pub server: uuid::Uuid,
    pub session: uuid::Uuid,
    pub columns: u16,
    pub rows: u16,
    /// Defaults to false when absent on the wire.
    pub persist: bool,
    /// Defaults to false when absent on the wire.
    pub attach: bool,
    pub colors: Option<[[u8; 3]; 3]>,
}

#[derive(Clone, Debug)]
pub struct CommandOpenArgs {
    pub server: uuid::Uuid,
    pub command: String,
}

#[derive(Clone, Debug)]
pub struct RemoteAppsOpenArgs {
    pub server: uuid::Uuid,
    pub arguments: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct LspOpenArgs {
    pub server: uuid::Uuid,
    pub expected: crate::desktop::configuration::ConnectionScope,
    pub root: String,
    pub server_name: String,
    pub envrc: bool,
}

#[derive(Clone, Debug)]
pub struct TerminalOpened {
    pub version: u32,
    pub session: uuid::Uuid,
}

#[derive(Clone, Debug)]
pub struct CommandOpened {
    pub version: u32,
    pub server: uuid::Uuid,
}

#[derive(Clone, Debug)]
pub struct RemoteAppsOpened {
    pub version: u32,
    pub server: uuid::Uuid,
    pub service_version: u32,
}

#[derive(Clone, Debug)]
pub struct LspOpened {
    pub version: u32,
    pub server: uuid::Uuid,
}

#[derive(Clone, Debug)]
pub enum Opened {
    Terminal(Box<crate::desktop::stream::TerminalOpened>),
    Command(Box<crate::desktop::stream::CommandOpened>),
    RemoteApps(Box<crate::desktop::stream::RemoteAppsOpened>),
    Lsp(Box<crate::desktop::stream::LspOpened>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ending {
    Exited,
    Stopped,
    Refused,
    Lost,
}

#[derive(Clone, Debug)]
pub struct CommandCompletion {
    pub status: Option<u32>,
    pub signal: Option<String>,
    pub ending: crate::desktop::stream::Ending,
    pub stderr: bytes::Bytes,
    pub error: Option<crate::common::Error>,
}

#[derive(Clone, Debug)]
pub struct LspCompletion {
    pub error: Option<crate::common::Error>,
}

#[derive(Clone, Debug)]
pub enum Completion {
    Command(Box<crate::desktop::stream::CommandCompletion>),
    Lsp(Box<crate::desktop::stream::LspCompletion>),
}

#[derive(Clone, Debug)]
pub enum OpaqueClient {
    Data(Box<bytes::Bytes>),
    InputComplete(Box<crate::common::Present>),
}

#[derive(Clone, Debug)]
pub enum OpaqueServer {
    Data(Box<bytes::Bytes>),
    Completion(Box<crate::desktop::stream::Completion>),
}

#[derive(Clone, Debug)]
pub struct Input {
    pub epoch: u64,
    pub data: bytes::Bytes,
}

#[derive(Clone, Debug)]
pub struct Resize {
    pub columns: u16,
    pub rows: u16,
}

#[derive(Clone, Debug)]
pub struct Ack {
    pub bytes: u64,
}

#[derive(Clone, Debug)]
pub enum TerminalClient {
    Input(Box<crate::desktop::stream::Input>),
    Resize(Box<crate::desktop::stream::Resize>),
    Ack(Box<crate::desktop::stream::Ack>),
    End(Box<crate::common::Present>),
    Activate(Box<crate::common::Present>),
}

#[derive(Clone, Debug)]
pub struct Output {
    pub data: bytes::Bytes,
}

#[derive(Clone, Debug)]
pub struct Taken {
    pub bytes: u64,
}

#[derive(Clone, Debug)]
pub struct Restore {
    pub state: bytes::Bytes,
    pub prompted: bool,
}

#[derive(Clone, Debug)]
pub struct Reconnecting {
    pub attempt: u32,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct Exit {
    pub code: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct Active {
    pub active: bool,
}

/// A new tab's shell that Needle couldn't hold: an ordinary shell runs
/// instead, and ends when the connection does.
#[derive(Clone, Debug)]
pub struct NotHeld {
    pub reason: String,
}

#[derive(Clone, Debug)]
pub enum TerminalEvent {
    Ready(Box<crate::common::Present>),
    Persistent(Box<crate::common::Present>),
    Reconnecting(Box<crate::desktop::stream::Reconnecting>),
    Exit(Box<crate::desktop::stream::Exit>),
    Error(Box<crate::desktop::Failure>),
    Ended(Box<crate::common::Present>),
    Active(Box<crate::desktop::stream::Active>),
    DisplayLost(Box<crate::common::Present>),
    NotHeld(Box<crate::desktop::stream::NotHeld>),
}

#[derive(Clone, Debug)]
pub enum TerminalServer {
    Output(Box<crate::desktop::stream::Output>),
    Taken(Box<crate::desktop::stream::Taken>),
    Restore(Box<crate::desktop::stream::Restore>),
    Event(Box<crate::desktop::stream::TerminalEvent>),
}

#[derive(Clone, Debug)]
pub enum Operation {
    TerminalOpen(Box<crate::desktop::stream::TerminalOpenArgs>),
    CommandOpen(Box<crate::desktop::stream::CommandOpenArgs>),
    RemoteAppsOpen(Box<crate::desktop::stream::RemoteAppsOpenArgs>),
    LspOpen(Box<crate::desktop::stream::LspOpenArgs>),
}
