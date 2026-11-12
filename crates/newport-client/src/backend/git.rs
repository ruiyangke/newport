//! Typed Git calls; only their final rendering projection is a JSON value.
use super::Backend;
use crate::request_error::RequestError;
use newport_protocol::{
    desktop::{self, git as desktop_git},
    git,
};
use serde_json::{Value, json};
use std::time::Duration;

impl Backend {
    pub(super) fn start_git_action(
        &self,
        server: &str,
        repo: &str,
        snapshot: &str,
        action: git::Action,
        operation: uuid::Uuid,
    ) -> Result<crate::git_operation::Operation, crate::git_operation::WriteError> {
        let request = git::Request::Start {
            operation_id: operation.to_string(),
            repo_id: repo.into(),
            expected_snapshot: snapshot.into(),
            action,
        };
        self.dispatch_git_write(server, request, operation)
    }

    pub(super) fn dispatch_git_write(
        &self,
        server: &str,
        request: git::Request,
        operation: uuid::Uuid,
    ) -> Result<crate::git_operation::Operation, crate::git_operation::WriteError> {
        // Complete semantic validation before recording a pending operation.
        request.validate().map_err(|error| error.to_string())?;
        let server = uuid::Uuid::parse_str(server).map_err(|error| error.to_string())?;
        // The service envelope has its own budget, including protobuf overhead.
        let message = git::ClientMessage::Call(git::Call {
            id: operation.to_string(),
            request: Box::new(request),
        });
        use git::wire::Message as _;
        let frame = message.encode().map_err(|error| error.to_string())?;
        drop(frame);
        let git::ClientMessage::Call(call) = message else {
            return Err("Invalid Git call state.".to_owned().into());
        };
        let request = *call.request;
        self.execute(desktop::Operation::Git(Box::new(
            desktop_git::Operation::PendingOperations(Box::new(
                desktop_git::PendingOperationsArgs { server },
            )),
        )))
        .map_err(|error| error.to_string())?;
        let operation_request = desktop::Operation::Git(Box::new(desktop_git::Operation::Request(
            Box::new(desktop_git::RequestArgs {
                server,
                request,
                retry_connection: false,
            }),
        )));
        let (request, reply) = self
            .execute_retained_with_timeout(operation_request, Duration::from_secs(600))
            .map_err(|error| crate::git_operation::WriteError {
                message: error.to_string(),
                dispatched: error.dispatched,
            })?;
        let value =
            project_reply(request, reply).map_err(|error| crate::git_operation::WriteError {
                message: error.message,
                dispatched: true,
            })?;
        crate::git_operation::Operation::decode(value, &operation.to_string()).map_err(|message| {
            crate::git_operation::WriteError {
                message,
                dispatched: true,
            }
        })
    }

    pub fn request_git(
        &self,
        server: uuid::Uuid,
        request: git::Request,
        retry_connection: bool,
    ) -> Result<Value, RequestError> {
        self.request_git_with_timeout(server, request, retry_connection, Duration::from_secs(600))
    }
    pub fn request_git_with_timeout(
        &self,
        server: uuid::Uuid,
        request: git::Request,
        retry_connection: bool,
        timeout: Duration,
    ) -> Result<Value, RequestError> {
        let operation = desktop::Operation::Git(Box::new(desktop_git::Operation::Request(
            Box::new(desktop_git::RequestArgs {
                server,
                request,
                retry_connection,
            }),
        )));
        let (operation, reply) = self.execute_retained_with_timeout(operation, timeout)?;
        project_reply(operation, reply)
    }
}

fn project_reply(
    operation: desktop::Operation,
    reply: desktop::Reply,
) -> Result<Value, RequestError> {
    let desktop::Operation::Git(operation) = operation else {
        return Err("Invalid retained Git request".into());
    };
    let desktop_git::Operation::Request(request) = *operation else {
        return Err("Invalid retained Git request".into());
    };
    let desktop::Reply::Git(reply) = reply else {
        return Err("Invalid Git response family".into());
    };
    let desktop_git::Reply::Request(result) = *reply else {
        return Err("Invalid Git response operation".into());
    };
    let mut value = match *result {
        desktop_git::GitResult::Reply(reply) => {
            serde_json::to_value(reply.projection(&request.request)).map_err(|error| {
                RequestError {
                    message: error.to_string(),
                    code: Some("PROTOCOL_ERROR".into()),
                }
            })?
        }
        desktop_git::GitResult::Diff(value) => {
            json!({"snapshot":value.snapshot,"diff":value.diff})
        }
    };
    let validation = match &request.request {
        git::Request::DiffPage { cursor, .. } => {
            crate::git_diff_page::decode(&mut value, true, cursor.is_none())
        }
        git::Request::CommitDiffPage { cursor, .. } => {
            crate::git_diff_page::decode(&mut value, false, cursor.is_none())
        }
        git::Request::Status { .. }
        | git::Request::History { .. }
        | git::Request::CommitFiles { .. }
        | git::Request::Branches { .. }
        | git::Request::RemoteNames { .. }
        | git::Request::RemoteRefs { .. }
        | git::Request::Stashes { .. }
        | git::Request::Tags { .. }
        | git::Request::Worktrees { .. }
        | git::Request::BlobPage { .. } => crate::git_page::validate(&value),
        git::Request::Clone { .. }
        | git::Request::Init { .. }
        | git::Request::Tag { .. }
        | git::Request::Blob { .. }
        | git::Request::Remote { .. }
        | git::Request::Remotes { .. }
        | git::Request::Start { .. }
        | git::Request::Get { .. }
        | git::Request::Review { .. }
        | git::Request::Open { .. }
        | git::Request::Close { .. }
        | git::Request::StatusSummary { .. }
        | git::Request::Commit { .. }
        | git::Request::CommitDiff { .. }
        | git::Request::Diff { .. } => Ok(()),
    };
    validation.map_err(|message| RequestError {
        message,
        code: Some("PROTOCOL_ERROR".into()),
    })?;
    Ok(value)
}
