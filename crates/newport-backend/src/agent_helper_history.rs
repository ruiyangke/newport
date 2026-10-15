//! Remote helper evidence is separate from saved, authorized recovery operations.
use super::*;
use crate::lifecycle_receipts::protocol::HelperHistory;
#[path = "agent_helper_recovery.rs"]
pub(crate) mod recovery;

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Snapshot {
    pub supported: Option<bool>,
    pub history: Option<HelperHistory>,
    pub checked_at: u64,
    pub error: Option<String>,
}

#[derive(Default)]
pub(crate) struct State(Mutex<Snapshot>);

impl State {
    pub fn needs_check(&self) -> bool {
        self.0.lock().unwrap().checked_at == 0
    }
    pub fn invalidate(&self) {
        let mut snapshot = self.0.lock().unwrap();
        snapshot.checked_at = 0;
        snapshot.supported = None;
        if snapshot.history.is_some() {
            snapshot.error = Some("Needle changed; helper history needs a fresh check.".into());
        }
    }
    pub fn snapshot(&self) -> Snapshot {
        self.0.lock().unwrap().clone()
    }
    pub fn record(&self, result: Result<Option<HelperHistory>, String>) {
        let mut snapshot = self.0.lock().unwrap();
        snapshot.checked_at = crate::metrics_db::now_ms() as u64;
        match result {
            Ok(history) => {
                snapshot.supported = Some(history.is_some());
                snapshot.history = history;
                snapshot.error = None;
            }
            Err(error) => snapshot.error = Some(error),
        }
    }
}

pub(crate) async fn inspect(
    session: &ExecSession,
    hash: &str,
) -> Result<Option<HelperHistory>, String> {
    let command = "\"$HOME/.local/bin/newport-agent\"";
    let protocol = super::helper_protocol(session, command, hash).await?;
    if protocol["helperHistory"] != 1 {
        return Ok(None);
    }
    parse(
        &session
            .execute(&format!("{command} deployment helper-history"), None)
            .await?,
        hash,
    )
    .map(Some)
}

