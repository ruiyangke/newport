//! Git RPC retains shared typed replies and durable mutation outcomes.
use super::failure::{self, operation};
use crate::{
    git::{ClientIdentity, LogSink, pending::PendingOperations},
    manager::Shared,
};
use newport_protocol::{
    common::{Present, Retry},
    desktop::{Failure, git as dto},
};

pub struct Services<'a> {
    pub identity: &'a dyn ClientIdentity,
    pub manager: &'a Shared,
    pub pending: &'a PendingOperations,
    pub events: LogSink,
}
fn error(value: newport_protocol::git::Error) -> Failure {
    // Git's shared contract allows only the non-replay policy "never".
    failure::failure(&value.code, value.message, Retry::No)
}
pub async fn dispatch(
    command: &dto::Operation,
    services: &Services<'_>,
) -> Result<dto::Reply, Failure> {
    use dto::{Operation as O, Reply as R};
    Ok(match command {
        O::Request(args) => {
            if args.retry_connection
                && !matches!(args.request, newport_protocol::git::Request::Open { .. })
            {
                return Err(operation(
                    "Connection retry is only supported when opening a checkout",
                ));
            }
            R::Request(Box::new(
                crate::git::git_request_typed(
                    services.events.clone(),
                    services.identity,
                    services.manager,
                    services.pending,
                    args.server,
                    args.request.clone(),
                    args.retry_connection,
                )
                .await
                .map_err(error)?,
            ))
        }
        O::RefreshRepository(args) => {
            crate::git::git_refresh_repository(services.manager, args.server, args.repo_id.clone())
                .await
                .map_err(error)?;
            R::RefreshRepository(Box::new(dto::Refreshed {
                repo_id: args.repo_id.clone(),
            }))
        }
        O::PendingOperations(args) => {
            let receipts =
                crate::git::pending::git_pending_operations(services.pending, args.server)
                    .await
                    .map_err(operation)?;
            R::PendingOperations(Box::new(dto::Receipts {
                receipts: receipts
                    .into_iter()
                    .map(|value| dto::Receipt {
                        operation_id: value.operation_id,
                        server: value.server_id,
                        action: value.action,
                        state: match value.state {
                            crate::git::pending::ReceiptState::Pending => {
                                dto::ReceiptState::Pending
                            }
                            crate::git::pending::ReceiptState::Succeeded => {
                                dto::ReceiptState::Succeeded
                            }
                            crate::git::pending::ReceiptState::Failed => dto::ReceiptState::Failed,
                            crate::git::pending::ReceiptState::Rejected => {
                                dto::ReceiptState::Rejected
                            }
                            crate::git::pending::ReceiptState::NeedsResolution => {
                                dto::ReceiptState::NeedsResolution
                            }
                            crate::git::pending::ReceiptState::OutcomeUnknown => {
                                dto::ReceiptState::OutcomeUnknown
                            }
                            crate::git::pending::ReceiptState::ReviewedUnknown => {
                                dto::ReceiptState::ReviewedUnknown
                            }
                        },
                    })
                    .collect(),
            }))
        }
        O::AcknowledgeOperation(args) => {
            crate::git::pending::git_acknowledge_operation(
                services.pending,
                args.server,
                args.operation_id,
            )
            .await
            .map_err(operation)?;
            R::AcknowledgeOperation(Box::new(Present {}))
        }
        O::ReviewOperation(args) => {
            crate::git::pending::git_review_operation(
                services.events.clone(),
                services.identity,
                services.manager,
                services.pending,
                args.server,
                args.operation_id,
            )
            .await
            .map_err(error)?;
            R::ReviewOperation(Box::new(Present {}))
        }
    })
}

#[cfg(test)]
mod tests;
