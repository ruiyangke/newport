//! Local lifecycle intent, persisted before remote deployment and retained on interruption.
use serde::{Deserialize, Serialize};
#[cfg(test)]
use std::fs;
use std::sync::Arc;
use uuid::Uuid;
#[allow(dead_code)]
#[path = "../../../tools/agent/src/deployment_protocol.rs"]
pub mod protocol;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Record {
    pub server_id: Uuid,
    pub connection: String,
    pub intent: protocol::Intent,
    pub phase: Phase,
    pub error: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    Preparing,
    Uploading,
    Activating,
    Handoff,
    Ready,
    NeedsResolution,
    RollingBack,
    RolledBack,
}
impl Phase {
    pub fn terminal(self) -> bool {
        matches!(self, Self::Ready | Self::RolledBack)
    }
}
#[path = "lifecycle_removals.rs"]
mod removals;
pub use removals::{Removal, RemovalPhase};
#[path = "lifecycle_helper_recoveries.rs"]
mod helper_recoveries;
pub use helper_recoveries::{HelperRecovery, HelperRecoveryPhase};

pub struct OperationsSnapshot {
    pub operations: Vec<Record>,
    pub removals: Vec<Removal>,
    pub helper_recoveries: Vec<HelperRecovery>,
}

/// Each server journal covers deployments, removals and helper recovery.
/// Global version-one and array-only stores migrate on the first write.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Journal {
    version: u32,
    operations: Vec<Record>,
    removals: Vec<Removal>,
    #[serde(default)]
    helper_recoveries: Vec<HelperRecovery>,
}
impl Default for Journal {
    fn default() -> Self {
        Self {
            version: 1,
            operations: Vec::new(),
            removals: Vec::new(),
            helper_recoveries: Vec::new(),
        }
    }
}
#[derive(Deserialize)]
#[serde(untagged)]
enum Loaded {
    Versioned(Journal),
    Legacy(Vec<Record>),
}

#[path = "lifecycle_storage.rs"]
mod storage;
use storage::Storage;