fn parse(output: &str, expected_build: &str) -> Result<HelperHistory, String> {
    if output.len() > 512 * 1024 {
        return Err("Needle helper history exceeds limit".into());
    }
    let history: HelperHistory =
        serde_json::from_str(output).map_err(|_| "Invalid Needle helper history")?;
    let valid = crate::agent_runtime::protocol::valid_build;
    let mut ids = std::collections::HashSet::new();
    if history.version != 1
        || !valid(expected_build)
        || history.build_sha256 != expected_build
        || history.operations.len() > 1000
        || (!history.ownership_recorded && !history.operations.is_empty())
        || history.operations.iter().any(|row| {
            row.operation_id.is_nil()
                || !ids.insert(row.operation_id)
                || row.file_count > 32
                || !valid(&row.fingerprint)
                || (row.origin == crate::lifecycle_receipts::protocol::HelperOrigin::Unattributed
                    && row.restore_blocked.is_none())
                || row.restore_blocked.as_ref().is_some_and(|s| s.len() > 4096)
        })
    {
        return Err("Invalid or obsolete Needle helper history".into());
    }
    Ok(history)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn history() -> serde_json::Value {
        json!({"version":1,"buildSha256":"a".repeat(64),"ownershipRecorded":true,
            "operations":[{"operationId":uuid::Uuid::new_v4(),"origin":"standalone","state":"applied","fileCount":2,
                "fingerprint":"b".repeat(64),"restoreBlocked":null}]})
    }
    #[test]
    fn history_requires_current_build_unique_ids_and_bounded_ownership() {
        let value = history();
        let hash = "a".repeat(64);
        assert!(parse(&value.to_string(), &hash).is_ok());
        assert!(parse(&value.to_string(), &"c".repeat(64)).is_err());
        for (field, invalid) in [
            ("version", json!(2)),
            ("ownershipRecorded", json!(false)),
            ("buildSha256", json!("changed")),
        ] {
            let mut value = history();
            value[field] = invalid;
            assert!(parse(&value.to_string(), &hash).is_err());
        }
        for (field, invalid) in [
            ("operationId", json!(uuid::Uuid::nil())),
            ("state", json!("unknown")),
            ("fingerprint", json!("invalid")),
            ("fileCount", json!(33)),
            ("restoreBlocked", json!("x".repeat(4097))),
        ] {
            let mut value = history();
            value["operations"][0][field] = invalid;
            assert!(parse(&value.to_string(), &hash).is_err());
        }
        let mut duplicate = history();
        let row = duplicate["operations"][0].clone();
        duplicate["operations"].as_array_mut().unwrap().push(row);
        assert!(parse(&duplicate.to_string(), &hash).is_err());
        assert!(parse(&" ".repeat(512 * 1024 + 1), &hash).is_err());
    }

    #[test]
    fn errors_keep_last_observation_without_manufacturing_local_intent() {
        let state = State::default();
        let value = parse(&history().to_string(), &"a".repeat(64)).unwrap();
        let id = value.operations[0].operation_id;
        state.record(Ok(Some(value)));
        state.record(Err("offline".into()));
        let snapshot = state.snapshot();
        assert_eq!(snapshot.history.unwrap().operations[0].operation_id, id);
        assert_eq!(snapshot.error.as_deref(), Some("offline"));
        assert!(!state.needs_check());
        state.invalidate();
        assert!(state.needs_check());
        assert!(state.snapshot().history.is_some());
        assert_eq!(state.snapshot().supported, None);
        state.record(Ok(None));
        assert_eq!(state.snapshot().supported, Some(false));
        assert!(state.snapshot().history.is_none());
        // Another connection owner never inherits an observation or its error.
        assert!(State::default().snapshot().history.is_none());
    }

    #[tokio::test]
    #[ignore = "Requires the disposable Linux OpenSSH fixture"]
    async fn discovery_preserves_remote_files_and_does_not_create_recovery_intent() {
        let server: crate::model::Server = serde_json::from_value(json!({
            "id":uuid::Uuid::new_v4(), "name":"Helper history fixture", "sshUser":"fixture",
            "sshHost":"127.0.0.1", "sshPort":std::env::var("NEWPORT_TEST_SSH_PORT").unwrap().parse::<u16>().unwrap(),
            "identityFile":std::env::var("NEWPORT_TEST_IDENTITY").unwrap(), "agentInstallApproved":true
        })).unwrap();
        let registry = crate::server_session::Registry::default();
        let owner = registry.session(&server, 0);
        let lease = owner.acquire(&server).await.unwrap();
        lease.install(false, false).await.unwrap();
        assert!(owner.lifecycle.helper_history.needs_check());
        lease.check_installation_if_due().await.unwrap();
        let history = owner.lifecycle.helper_history.snapshot();
        assert_eq!(history.supported, Some(true));
        assert!(history.history.unwrap().operations.is_empty());
        let local_before = owner.lifecycle.receipts.for_server(&server).unwrap().len();
        lease
            .execute("\"$HOME/.local/bin/newport-agent\" install", None)
            .await
            .unwrap();
        let journal_command = "cat \"$HOME/.local/share/newport/needle/helpers/journal.json\"";
        let before = lease.execute(journal_command, None).await.unwrap();
        lease.check_installation().await.unwrap();
        let history = owner.lifecycle.helper_history.snapshot().history.unwrap();
        assert_eq!(history.operations.len(), 1);
        let id = history.operations[0].operation_id;
        assert_eq!(
            history.operations[0].origin,
            crate::lifecycle_receipts::protocol::HelperOrigin::Standalone
        );
        assert_eq!(lease.execute(journal_command, None).await.unwrap(), before);
        assert_eq!(
            owner.lifecycle.receipts.for_server(&server).unwrap().len(),
            local_before
        );
        assert!(owner.lifecycle.receipts.get(id).unwrap().is_none());
        assert_eq!(
            registry.lifecycle_operations()[&server.id]["helperHistory"]["history"]["operations"]
                [0]["operationId"],
            id.to_string()
        );
        lease
            .execute(
                "printf '{}' > \"$HOME/.local/share/newport/needle/helpers/journal.json\"",
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            lease.check_installation().await.unwrap().installed,
            Some(true)
        );
        let failed = owner.lifecycle.helper_history.snapshot();
        assert!(failed.error.is_some());
        assert_eq!(failed.history.unwrap().operations[0].operation_id, id);
        let replacement = registry.session(&server, 1);
        assert!(
            replacement
                .lifecycle
                .helper_history
                .snapshot()
                .history
                .is_none()
        );
        registry.shutdown().await;
    }
}
