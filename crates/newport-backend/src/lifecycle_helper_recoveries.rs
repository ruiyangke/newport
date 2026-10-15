//! Explicit adoption of remote helper evidence into durable local restoration intent.
use super::{Store, scope};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HelperRecoveryPhase {
    Restoring,
    Restored,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HelperRecovery {
    pub server_id: Uuid,
    pub connection: String,
    pub operation_id: Uuid,
    pub build: String,
    pub fingerprint: String,
    pub phase: HelperRecoveryPhase,
    pub error: Option<String>,
}

pub(super) fn require_available(
    rows: &[HelperRecovery],
    server: Uuid,
    connection: &str,
) -> Result<(), String> {
    if rows.iter().any(|r| {
        r.server_id == server
            && r.connection == connection
            && r.phase != HelperRecoveryPhase::Restored
    }) {
        return Err(
            "Finish the recorded helper restoration before changing the Needle installation."
                .into(),
        );
    }
    Ok(())
}

impl Store {
    pub fn helper_recovery(
        &self,
        server: &crate::model::Server,
        id: Uuid,
    ) -> Result<Option<HelperRecovery>, String> {
        let journal = self.read(server.id)?;
        match journal
            .helper_recoveries
            .iter()
            .find(|r| r.operation_id == id)
            .cloned()
        {
            Some(r) if r.server_id != server.id || r.connection != scope(server) => {
                Err("Helper restoration belongs to a different server connection".into())
            }
            row => Ok(row),
        }
    }

    /// Call only after explicit selection and fresh remote evidence validation.
    pub async fn begin_helper_recovery(
        &self,
        server: &crate::model::Server,
        id: Uuid,
        build: &str,
        fingerprint: &str,
    ) -> Result<HelperRecovery, String> {
        let server = server.clone();
        let build = build.to_owned();
        let fingerprint = fingerprint.to_owned();
        self.commit(server.id, move |journal| {
            let connection = scope(&server);
            if let Some(row) = journal
                .helper_recoveries
                .iter()
                .find(|r| r.operation_id == id)
            {
                if row.server_id != server.id
                    || row.connection != connection
                    || row.build != build
                    || row.fingerprint != fingerprint
                {
                    return Err(
                        "Helper restoration ID belongs to a different request or connection".into(),
                    );
                }
                return Ok(row.clone());
            }
            require_available(&journal.helper_recoveries, server.id, &connection)?;
            super::removals::require_available(&journal.removals, server.id, &connection)?;
            if journal.operations.iter().any(|r| {
                r.intent.operation_id == id
                    || r.server_id == server.id && r.connection == connection && !r.phase.terminal()
            }) || journal.removals.iter().any(|r| r.operation_id == id)
            {
                return Err(
                    "Resolve the recorded Needle operation before restoring standalone helpers"
                        .into(),
                );
            }
            let row = HelperRecovery {
                server_id: server.id,
                connection,
                operation_id: id,
                build,
                fingerprint,
                phase: HelperRecoveryPhase::Restoring,
                error: None,
            };
            journal.helper_recoveries.push(row.clone());
            Ok(row)
        })
        .await
    }

    pub async fn update_helper_recovery(
        &self,
        expected: &HelperRecovery,
        complete: bool,
        error: Option<String>,
    ) -> Result<(), String> {
        let expected = expected.clone();
        self.commit(expected.server_id, move |journal| {
            let row = journal
                .helper_recoveries
                .iter_mut()
                .find(|r| r.operation_id == expected.operation_id)
                .ok_or("Helper restoration not found")?;
            if row.server_id != expected.server_id
                || row.connection != expected.connection
                || row.build != expected.build
                || row.fingerprint != expected.fingerprint
            {
                return Err("Helper restoration identity changed".into());
            }
            if row.phase == HelperRecoveryPhase::Restored {
                return Ok(());
            }
            if complete {
                row.phase = HelperRecoveryPhase::Restored;
            }
            row.error = error.map(|e| e.chars().take(1000).collect());
            Ok(())
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn intent_survives_restart_excludes_other_lifecycle_actions_and_cannot_regress() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("journal.json");
        let store = Store::new(path.clone());
        let server = super::super::tests::server();
        let id = Uuid::new_v4();
        let row = store
            .begin_helper_recovery(&server, id, &"a".repeat(64), &"b".repeat(64))
            .await
            .unwrap();
        assert!(
            store
                .begin(super::super::tests::record(&server))
                .await
                .is_err()
        );
        assert!(
            store
                .begin_removal(&server, Uuid::new_v4(), &row.build)
                .await
                .is_err()
        );
        assert!(
            store
                .begin_helper_recovery(&server, Uuid::new_v4(), &row.build, &row.fingerprint)
                .await
                .is_err()
        );
        assert!(
            store.require_agent_available(&server).is_ok(),
            "helper restoration does not stop core services"
        );
        let store = Store::new(path);
        assert_eq!(
            store.helper_recovery(&server, id).unwrap(),
            Some(row.clone())
        );
        let mut changed = server.clone();
        changed.ssh_port += 1;
        assert!(store.helper_recovery(&changed, id).is_err());
        assert!(
            store
                .begin_helper_recovery(&server, id, &"c".repeat(64), &row.fingerprint)
                .await
                .is_err()
        );
        store
            .update_helper_recovery(&row, true, None)
            .await
            .unwrap();
        store
            .update_helper_recovery(&row, false, Some("late failure".into()))
            .await
            .unwrap();
        assert_eq!(
            store.helper_recovery(&server, id).unwrap().unwrap().phase,
            HelperRecoveryPhase::Restored
        );
        assert!(
            store
                .helper_recovery(&server, id)
                .unwrap()
                .unwrap()
                .error
                .is_none()
        );
        assert!(
            store
                .begin(super::super::tests::record(&server))
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn legacy_journal_reads_without_adopting_history_and_invalid_identity_is_rejected() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("journal.json");
        std::fs::write(&path, br#"{"version":1,"operations":[],"removals":[]}"#).unwrap();
        let store = Store::new(path);
        let server = super::super::tests::server();
        assert!(
            store
                .helper_recovery(&server, Uuid::new_v4())
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .begin_helper_recovery(&server, Uuid::nil(), &"a".repeat(64), &"b".repeat(64))
                .await
                .is_err()
        );
        assert!(
            store
                .begin_helper_recovery(&server, Uuid::new_v4(), &"a".repeat(64), "changed")
                .await
                .is_err()
        );
        assert!(
            store
                .begin(super::super::tests::record(&server))
                .await
                .is_ok()
        );
    }
}
