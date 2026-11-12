//! Typed desktop events contracts.
#[derive(Clone, Debug)]
pub struct GitCommand {
    pub server: uuid::Uuid,
    pub repo_id: Option<String>,
    pub operation_id: Option<uuid::Uuid>,
    pub entry: crate::git::CommandLog,
}

#[derive(Clone, Debug)]
pub enum Event {
    GitCommand(Box<crate::desktop::events::GitCommand>),
}

#[derive(Clone, Debug)]
pub struct Batch {
    pub cursor: u64,
    pub entries: Vec<crate::desktop::events::Event>,
}

#[derive(Clone, Debug)]
pub struct PollArgs {
    pub after: u64,
}

#[derive(Clone, Debug)]
pub enum Operation {
    Poll(Box<crate::desktop::events::PollArgs>),
}

#[derive(Clone, Debug)]
pub enum Reply {
    Poll(Box<crate::desktop::events::Batch>),
}
