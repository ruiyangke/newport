//! Typed replies become render state only after the entire IPC exchange validates.
//! These explicit projections preserve presentation shapes without introducing
//! a generic JSON request or transport payload.
mod applications;
pub(crate) mod configuration;
mod connection;
mod metrics;
mod monitor_compose;
mod monitor_containers;
mod monitor_metrics;
mod preferences;
use crate::request_error::RequestError;
use newport_protocol::desktop::{self, Reply};
use serde_json::{Value, json};
fn product(value: newport_protocol::product::Variant) -> &'static str {
    match value {
        newport_protocol::product::Variant::Core => "core",
        newport_protocol::product::Variant::Full => "full",
    }
}
pub fn project(reply: Reply) -> Result<Value, RequestError> {
    Ok(match reply {
        value @ Reply::Account(_) => return crate::account::project_reply(value).map_err(Into::into),
        value @ Reply::Services(_) => return crate::services::project_reply(value).map_err(Into::into),
        Reply::Host(value) => match *value {
            desktop::host::Reply::Activate(_) | desktop::host::Reply::OpenRemoteApp(_) => Value::Null,
            value => return crate::settings::project_reply(Reply::Host(Box::new(value))).map_err(Into::into),
        },
        Reply::Configuration(value) => configuration::project(*value),
        Reply::Connection(value) => return connection::project(*value),
        Reply::Preferences(value) => preferences::project_reply(*value),
        Reply::Files(value) => return crate::file_result::project(*value).map_err(Into::into),
        Reply::Metrics(value) => metrics::project(*value),
        Reply::Containers(value) => monitor_containers::project(*value),
        Reply::Applications(value) => applications::project(*value),
        Reply::Terminal(value) => terminal(*value),
        Reply::Events(value) => match *value { desktop::events::Reply::Poll(value) => events(*value) },
        Reply::Git(value) => match *value {
            desktop::git::Reply::Request(_) => return Err("Git results require their original typed request for rendering".into()),
            desktop::git::Reply::RefreshRepository(value) => json!({"repoId":value.repo_id}),
            desktop::git::Reply::AcknowledgeOperation(_) | desktop::git::Reply::ReviewOperation(_) => Value::Null,
            desktop::git::Reply::PendingOperations(value) => Value::Array(value.receipts.into_iter().map(|value| {
                let state = match value.state { desktop::git::ReceiptState::Pending=>"pending",desktop::git::ReceiptState::Succeeded=>"succeeded",desktop::git::ReceiptState::Failed=>"failed",desktop::git::ReceiptState::Rejected=>"rejected",desktop::git::ReceiptState::NeedsResolution=>"needs_resolution",desktop::git::ReceiptState::OutcomeUnknown=>"outcome_unknown",desktop::git::ReceiptState::ReviewedUnknown=>"reviewed_unknown" };
                json!({"operationId":value.operation_id,"serverId":value.server,"action":value.action,"state":state})
            }).collect()),
        },
    })
}
fn terminal(reply: desktop::terminal::Reply) -> Value {
    use desktop::terminal::Reply;
    match reply {
        Reply::Close(_) => Value::Null,
        Reply::End(value) => json!({"ended":value.ended}),
        Reply::Rename(value) => json!({"renamed":value.renamed}),
        Reply::List(value) => {
            json!({"desktop":value.desktop,"holds":value.holds,"shells":value.shells.map(|value|value.shells.into_iter().map(|value|json!({"terminal":value.terminal,"name":value.name,"title":value.title,"creator":{"id":value.creator.id,"name":value.creator.name},"attached":value.attached.into_iter().map(|value|json!({"id":value.id,"name":value.name,"active":value.active})).collect::<Vec<_>>(),"started":value.started,"exited":value.exited})).collect::<Vec<_>>())})
        }
    }
}
fn events(value: desktop::events::Batch) -> Value {
    let entries: Vec<_> = value.entries.into_iter().map(|event| match event {
        desktop::events::Event::GitCommand(value) => json!({"kind":"git-command","serverId":value.server,"repoId":value.repo_id,"operationId":value.operation_id,"entry":{"command":value.entry.command,"durationMs":value.entry.duration_ms,"exitCode":value.entry.exit_code,"output":value.entry.output,"interrupted":value.entry.interrupted}}),
    }).collect();
    json!({"cursor":value.cursor,"entries":entries})
}
impl crate::backend::Backend {
    pub fn execute_rendered(&self, operation: desktop::Operation) -> Result<Value, RequestError> {
        let timeout = crate::backend::exchange::operation_timeout(&operation);
        self.execute_rendered_with_timeout(operation, timeout)
    }
    pub fn execute_rendered_with_timeout(
        &self,
        operation: desktop::Operation,
        timeout: std::time::Duration,
    ) -> Result<Value, RequestError> {
        match operation {
            desktop::Operation::Git(value) => match *value {
                desktop::git::Operation::Request(value) => self.request_git_with_timeout(
                    value.server,
                    value.request,
                    value.retry_connection,
                    timeout,
                ),
                value => project(
                    self.execute_with_timeout(desktop::Operation::Git(Box::new(value)), timeout)?,
                ),
            },
            operation => project(self.execute_with_timeout(operation, timeout)?),
        }
    }
}