/// Per-server lifecycle ownership; the registry holds no lock over storage I/O.
#[derive(Clone)]
pub struct Store {
    inner: Arc<Storage>,
}
impl Store {
    pub fn get(&self, id: Uuid) -> Result<Option<Record>, String> {
        let Some(server) = self.operation_server(id)? else {
            return Ok(None);
        };
        Ok(self
            .read(server)?
            .operations
            .iter()
            .find(|r| r.intent.operation_id == id)
            .cloned())
    }
    pub fn for_server(&self, server: &crate::model::Server) -> Result<Vec<Record>, String> {
        let connection = scope(server);
        Ok(self
            .read(server.id)?
            .operations
            .iter()
            .filter(|r| r.server_id == server.id && r.connection == connection)
            .cloned()
            .collect())
    }
    pub async fn begin(&self, record: Record) -> Result<Record, String> {
        self.commit(record.server_id, move |journal| {
            helper_recoveries::require_available(
                &journal.helper_recoveries,
                record.server_id,
                &record.connection,
            )?;
            let rows = &mut journal.operations;
            if let Some(r) = rows
                .iter()
                .find(|r| r.intent.operation_id == record.intent.operation_id)
            {
                if r.intent != record.intent
                    || r.connection != record.connection
                    || r.server_id != record.server_id
                {
                    return Err(
                        "Needle operation ID belongs to a different request or connection".into(),
                    );
                }
                return Ok(r.clone());
            }
            if journal
                .removals
                .iter()
                .any(|r| r.operation_id == record.intent.operation_id)
                || journal
                    .helper_recoveries
                    .iter()
                    .any(|r| r.operation_id == record.intent.operation_id)
            {
                return Err("Needle operation ID already belongs to a removal".into());
            }
            removals::require_available(&journal.removals, record.server_id, &record.connection)?;
            if rows.iter().any(|r| {
                r.server_id == record.server_id
                    && r.connection == record.connection
                    && !r.phase.terminal()
            }) {
                return Err(
                    "Resume the existing Needle operation before starting another installation"
                        .into(),
                );
            }
            if rows.len() + journal.removals.len() + journal.helper_recoveries.len() >= 1000 {
                return Err("Needle recovery journal is full".into());
            }
            rows.push(record.clone());
            Ok(record)
        })
        .await
    }
    pub async fn update(
        &self,
        id: Uuid,
        phase: Option<Phase>,
        error: Option<String>,
    ) -> Result<(), String> {
        let server = self
            .operation_server(id)?
            .ok_or("Needle operation not found")?;
        self.commit(server, move |journal| {
            let rows = &mut journal.operations;
            let row = rows
                .iter_mut()
                .find(|r| r.intent.operation_id == id)
                .ok_or("Needle operation not found")?;
            if !row.phase.terminal() {
                if let Some(phase) = phase {
                    row.phase = phase;
                }
                row.error = error.map(|s| s.chars().take(1000).collect());
            }
            Ok(())
        })
        .await
    }
}
impl Journal {
    fn identities(&self) -> impl Iterator<Item = (Uuid, Uuid)> + '_ {
        self.operations
            .iter()
            .map(|r| (r.intent.operation_id, r.server_id))
            .chain(self.removals.iter().map(|r| (r.operation_id, r.server_id)))
            .chain(
                self.helper_recoveries
                    .iter()
                    .map(|r| (r.operation_id, r.server_id)),
            )
    }
    fn for_owner(&self, server: Uuid) -> Self {
        Self {
            version: self.version,
            operations: self
                .operations
                .iter()
                .filter(|r| r.server_id == server)
                .cloned()
                .collect(),
            removals: self
                .removals
                .iter()
                .filter(|r| r.server_id == server)
                .cloned()
                .collect(),
            helper_recoveries: self
                .helper_recoveries
                .iter()
                .filter(|r| r.server_id == server)
                .cloned()
                .collect(),
        }
    }
    fn validate(&self) -> Result<(), String> {
        let mut ids = std::collections::HashSet::new();
        let valid = crate::agent_runtime::protocol::valid_build;
        if self.version != 1
            || self.operations.len() + self.removals.len() + self.helper_recoveries.len() > 1000
            || self.operations.iter().any(|r| {
                (!r.intent.kind.is_deployment()
                    && (!r.intent.integrations || r.intent.previous != r.intent.candidate))
                    || !ids.insert(r.intent.operation_id)
                    || r.intent.operation_id.is_nil()
                    || r.server_id.is_nil()
                    || !valid(&r.connection)
                    || !valid(&r.intent.candidate)
                    || (r.intent.previous != "missing" && !valid(&r.intent.previous))
                    || r.error.as_ref().is_some_and(|e| e.len() > 4096)
            })
            || self.removals.iter().any(|r| {
                !ids.insert(r.operation_id)
                    || r.operation_id.is_nil()
                    || r.server_id.is_nil()
                    || !valid(&r.connection)
                    || !valid(&r.build)
                    || r.error.as_ref().is_some_and(|e| e.len() > 4096)
            })
            || self.helper_recoveries.iter().any(|r| {
                !ids.insert(r.operation_id)
                    || r.operation_id.is_nil()
                    || r.server_id.is_nil()
                    || !valid(&r.connection)
                    || !valid(&r.build)
                    || !valid(&r.fingerprint)
                    || r.error.as_ref().is_some_and(|e| e.len() > 4096)
            })
        {
            return Err("Invalid Needle recovery journal".into());
        }
        let mut pending = std::collections::HashSet::new();
        for (id, connection) in self
            .operations
            .iter()
            .filter(|r| !r.phase.terminal())
            .map(|r| (r.server_id, &r.connection))
            .chain(
                self.removals
                    .iter()
                    .filter(|r| r.phase != RemovalPhase::Restored)
                    .map(|r| (r.server_id, &r.connection)),
            )
            .chain(
                self.helper_recoveries
                    .iter()
                    .filter(|r| r.phase != HelperRecoveryPhase::Restored)
                    .map(|r| (r.server_id, &r.connection)),
            )
        {
            if !pending.insert((id, connection)) {
                return Err("Conflicting Needle recovery intents".into());
            }
        }
        Ok(())
    }
}
pub fn scope(server: &crate::model::Server) -> String {
    use sha2::{Digest, Sha256};
    let identity = serde_json::json!([
        server.id,
        server.ssh_host,
        server.ssh_port,
        server.ssh_user,
        server.auth_method,
        server.identity_file,
        server.agent_source,
        server.agent_key_fingerprint
    ]);
    Sha256::digest(identity.to_string().as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
#[path = "lifecycle_storage_tests.rs"]
mod storage_tests;

#[cfg(test)]
mod tests {
    use super::*;
    pub(super) fn server() -> crate::model::Server {
        serde_json::from_value(serde_json::json!({"id":Uuid::new_v4(),"name":"fixture","sshHost":"127.0.0.1","sshPort":22,"sshUser":"fixture"})).unwrap()
    }
    pub(super) fn record(server: &crate::model::Server) -> Record {
        Record {
            server_id: server.id,
            connection: scope(server),
            phase: Phase::Preparing,
            error: None,
            intent: protocol::Intent {
                kind: protocol::Kind::Deployment,
                operation_id: Uuid::new_v4(),
                candidate: "1".repeat(64),
                previous: "missing".into(),
                integrations: false,
            },
        }
    }
    #[tokio::test]
    async fn intent_survives_restart_and_old_results_cannot_change_connection_or_regress_success() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("needle.json");
        let server = server();
        let record = record(&server);
        let id = record.intent.operation_id;
        let store = Store::new(path.clone());
        store.begin(record.clone()).await.unwrap();
        store
            .update(id, Some(Phase::Handoff), Some("connection lost".into()))
            .await
            .unwrap();
        drop(store);
        let store = Store::new(path);
        assert_eq!(store.get(id).unwrap().unwrap().phase, Phase::Handoff);
        assert!(
            store
                .begin(Record {
                    intent: protocol::Intent {
                        operation_id: Uuid::new_v4(),
                        ..record.intent.clone()
                    },
                    ..record.clone()
                })
                .await
                .is_err()
        );
        assert_eq!(
            store.begin(record.clone()).await.unwrap().phase,
            Phase::Handoff
        );
        let mut edited = server.clone();
        edited.ssh_host = "changed.example".into();
        assert!(store.for_server(&edited).unwrap().is_empty());
        assert!(
            store
                .begin(Record {
                    connection: scope(&edited),
                    ..record.clone()
                })
                .await
                .is_err()
        );
        store.update(id, Some(Phase::Ready), None).await.unwrap();
        store
            .update(
                id,
                Some(Phase::NeedsResolution),
                Some("late failure".into()),
            )
            .await
            .unwrap();
        assert_eq!(store.get(id).unwrap().unwrap().phase, Phase::Ready);
        assert!(store.get(id).unwrap().unwrap().error.is_none());
    }
    #[tokio::test]
    async fn rolled_back_operations_are_terminal_and_allow_a_new_intent() {
        let store = Store::default();
        let first = record(&server());
        let id = first.intent.operation_id;
        store.begin(first.clone()).await.unwrap();
        store
            .update(id, Some(Phase::RollingBack), None)
            .await
            .unwrap();
        let mut next = first.clone();
        next.intent.operation_id = Uuid::new_v4();
        assert!(store.begin(next.clone()).await.is_err());
        store
            .update(id, Some(Phase::RolledBack), None)
            .await
            .unwrap();
        store
            .update(id, Some(Phase::Handoff), Some("late completion".into()))
            .await
            .unwrap();
        assert_eq!(store.get(id).unwrap().unwrap().phase, Phase::RolledBack);
        assert!(store.get(id).unwrap().unwrap().error.is_none());
        store.begin(next).await.unwrap();
    }
    #[tokio::test]
    async fn corrupt_or_unwritable_receipts_block_deployment_without_erasing_evidence() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("needle.json");
        fs::write(&path, b"broken").unwrap();
        let store = Store::new(path.clone());
        assert!(store.begin(record(&server())).await.is_err());
        assert_eq!(fs::read(path).unwrap(), b"broken");
        let blocked = directory.path().join("not-a-directory");
        fs::write(&blocked, b"preserve").unwrap();
        let store = Store::new(blocked.join("journal"));
        assert!(store.begin(record(&server())).await.is_err());
        assert_eq!(fs::read(blocked).unwrap(), b"preserve");
    }

    #[tokio::test]
    async fn legacy_journal_migrates_atomically_and_removal_barrier_survives_restart() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("needle.json");
        let server = server();
        let mut installed = record(&server);
        installed.phase = Phase::Ready;
        let legacy = serde_json::to_vec(&vec![installed.clone()]).unwrap();
        fs::write(&path, &legacy).unwrap();
        let store = Store::new(path.clone());
        assert_eq!(store.for_server(&server).unwrap().len(), 1);
        assert_eq!(fs::read(&path).unwrap(), legacy, "reading must not migrate");
        let id = Uuid::new_v4();
        let build = "2".repeat(64);
        store.begin_removal(&server, id, &build).await.unwrap();
        store
            .update_removal(
                &server,
                id,
                RemovalPhase::Preparing,
                RemovalPhase::Removing,
                None,
            )
            .await
            .unwrap();
        store
            .update_removal(
                &server,
                id,
                RemovalPhase::Removing,
                RemovalPhase::Removed,
                None,
            )
            .await
            .unwrap();
        drop(store);
        let store = Store::new(path.clone());
        assert_eq!(
            store.removal(&server, id).unwrap().unwrap().phase,
            RemovalPhase::Removed
        );
        assert!(store.require_agent_available(&server).is_err());
        assert!(store.begin(record(&server)).await.is_err());
        assert_eq!(
            store
                .get(installed.intent.operation_id)
                .unwrap()
                .unwrap()
                .phase,
            Phase::Ready
        );
        let descriptor: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(descriptor["version"], 2);
        let saved: serde_json::Value =
            serde_json::from_slice(&fs::read(store.journal_path(server.id).unwrap()).unwrap())
                .unwrap();
        assert_eq!(saved["version"], 1);
        assert_eq!(saved["operations"].as_array().unwrap().len(), 1);
        store
            .update_removal(
                &server,
                id,
                RemovalPhase::Removed,
                RemovalPhase::Restoring,
                None,
            )
            .await
            .unwrap();
        assert!(store.require_agent_available(&server).is_err());
        store
            .update_removal(
                &server,
                id,
                RemovalPhase::Restoring,
                RemovalPhase::Restored,
                None,
            )
            .await
            .unwrap();
        store.require_agent_available(&server).unwrap();
        store.begin(record(&server)).await.unwrap();
        assert!(
            store
                .update_removal(
                    &server,
                    id,
                    RemovalPhase::Removing,
                    RemovalPhase::Removed,
                    None
                )
                .await
                .is_err()
        );
        assert_eq!(
            store
                .begin_removal(&server, id, &build)
                .await
                .unwrap()
                .phase,
            RemovalPhase::Restored
        );
    }

    #[tokio::test]
    async fn removal_ids_and_transitions_are_scoped_and_cannot_bypass_pending_installation() {
        let store = Store::default();
        let server = server();
        let install = record(&server);
        store.begin(install.clone()).await.unwrap();
        let id = Uuid::new_v4();
        let build = "3".repeat(64);
        assert!(store.begin_removal(&server, id, &build).await.is_err());
        store
            .update(install.intent.operation_id, Some(Phase::Ready), None)
            .await
            .unwrap();
        assert!(
            store
                .begin_removal(&server, install.intent.operation_id, &build)
                .await
                .is_err()
        );
        assert!(
            store
                .begin_removal(&server, Uuid::nil(), &build)
                .await
                .is_err()
        );
        assert!(store.begin_removal(&server, id, "bad hash").await.is_err());
        store.begin_removal(&server, id, &build).await.unwrap();
        assert!(
            store
                .begin_removal(&server, id, &"4".repeat(64))
                .await
                .is_err()
        );
        assert!(
            store
                .begin_removal(&server, Uuid::new_v4(), &build)
                .await
                .is_err()
        );
        let mut changed = server.clone();
        changed.ssh_port += 1;
        assert!(store.removal(&changed, id).is_err());
        assert!(
            store
                .update_removal(
                    &changed,
                    id,
                    RemovalPhase::Preparing,
                    RemovalPhase::Removing,
                    None
                )
                .await
                .is_err()
        );
        assert!(store.begin_removal(&changed, id, &build).await.is_err());
        store.require_agent_available(&changed).unwrap();
        assert!(
            store
                .update_removal(
                    &server,
                    id,
                    RemovalPhase::Preparing,
                    RemovalPhase::Restored,
                    None
                )
                .await
                .is_err()
        );
        let mut reused = record(&changed);
        reused.intent.operation_id = id;
        assert!(store.begin(reused).await.is_err());
        // The exact error remains attached across a duplicate transition.
        store
            .update_removal(
                &server,
                id,
                RemovalPhase::Preparing,
                RemovalPhase::Removing,
                Some("lost reply".into()),
            )
            .await
            .unwrap();
        let retry = store
            .update_removal(
                &server,
                id,
                RemovalPhase::Preparing,
                RemovalPhase::Removing,
                None,
            )
            .await
            .unwrap();
        assert_eq!(retry.error.as_deref(), Some("lost reply"));
    }

    #[tokio::test]
    async fn competing_installation_and_removal_have_one_durable_winner() {
        let store = std::sync::Arc::new(Store::default());
        let server = server();
        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(3));
        let installation = {
            let (store, server, barrier) = (store.clone(), server.clone(), barrier.clone());
            tokio::spawn(async move {
                barrier.wait().await;
                store.begin(record(&server)).await.is_ok()
            })
        };
        let removal = {
            let (store, server, barrier) = (store.clone(), server.clone(), barrier.clone());
            tokio::spawn(async move {
                barrier.wait().await;
                store
                    .begin_removal(&server, Uuid::new_v4(), &"5".repeat(64))
                    .await
                    .is_ok()
            })
        };
        barrier.wait().await;
        assert_ne!(installation.await.unwrap(), removal.await.unwrap());
        let snapshot = store.operations_snapshot(&server).unwrap();
        assert_eq!(snapshot.operations.len() + snapshot.removals.len(), 1);
    }

    #[tokio::test]
    async fn unknown_or_conflicting_journals_preserve_evidence_and_block_agent_admission() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("needle.json");
        let server = server();
        let store = Store::new(path.clone());
        store
            .begin_removal(&server, Uuid::new_v4(), &"6".repeat(64))
            .await
            .unwrap();
        let shard = store.journal_path(server.id).unwrap();
        let valid: serde_json::Value = serde_json::from_slice(&fs::read(&shard).unwrap()).unwrap();
        for kind in [
            "unknown version",
            "unknown field",
            "conflicting intent",
            "reused id",
            "unknown phase",
        ] {
            let mut data = valid.clone();
            match kind {
                "unknown version" => data["version"] = 2.into(),
                "unknown field" => data["extra"] = true.into(),
                "conflicting intent" => data["operations"] = serde_json::json!([record(&server)]),
                "reused id" => {
                    let mut r = record(&server);
                    r.phase = Phase::Ready;
                    r.intent.operation_id =
                        serde_json::from_value(data["removals"][0]["operationId"].clone()).unwrap();
                    data["operations"] = serde_json::json!([r]);
                }
                _ => data["removals"][0]["phase"] = "unknown".into(),
            }
            let bytes = serde_json::to_vec(&data).unwrap();
            fs::write(&shard, &bytes).unwrap();
            let store = Store::new(path.clone());
            assert!(store.require_agent_available(&server).is_err(), "{kind}");
            assert!(store.begin(record(&server)).await.is_err(), "{kind}");
            assert!(
                store
                    .begin_removal(&server, Uuid::new_v4(), &"6".repeat(64))
                    .await
                    .is_err(),
                "{kind}"
            );
            assert_eq!(fs::read(&shard).unwrap(), bytes);
        }
    }
}
