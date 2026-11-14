//! Typed Git calls and the checks their replies pass before anyone reads them.
use super::Backend;
use crate::request_error::RequestError;
use newport_protocol::git::code;
use newport_protocol::{
    desktop::{self, git as desktop_git},
    git,
};
use std::time::Duration;

impl Backend {
    pub(super) fn start_git_action(
        &self,
        server: &str,
        repo: &str,
        snapshot: &str,
        action: git::Action,
        operation: uuid::Uuid,
    ) -> Result<git::Operation, crate::git_operation::WriteError> {
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
    ) -> Result<git::Operation, crate::git_operation::WriteError> {
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
        let reply = git_reply(request, reply)
            .and_then(|(request, result)| checked(&request, result))
            .map_err(|error| crate::git_operation::WriteError {
                message: error.message,
                dispatched: true,
            })?;
        crate::git_operation::confirmed(reply, &operation.to_string()).map_err(|message| {
            crate::git_operation::WriteError {
                message,
                dispatched: true,
            }
        })
    }

    /// Sends a Git read and returns its checked reply.
    pub fn git(
        &self,
        server: uuid::Uuid,
        request: git::Request,
        retry_connection: bool,
    ) -> Result<git::Reply, RequestError> {
        self.git_with_timeout(server, request, retry_connection, Duration::from_secs(600))
    }
    /// [`Self::git`] with its own reply budget.
    pub fn git_with_timeout(
        &self,
        server: uuid::Uuid,
        request: git::Request,
        retry_connection: bool,
        timeout: Duration,
    ) -> Result<git::Reply, RequestError> {
        let operation = desktop::Operation::Git(Box::new(desktop_git::Operation::Request(
            Box::new(desktop_git::RequestArgs {
                server,
                request,
                retry_connection,
            }),
        )));
        let (operation, reply) = self.execute_retained_with_timeout(operation, timeout)?;
        let (request, reply) = git_reply(operation, reply)?;
        checked(&request, reply)
    }
}

/// The Git request a retained operation carried, and the reply it received.
fn git_reply(
    operation: desktop::Operation,
    reply: desktop::Reply,
) -> Result<(git::Request, desktop_git::GitResult), RequestError> {
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
    Ok((request.request, *result))
}

/// Checks what the protocol's decoder cannot: page envelopes, and the order
/// and continuity of diff pages.
fn checked(
    request: &git::Request,
    result: desktop_git::GitResult,
) -> Result<git::Reply, RequestError> {
    let desktop_git::GitResult::Reply(reply) = result else {
        return Err(RequestError {
            message: "Invalid Git response operation".into(),
            code: Some(code::PROTOCOL_ERROR.into()),
        });
    };
    let reply = *reply;
    let validation = match (&reply, request) {
        (git::Reply::DiffPage(page), git::Request::DiffPage { cursor, .. }) => {
            crate::git_diff_page::validate(page, true, cursor.is_none())
        }
        (git::Reply::CommitDiffPage(page), git::Request::CommitDiffPage { cursor, .. }) => {
            crate::git_diff_page::validate(page, false, cursor.is_none())
        }
        (git::Reply::Status(page), _) => crate::git_page::validate_page(page),
        (git::Reply::History(page), _) => crate::git_page::validate_page(page),
        (git::Reply::CommitFiles(page), _) => crate::git_page::validate_page(page),
        (git::Reply::Branches(page), _) => crate::git_page::validate_page(page),
        (git::Reply::RemoteNames(page), _) => crate::git_page::validate_page(page),
        (git::Reply::RemoteRefs(page), _) => crate::git_page::validate_page(page),
        (git::Reply::Stashes(page), _) => crate::git_page::validate_page(page),
        (git::Reply::Tags(page), _) => crate::git_page::validate_page(page),
        (git::Reply::Worktrees(page), _) => crate::git_page::validate_page(page),
        (git::Reply::BlobPage(page), _) => crate::git_page::validate_page(page),
        _ => Ok(()),
    };
    validation.map_err(|message| RequestError {
        message,
        code: Some(code::PROTOCOL_ERROR.into()),
    })?;
    Ok(reply)
}
