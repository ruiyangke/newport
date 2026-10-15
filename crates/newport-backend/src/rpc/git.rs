//! Native Git dispatch. Session ownership, journals and requests are shared with the desktop adapter.
use super::arguments::{argument, encode, error};
use crate::{
    git::{ClientIdentity, LogSink, Sessions, pending::PendingOperations},
    manager::Shared,
};
use serde::Deserialize;
use serde_json::Value;

#[cfg(test)]
mod tests;

pub struct Services<'a> {
    pub identity: &'a dyn ClientIdentity,
    pub manager: &'a Shared,
    pub sessions: &'a Sessions,
    pub pending: &'a PendingOperations,
    pub events: LogSink,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Command {
    GitConnect,
    GitRequest,
    GitRefreshRepository,
    GitRegisterRead,
    GitCancelRead,
    GitDisconnect,
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
            Self::GitConnect => encode(
                crate::git::git_connect(
                    services.identity,
                    services.manager,
                    argument(args, "serverId")?,
                )
                .await
                .map_err(error)?,
            ),
            Self::GitRequest => encode(
                crate::git::git_request(
                    services.events.clone(),
                    services.identity,
                    services.manager,
                    services.sessions,
                    services.pending,
                    argument(args, "serverId")?,
                    argument(args, "request")?,
                    argument(args, "readId")?,
                )
                .await
                .map_err(error)?,
            ),
            Self::GitRefreshRepository => crate::git::git_refresh_repository(
                services.manager,
                argument(args, "serverId")?,
                argument(args, "repoId")?,
            )
            .await
            .map_err(error),
            Self::GitRegisterRead => encode(
                crate::git::git_register_read(services.sessions, argument(args, "serverId")?)
                    .map_err(error)?,
            ),
            Self::GitCancelRead => {
                crate::git::git_cancel_read(
                    services.sessions,
                    argument(args, "serverId")?,
                    argument(args, "readId")?,
                );
                encode(())
            }
            Self::GitDisconnect => encode(
                crate::git::git_disconnect(services.manager, argument(args, "serverId")?)
                    .await
                    .map_err(error)?,
            ),
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
                    services.sessions,
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
