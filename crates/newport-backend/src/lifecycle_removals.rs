//! Durable local removal intent. Removed is deliberately not an admission-ready
//! terminal state: only a confirmed explicit restoration releases the barrier.
use super::{Store, scope};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RemovalPhase {
    Preparing,
    Removing,
    Removed,
    Restoring,
    Restored,
}
impl RemovalPhase {
    fn permits(self, next: Self) -> bool {
        self == next
            || matches!(
                (self, next),
                (Self::Preparing, Self::Removing | Self::Restoring)
                    | (Self::Removing, Self::Removed | Self::Restoring)
                    | (Self::Removed, Self::Restoring)
                    | (Self::Restoring, Self::Restored)
            )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Removal {
    pub server_id: Uuid,
    pub connection: String,
    pub operation_id: Uuid,
    pub build: String,
    pub phase: RemovalPhase,
    pub error: Option<String>,
}

pub(super) fn require_available(
    rows: &[Removal],
    server: Uuid,
    connection: &str,
) -> Result<(), String> {
    if rows.iter().any(|r| {
        r.server_id == server && r.connection == connection && r.phase != RemovalPhase::Restored
    }) {
        return Err("Needle removal is pending or completed. Restore the recorded removal before starting agent services or another installation.".into());
    }
    Ok(())
}

impl Store {
    pub fn operations_snapshot(
        &self,
        server: &crate::model::Server,
    ) -> Result<super::OperationsSnapshot, String> {
        let journal = self.read(server.id)?;
        let connection = scope(server);
        Ok(super::OperationsSnapshot {
            operations: journal
                .operations
                .iter()
                .filter(|r| r.server_id == server.id && r.connection == connection)
                .cloned()
                .collect(),
            removals: journal
                .removals
                .iter()
                .filter(|r| r.server_id == server.id && r.connection == connection)
                .cloned()
                .collect(),
            helper_recoveries: journal
                .helper_recoveries
                .iter()
                .filter(|r| r.server_id == server.id && r.connection == connection)
                .cloned()
                .collect(),
        })
    }

    pub fn removals_for_server(
        &self,
        server: &crate::model::Server,
    ) -> Result<Vec<Removal>, String> {
        let journal = self.read(server.id)?;
        let connection = scope(server);
        Ok(journal
            .removals
            .iter()
            .filter(|r| r.server_id == server.id && r.connection == connection)
            .cloned()
            .collect())
    }

    pub fn require_agent_available(&self, server: &crate::model::Server) -> Result<(), String> {
        let connection = scope(server);
        let (journal, pending) = {
            let owner = self.server(server.id)?;
            let published = owner.published.read().unwrap();
            (
                published.journal.clone()?,
                published
                    .pending_removals
                    .iter()
                    .any(|(id, key)| *id == server.id && *key == connection),
            )
        };
        if pending {
            return Err(
                "Needle removal intent is being saved. Agent services cannot start yet.".into(),
            );
        }
        require_available(&journal.removals, server.id, &connection)
    }

    pub fn removal(
        &self,
        server: &crate::model::Server,
        id: Uuid,
    ) -> Result<Option<Removal>, String> {
        let journal = self.read(server.id)?;
        match journal
            .removals
            .iter()
            .find(|r| r.operation_id == id)
            .cloned()
        {
            Some(r) if r.server_id != server.id || r.connection != scope(server) => {
                Err("Needle removal belongs to a different server connection".into())
            }
            row => Ok(row),
        }
    }

    /// Persist intent before stopping services or dispatching remote writes.
    pub async fn begin_removal(
        &self,
        server: &crate::model::Server,
        id: Uuid,
        build: &str,
    ) -> Result<Removal, String> {
        let server = server.clone();
        let build = build.to_owned();
        self.commit(server.id, move |journal| {
            let connection = scope(&server);
            super::helper_recoveries::require_available(
                &journal.helper_recoveries,
                server.id,
                &connection,
            )?;
            if let Some(row) = journal.removals.iter().find(|r| r.operation_id == id) {
                if row.server_id != server.id || row.connection != connection || row.build != build
                {
                    return Err(
                        "Needle removal ID belongs to a different request or connection".into(),
                    );
                }
                return Ok(row.clone());
            }
            if journal
                .operations
                .iter()
                .any(|r| r.intent.operation_id == id)
                || journal
                    .helper_recoveries
                    .iter()
                    .any(|r| r.operation_id == id)
            {
                return Err("Needle operation ID already belongs to an installation".into());
            }
            require_available(&journal.removals, server.id, &connection)?;
            if journal.operations.iter().any(|r| {
                r.server_id == server.id && r.connection == connection && !r.phase.terminal()
            }) {
                return Err("Resolve the existing Needle installation before removing it".into());
            }
            let row = Removal {
                server_id: server.id,
                connection,
                operation_id: id,
                build,
                phase: RemovalPhase::Preparing,
                error: None,
            };
            journal.removals.push(row.clone());
            Ok(row)
        })
        .await
    }

    /// Compare the prior phase so a late removal result cannot undo a restore.
    /// Repeating an already applied transition is an observation, not a rewrite.
    pub async fn update_removal(
        &self,
        server: &crate::model::Server,
        id: Uuid,
        expected: RemovalPhase,
        next: RemovalPhase,
        error: Option<String>,
    ) -> Result<Removal, String> {
        let server = server.clone();
        self.commit(server.id, move |journal| {
            let row = journal
                .removals
                .iter_mut()
                .find(|r| r.operation_id == id)
                .ok_or("Needle removal not found")?;
            if row.server_id != server.id || row.connection != scope(&server) {
                return Err("Needle removal belongs to a different server connection".into());
            }
            if !expected.permits(next) {
                return Err("Invalid Needle removal transition".into());
            }
            if row.phase == next && row.phase != expected {
                return Ok(row.clone());
            }
            if row.phase != expected {
                return Err("Needle removal state changed; reload its recorded outcome".into());
            }
            if row.phase == RemovalPhase::Restored {
                return Ok(row.clone());
            }
            row.phase = next;
            row.error = error.map(|s| s.chars().take(1000).collect());
            Ok(row.clone())
        })
        .await
    }
}
