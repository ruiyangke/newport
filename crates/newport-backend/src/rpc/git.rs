//! Git dispatch. Session ownership, journals and requests live in `crate::git`.
use super::arguments::{argument, encode, error};
use crate::{
    git::{ClientIdentity, LogSink, pending::PendingOperations},
    manager::Shared,
};
use serde::Deserialize;
use serde_json::Value;

#[cfg(test)]
mod tests;

pub struct Services<'a> {
    pub identity: &'a dyn ClientIdentity,
    pub manager: &'a Shared,
    pub pending: &'a PendingOperations,
    pub events: LogSink,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Command {
    GitRequest,
    GitRefreshRepository,
    GitPendingOperations,
    GitAcknowledgeOperation,
    GitReviewOperation,
}
impl Command {
    pub fn parse(method: &str) -> Option<Self> {
        Self::deserialize(serde::de::value::StrDeserializer::<serde::de::value::Error>::new(method))
            .ok()
    }
    pub async fn dispatch(self, services: &Services<'_>, args: &Value) -> Result<Value, Value> {
        match self {
            Self::GitRequest => {
                let server_id = argument(args, "serverId")?;
                let request = argument(args, "request")?;
                let retry = argument::<Option<bool>>(args, "retryConnection")?.unwrap_or(false);
                if retry && !matches!(request, crate::git::protocol::Request::Open { .. }) {
                    return Err(error(
                        "Connection retry is only supported when opening a checkout",
                    ));
                }
                encode(
                    crate::git::git_request(
                        services.events.clone(),
                        services.identity,
                        services.manager,
                        services.pending,
                        server_id,
                        request,
                        retry,
                    )
                    .await
                    .map_err(error)?,
                )
            }
            Self::GitRefreshRepository => crate::git::git_refresh_repository(
                services.manager,
                argument(args, "serverId")?,
                argument(args, "repoId")?,
            )
            .await
            .map_err(error),
            Self::GitPendingOperations => encode(
                crate::git::pending::git_pending_operations(
                    services.pending,
                    argument(args, "serverId")?,
                )
                .await
                .map_err(error)?,
            ),
            Self::GitAcknowledgeOperation => encode(
                crate::git::pending::git_acknowledge_operation(
                    services.pending,
                    argument(args, "serverId")?,
                    argument(args, "operationId")?,
                )
                .await
                .map_err(error)?,
            ),
            Self::GitReviewOperation => encode(
                crate::git::pending::git_review_operation(
                    services.events.clone(),
                    services.identity,
                    services.manager,
                    services.pending,
                    argument(args, "serverId")?,
                    argument(args, "operationId")?,
                )
                .await
                .map_err(error)?,
            ),
        }
    }
}
