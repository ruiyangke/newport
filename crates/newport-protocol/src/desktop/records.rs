//! Typed desktop envelope contracts.
#[derive(Clone, Debug)]
pub struct Hello {
    pub version: u32,
    pub contract_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Ready {
    pub version: u32,
    pub backend_instance: uuid::Uuid,
    pub contract_id: Option<String>,
}

#[derive(Clone, Debug)]
pub enum FailureDetail {
    LicenseRefusal(Box<crate::desktop::account::RefusalReason>),
}

#[derive(Clone, Debug)]
pub struct Failure {
    pub error: crate::common::Error,
    pub detail: Option<crate::desktop::FailureDetail>,
}

#[derive(Clone, Debug)]
pub struct AgentApproval {
    pub server: uuid::Uuid,
    pub awaiting: bool,
}

#[derive(Clone, Debug)]
pub enum ProgressState {
    AgentApproval(Box<crate::desktop::AgentApproval>),
}

#[derive(Clone, Debug)]
pub struct Progress {
    pub id: crate::desktop::RequestId,
    pub state: crate::desktop::ProgressState,
}

#[derive(Clone, Debug)]
pub enum Operation {
    Account(Box<crate::desktop::account::Operation>),
    Connection(Box<crate::desktop::connection::Operation>),
    Configuration(Box<crate::desktop::configuration::Operation>),
    Files(Box<crate::desktop::files::Operation>),
    Metrics(Box<crate::desktop::metrics::Operation>),
    Services(Box<crate::desktop::services::Operation>),
    Containers(Box<crate::desktop::containers::Operation>),
    Preferences(Box<crate::desktop::preferences::Operation>),
    Git(Box<crate::desktop::git::Operation>),
    Applications(Box<crate::desktop::applications::Operation>),
    Host(Box<crate::desktop::host::Operation>),
    Events(Box<crate::desktop::events::Operation>),
    Terminal(Box<crate::desktop::terminal::Operation>),
    Stream(Box<crate::desktop::stream::Operation>),
}

#[derive(Clone, Debug)]
pub enum Reply {
    Account(Box<crate::desktop::account::Reply>),
    Connection(Box<crate::desktop::connection::Reply>),
    Configuration(Box<crate::desktop::configuration::Reply>),
    Files(Box<crate::desktop::files::Reply>),
    Metrics(Box<crate::desktop::metrics::Reply>),
    Services(Box<crate::desktop::services::Reply>),
    Containers(Box<crate::desktop::containers::Reply>),
    Preferences(Box<crate::desktop::preferences::Reply>),
    Git(Box<crate::desktop::git::Reply>),
    Applications(Box<crate::desktop::applications::Reply>),
    Host(Box<crate::desktop::host::Reply>),
    Events(Box<crate::desktop::events::Reply>),
    Terminal(Box<crate::desktop::terminal::Reply>),
}

#[derive(Clone, Debug)]
pub struct Request {
    pub id: crate::desktop::RequestId,
    pub operation: crate::desktop::Operation,
}

#[derive(Clone, Debug)]
pub enum Outcome {
    Completed(Box<crate::desktop::Reply>),
    Failed(Box<crate::desktop::Failure>),
    Opened(Box<crate::desktop::stream::Opened>),
}

#[derive(Clone, Debug)]
pub struct Response {
    pub id: crate::desktop::RequestId,
    pub outcome: crate::desktop::Outcome,
}

#[derive(Clone, Debug)]
pub enum ClientMessage {
    Hello(Box<crate::desktop::Hello>),
    Request(Box<crate::desktop::Request>),
}

#[derive(Clone, Debug)]
pub enum ServerMessage {
    Ready(Box<crate::desktop::Ready>),
    Refused(Box<crate::common::Error>),
    Progress(Box<crate::desktop::Progress>),
    Response(Box<crate::desktop::Response>),
}
