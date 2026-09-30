//! Desktop recovery receipts. Written before dispatch; never automatically replay writes.
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tauri::State;
use uuid::Uuid;

const MAX_RECORDS: usize = 1000;
const MAX_BYTES: u64 = 1024 * 1024;

#[derive(Clone)]
pub struct PendingOperations {
    path: PathBuf,
    access: Arc<Mutex<()>>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Receipt {
    pub operation_id: Uuid,
    pub server_id: Uuid,
    pub action: String,
    pub state: ReceiptState,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptState {
    Pending,
    Succeeded,
    Failed,
    Rejected,
    NeedsResolution,
    OutcomeUnknown,
    ReviewedUnknown,
}
impl ReceiptState {
    fn complete(self) -> bool {
        matches!(
            self,
            Self::Succeeded
                | Self::Failed
                | Self::Rejected
                | Self::NeedsResolution
                | Self::ReviewedUnknown
        )
    }
}
/// For operation.start these errors are returned before journal.begin/apply.
/// Mutation failures are returned inside a durable operation record instead.
pub(super) fn rejected_before_mutation(code: &str) -> bool {
    matches!(
        code,
        "STALE_SNAPSHOT" | "INVALID_REQUEST" | "REPO_NOT_FOUND" | "UNSUPPORTED_METHOD"
    )
}
/// Init/clone return these top-level errors only before journal.begin.
/// Once preparation starts, errors are contained in the operation record;
/// failure to persist that record instead returns OUTCOME_UNKNOWN.
pub(super) fn bootstrap_rejected_before_mutation(code: &str) -> bool {
    matches!(
        code,
        "INVALID_REQUEST"
            | "UNSUPPORTED_METHOD"
            | "DIRECTORY_REQUIRED"
            | "ALREADY_REPOSITORY"
            | "PATH_EXISTS"
            | "PATH_CHANGED"
    )
}
pub(super) fn remote_outcome(
    value: &serde_json::Value,
    expected_id: &str,
) -> Result<ReceiptState, ()> {
    if value["operationId"].as_str() != Some(expected_id) {
        return Err(());
    }
    match value["state"].as_str() {
        Some("running") => Ok(ReceiptState::Pending),
        Some("succeeded") => Ok(ReceiptState::Succeeded),
        Some("failed") => Ok(ReceiptState::Failed),
        Some("needs_resolution") => Ok(ReceiptState::NeedsResolution),
        Some("outcome_unknown") => Ok(ReceiptState::OutcomeUnknown),
        Some("reviewed_unknown") => Ok(ReceiptState::ReviewedUnknown),
        _ => Err(()),
    }
}
impl PendingOperations {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            access: Arc::new(Mutex::new(())),
        }
    }
    fn read(&self) -> Result<Vec<Receipt>, String> {
        let file = match fs::File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.to_string()),
        };
        let mut bytes = Vec::new();
        file.take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err("Git recovery log exceeds its limit.".into());
        }
        let rows: Vec<Receipt> = serde_json::from_slice(&bytes)
            .map_err(|e| format!("Cannot read Git recovery log: {e}"))?;
        let mut ids = std::collections::HashSet::new();
        if rows.len() > MAX_RECORDS
            || rows.iter().any(|row| {
                !ids.insert(row.operation_id) || row.action.is_empty() || row.action.len() > 80
            })
        {
            return Err("Git recovery log is invalid.".into());
        }
        Ok(rows)
    }
    fn save(&self, rows: &[Receipt]) -> Result<(), String> {
        let parent = self.path.parent().ok_or("Invalid recovery log path.")?;
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
        serde_json::to_writer(&mut file, rows).map_err(|e| e.to_string())?;
        file.flush().map_err(|e| e.to_string())?;
        file.as_file().sync_all().map_err(|e| e.to_string())?;
        file.persist(&self.path).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        fs::File::open(parent)
            .and_then(|file| file.sync_all())
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn begin(&self, server_id: Uuid, operation_id: Uuid, action: String) -> Result<(), String> {
        let _guard = self
            .access
            .lock()
            .map_err(|_| "Git recovery log is unavailable.")?;
        let mut rows = self.read()?;
        if action.is_empty() || action.len() > 80 {
            return Err("Invalid Git action name.".into());
        }
        if let Some(old) = rows.iter().find(|row| row.operation_id == operation_id) {
            if old.server_id != server_id || old.action != action {
                return Err("Operation ID belongs to another action.".into());
            }
            // A previous dispatch might have completed. Recovery uses operation.get.
            return Err(
                "This operation is already recorded. Check its outcome before continuing.".into(),
            );
        }
        if rows.len() >= MAX_RECORDS {
            return Err("Review saved Git outcomes before starting more operations.".into());
        }
        rows.push(Receipt {
            operation_id,
            server_id,
            action,
            state: ReceiptState::Pending,
        });
        self.save(&rows)
    }
    pub fn observe(
        &self,
        server_id: Uuid,
        operation_id: Uuid,
        state: ReceiptState,
    ) -> Result<(), String> {
        let _guard = self
            .access
            .lock()
            .map_err(|_| "Git recovery log is unavailable.")?;
        let mut rows = self.read()?;
        if let Some(row) = rows
            .iter_mut()
            .find(|row| row.operation_id == operation_id && row.server_id == server_id)
        {
            if row.state == ReceiptState::ReviewedUnknown {
                match state {
                    ReceiptState::OutcomeUnknown | ReceiptState::ReviewedUnknown => return Ok(()),
                    ReceiptState::Succeeded
                    | ReceiptState::Failed
                    | ReceiptState::NeedsResolution => {}
                    _ => {
                        return Err(
                            "A reviewed operation cannot return to an unconfirmed state.".into(),
                        )
                    }
                }
            } else if row.state.complete() && row.state != state {
                return Err("Git operation outcome changed unexpectedly.".into());
            }
            row.state = state;
            self.save(&rows)?;
        }
        Ok(())
    }
    fn review_ready(&self, server_id: Uuid, operation_id: Uuid) -> Result<(), String> {
        let _guard = self
            .access
            .lock()
            .map_err(|_| "Git recovery log is unavailable.")?;
        let rows = self.read()?;
        let row = rows
            .iter()
            .find(|row| row.server_id == server_id && row.operation_id == operation_id)
            .ok_or("The saved operation does not belong to this server.")?;
        if !matches!(
            row.state,
            ReceiptState::OutcomeUnknown | ReceiptState::ReviewedUnknown
        ) {
            return Err("Refresh this operation's outcome before reviewing it.".into());
        }
        Ok(())
    }
    /// Persist an operator acknowledgement only after remote journal confirmation.
    /// The boolean is supplied by the native lookup flow, never by the frontend.
    fn review(
        &self,
        server_id: Uuid,
        operation_id: Uuid,
        confirmed_reviewed_unknown: bool,
    ) -> Result<(), String> {
        if !confirmed_reviewed_unknown {
            return Err("Only a remotely confirmed review can be saved.".into());
        }
        let _guard = self
            .access
            .lock()
            .map_err(|_| "Git recovery log is unavailable.")?;
        let mut rows = self.read()?;
        let row = rows
            .iter_mut()
            .find(|row| row.server_id == server_id && row.operation_id == operation_id)
            .ok_or("The saved operation does not belong to this server.")?;
        match row.state {
            ReceiptState::OutcomeUnknown => row.state = ReceiptState::ReviewedUnknown,
            ReceiptState::ReviewedUnknown => return Ok(()),
            _ => return Err("The operation outcome changed. Refresh it before reviewing.".into()),
        }
        self.save(&rows)
    }
    fn acknowledge(&self, server_id: Uuid, operation_id: Uuid) -> Result<(), String> {
        let _guard = self
            .access
            .lock()
            .map_err(|_| "Git recovery log is unavailable.")?;
        let mut rows = self.read()?;
        if rows.iter().any(|row| {
            row.server_id == server_id && row.operation_id == operation_id && !row.state.complete()
        }) {
            return Err("This operation has no confirmed outcome yet.".into());
        }
        rows.retain(|row| row.server_id != server_id || row.operation_id != operation_id);
        self.save(&rows)
    }
}

#[tauri::command]
pub async fn git_pending_operations(
    state: State<'_, PendingOperations>,
    server_id: Uuid,
) -> Result<Vec<Receipt>, String> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let _guard = state
            .access
            .lock()
            .map_err(|_| "Git recovery log is unavailable.")?;
        Ok(state
            .read()?
            .into_iter()
            .filter(|row| row.server_id == server_id)
            .collect())
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
pub async fn git_acknowledge_operation(
    state: State<'_, PendingOperations>,
    server_id: Uuid,
    operation_id: Uuid,
) -> Result<(), String> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || state.acknowledge(server_id, operation_id))
        .await
        .map_err(|e| e.to_string())?
}

async fn review_with_remote_confirmation<F, Fut>(
    pending: PendingOperations,
    server_id: Uuid,
    operation_id: Uuid,
    lookup: F,
) -> Result<(), super::protocol::Error>
where
    F: FnOnce(super::protocol::Request) -> Fut,
    Fut: std::future::Future<Output = Result<serde_json::Value, super::protocol::Error>>,
{
    let check = pending.clone();
    tokio::task::spawn_blocking(move || check.review_ready(server_id, operation_id))
        .await
        .map_err(|_| {
            super::protocol::Error::new(
                "LOCAL_JOURNAL_UNAVAILABLE",
                "Git recovery log is unavailable.",
            )
        })?
        .map_err(|message| super::protocol::Error::new("OPERATION_NOT_REVIEWABLE", message))?;
    let id = operation_id.to_string();
    // Review is idempotent and never executes the original Git action.
    let result = lookup(super::protocol::Request::Review {
        operation_id: id.clone(),
    })
    .await?;
    let confirmed_reviewed_unknown =
        remote_outcome(&result, &id) == Ok(ReceiptState::ReviewedUnknown);
    if !confirmed_reviewed_unknown {
        return Err(super::protocol::Error::new(
            "OPERATION_NOT_REVIEWABLE",
            "The server did not confirm the operation review.",
        ));
    }
    tokio::task::spawn_blocking(move || {
        pending.review(server_id, operation_id, confirmed_reviewed_unknown)
    })
    .await
    .map_err(|_| {
        super::protocol::Error::new(
            "LOCAL_JOURNAL_UNAVAILABLE",
            "Git recovery log is unavailable.",
        )
    })?
    .map_err(|message| super::protocol::Error::new("OPERATION_NOT_REVIEWABLE", message))
}

#[tauri::command]
pub async fn git_review_operation(
    app: tauri::AppHandle,
    preferences: State<'_, crate::preferences::Preferences>,
    state: State<'_, super::Shared>,
    sessions: State<'_, super::Sessions>,
    pending: State<'_, PendingOperations>,
    server_id: Uuid,
    operation_id: Uuid,
) -> Result<(), super::protocol::Error> {
    let ledger = pending.inner().clone();
    review_with_remote_confirmation(ledger, server_id, operation_id, |request| {
        super::git_request(
            app,
            preferences,
            state,
            sessions,
            pending,
            server_id,
            request,
            None,
        )
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn receipts_survive_restart_and_require_confirmed_outcomes() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("operations.json");
        let store = PendingOperations::new(path.clone());
        let server = Uuid::new_v4();
        let operation = Uuid::new_v4();
        store.begin(server, operation, "commit".into()).unwrap();
        assert!(store.begin(server, operation, "commit".into()).is_err());
        let reopened = PendingOperations::new(path);
        assert_eq!(reopened.read().unwrap()[0].state, ReceiptState::Pending);
        assert!(reopened.acknowledge(server, operation).is_err());
        reopened
            .observe(server, operation, ReceiptState::OutcomeUnknown)
            .unwrap();
        assert!(reopened.acknowledge(server, operation).is_err());
        reopened
            .observe(server, operation, ReceiptState::Succeeded)
            .unwrap();
        assert!(reopened
            .observe(server, operation, ReceiptState::Failed)
            .is_err());
        reopened.acknowledge(Uuid::new_v4(), operation).unwrap();
        assert_eq!(reopened.read().unwrap().len(), 1);
        reopened.acknowledge(server, operation).unwrap();
        assert!(reopened.read().unwrap().is_empty());
    }

    #[test]
    fn reviewed_unknown_is_persisted_nonblocking_and_cannot_be_replayed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("operations.json");
        let store = PendingOperations::new(path.clone());
        let server = Uuid::new_v4();
        let operation = Uuid::new_v4();
        store
            .begin(server, operation, "pull.fast_forward".into())
            .unwrap();
        assert!(store.review(server, operation, true).is_err());
        store
            .observe(server, operation, ReceiptState::OutcomeUnknown)
            .unwrap();
        assert!(store.acknowledge(server, operation).is_err());
        assert!(store.review(server, operation, false).is_err());
        assert!(store.review(Uuid::new_v4(), operation, true).is_err());
        store.review(server, operation, true).unwrap();
        let reopened = PendingOperations::new(path.clone());
        assert_eq!(
            reopened.read().unwrap()[0].state,
            ReceiptState::ReviewedUnknown
        );
        assert!(ReceiptState::ReviewedUnknown.complete());
        assert!(String::from_utf8(fs::read(path).unwrap())
            .unwrap()
            .contains("reviewed_unknown"));
        assert!(reopened
            .begin(server, operation, "pull.fast_forward".into())
            .is_err());
        reopened
            .observe(server, operation, ReceiptState::OutcomeUnknown)
            .unwrap();
        assert_eq!(
            reopened.read().unwrap()[0].state,
            ReceiptState::ReviewedUnknown
        );
        assert!(reopened
            .observe(server, operation, ReceiptState::Pending)
            .is_err());
        assert!(reopened
            .observe(server, operation, ReceiptState::Rejected)
            .is_err());
        assert_eq!(
            reopened.read().unwrap()[0].state,
            ReceiptState::ReviewedUnknown
        );
        reopened
            .observe(server, operation, ReceiptState::Succeeded)
            .unwrap();
        assert_eq!(reopened.read().unwrap()[0].state, ReceiptState::Succeeded);
    }

    #[test]
    fn reconnect_recovers_a_review_whose_response_was_lost() {
        let dir = tempfile::tempdir().unwrap();
        let store = PendingOperations::new(dir.path().join("operations.json"));
        let server = Uuid::new_v4();
        let operation = Uuid::new_v4();
        store
            .begin(server, operation, "pull.fast_forward".into())
            .unwrap();
        store
            .observe(server, operation, ReceiptState::OutcomeUnknown)
            .unwrap();
        let response = serde_json::json!({"operationId":operation,"state":"reviewed_unknown"});
        store
            .observe(
                server,
                operation,
                remote_outcome(&response, &operation.to_string()).unwrap(),
            )
            .unwrap();
        assert_eq!(
            store.read().unwrap()[0].state,
            ReceiptState::ReviewedUnknown
        );
        store.review(server, operation, true).unwrap();
    }

    #[tokio::test]
    async fn review_confirms_remote_journal_and_rejects_stale_or_uncertain_results() {
        use super::super::protocol::{Error, Request};
        let dir = tempfile::tempdir().unwrap();
        let store = PendingOperations::new(dir.path().join("operations.json"));
        let server = Uuid::new_v4();
        let operation = Uuid::new_v4();
        store
            .begin(server, operation, "pull.fast_forward".into())
            .unwrap();
        store
            .observe(server, operation, ReceiptState::OutcomeUnknown)
            .unwrap();
        let initial = fs::read(&store.path).unwrap();
        for response in [
            Err(Error::transport("connection lost")),
            Err(Error::new("OPERATION_NOT_FOUND", "missing")),
            Ok(serde_json::json!({"operationId":operation,"state":"running"})),
            Ok(serde_json::json!({"operationId":operation,"state":"pending"})),
            Ok(serde_json::json!({"operationId":operation,"state":"outcome_unknown"})),
            Ok(serde_json::json!({"operationId":operation,"state":"succeeded"})),
            Ok(serde_json::json!({"operationId":Uuid::new_v4(),"state":"outcome_unknown"})),
        ] {
            let result=review_with_remote_confirmation(store.clone(),server,operation,|request|async move {
                assert!(matches!(request,Request::Review{operation_id} if operation_id==operation.to_string()));response
            }).await;
            assert!(result.is_err());
            assert_eq!(fs::read(&store.path).unwrap(), initial);
        }
        let unknown = serde_json::json!({"operationId":operation,"state":"reviewed_unknown"});
        assert!(review_with_remote_confirmation(
            store.clone(),
            Uuid::new_v4(),
            operation,
            |_| async { Ok(unknown.clone()) }
        )
        .await
        .is_err());
        assert_eq!(fs::read(&store.path).unwrap(), initial);
        // A second observation finishing between lookup and persistence wins;
        // a stale unknown response cannot overwrite a confirmed outcome.
        assert!(
            review_with_remote_confirmation(store.clone(), server, operation, |_| async {
                store
                    .observe(server, operation, ReceiptState::Failed)
                    .unwrap();
                Ok(unknown.clone())
            })
            .await
            .is_err()
        );
        assert_eq!(store.read().unwrap()[0].state, ReceiptState::Failed);
        let second = Uuid::new_v4();
        store.begin(server, second, "merge".into()).unwrap();
        store
            .observe(server, second, ReceiptState::OutcomeUnknown)
            .unwrap();
        let calls = std::sync::atomic::AtomicUsize::new(0);
        review_with_remote_confirmation(store.clone(), server, second, |request| async {
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            assert!(
                matches!(request,Request::Review{operation_id} if operation_id==second.to_string())
            );
            Ok(serde_json::json!({"operationId":second,"state":"reviewed_unknown"}))
        })
        .await
        .unwrap();
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(
            store.read().unwrap()[1].state,
            ReceiptState::ReviewedUnknown
        );
    }

    #[tokio::test]
    async fn review_preflight_rejects_pending_missing_and_other_servers_without_lookup() {
        use super::super::protocol::Error;
        let dir = tempfile::tempdir().unwrap();
        let store = PendingOperations::new(dir.path().join("operations.json"));
        let server = Uuid::new_v4();
        let operation = Uuid::new_v4();
        store.begin(server, operation, "fetch".into()).unwrap();
        for (server_id, operation_id) in [
            (server, operation),
            (Uuid::new_v4(), operation),
            (server, Uuid::new_v4()),
        ] {
            let calls = std::sync::atomic::AtomicUsize::new(0);
            let result = review_with_remote_confirmation(
                store.clone(),
                server_id,
                operation_id,
                |_| async {
                    calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Err(Error::transport("Unexpected lookup"))
                },
            )
            .await;
            assert_eq!(result.unwrap_err().code, "OPERATION_NOT_REVIEWABLE");
            assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
        }
        assert_eq!(store.read().unwrap()[0].state, ReceiptState::Pending);
    }
    #[test]
    fn corrupt_logs_block_writes_without_erasing_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("operations.json");
        fs::write(&path, b"broken").unwrap();
        let store = PendingOperations::new(path.clone());
        assert!(store
            .begin(Uuid::new_v4(), Uuid::new_v4(), "stage".into())
            .is_err());
        assert_eq!(fs::read(path).unwrap(), b"broken");
    }
    #[test]
    fn concurrent_writers_preserve_each_receipt() {
        let temp = tempfile::tempdir().unwrap();
        let store = PendingOperations::new(temp.path().join("operations.json"));
        let server = Uuid::new_v4();
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let store = store.clone();
                std::thread::spawn(move || {
                    store.begin(server, Uuid::new_v4(), "stage".into()).unwrap()
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(store.read().unwrap().len(), 8);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&store.path).unwrap().permissions().mode() & 0o077,
                0
            );
        }
    }
    #[test]
    fn remote_outcomes_require_the_exact_operation_and_a_known_state() {
        let id = Uuid::new_v4().to_string();
        assert_eq!(
            remote_outcome(
                &serde_json::json!({"operationId":id,"state":"needs_resolution"}),
                &id
            ),
            Ok(ReceiptState::NeedsResolution)
        );
        assert!(remote_outcome(
            &serde_json::json!({"operationId":"other","state":"succeeded"}),
            &id
        )
        .is_err());
        assert!(remote_outcome(
            &serde_json::json!({"operationId":id,"state":"unknown-new-state"}),
            &id
        )
        .is_err());
    }
    #[test]
    fn known_rejections_can_be_acknowledged_but_uncertain_errors_stay_pending() {
        let temp = tempfile::tempdir().unwrap();
        let store = PendingOperations::new(temp.path().join("operations.json"));
        let server = Uuid::new_v4();
        let operation = Uuid::new_v4();
        store.begin(server, operation, "stage".into()).unwrap();
        for code in [
            "OUTCOME_UNKNOWN",
            "TRANSPORT_ERROR",
            "JOURNAL_UNAVAILABLE",
            "OPERATION_ID_REUSED",
            "IO_ERROR",
            "UNRECOGNIZED_ERROR",
        ] {
            assert!(!rejected_before_mutation(code));
            assert!(!bootstrap_rejected_before_mutation(code));
        }
        assert!(rejected_before_mutation("STALE_SNAPSHOT"));
        store
            .observe(server, operation, ReceiptState::Rejected)
            .unwrap();
        let reopened = PendingOperations::new(store.path.clone());
        assert_eq!(reopened.read().unwrap()[0].state, ReceiptState::Rejected);
        reopened.acknowledge(server, operation).unwrap();
        assert!(reopened.read().unwrap().is_empty());
    }
    #[cfg(unix)]
    #[test]
    fn rejected_creation_preserves_files_and_can_be_dismissed_without_a_remote_record() {
        use crate::git::{
            journal::Journal,
            protocol::{Path, Request},
            repository::Service,
        };
        use std::os::unix::ffi::OsStrExt;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("keep.txt"), "preserve this file").unwrap();
        let existing = temp.path().join("existing");
        git2::Repository::init(&existing).unwrap();
        let missing = temp.path().join("missing");
        let path = |p: &std::path::Path| Path::new(p.as_os_str().as_bytes());
        let journal =
            Journal::open(temp.path().join("journal"), Uuid::new_v4().to_string()).unwrap();
        let mut service = Service::with_journal(journal.clone());
        let store = PendingOperations::new(temp.path().join("operations.json"));
        let server = Uuid::new_v4();

        // Each scenario exercises the real agent boundary, not just a code whitelist.
        for scenario in 0..5 {
            let operation = Uuid::new_v4();
            let operation_id = operation.to_string();
            let (request, expected) = match scenario {
                0 => (
                    Request::Init {
                        operation_id: operation_id.clone(),
                        path: path(&root),
                        initial_branch: "bad..branch".into(),
                    },
                    "INVALID_REQUEST",
                ),
                1 => (
                    Request::Init {
                        operation_id: operation_id.clone(),
                        path: path(&missing),
                        initial_branch: "main".into(),
                    },
                    "DIRECTORY_REQUIRED",
                ),
                2 => (
                    Request::Init {
                        operation_id: operation_id.clone(),
                        path: path(&existing),
                        initial_branch: "main".into(),
                    },
                    "ALREADY_REPOSITORY",
                ),
                3 => (
                    Request::Clone {
                        operation_id: operation_id.clone(),
                        path: path(&root),
                        url: "https://example.test/repo.git".into(),
                        branch: None,
                        bare: false,
                    },
                    "PATH_EXISTS",
                ),
                _ => (
                    Request::Clone {
                        operation_id: operation_id.clone(),
                        path: path(&missing),
                        url: "https://user:secret@example.test/repo.git".into(),
                        branch: None,
                        bare: false,
                    },
                    "INVALID_REQUEST",
                ),
            };
            store
                .begin(
                    server,
                    operation,
                    if scenario < 3 {
                        "repo.init"
                    } else {
                        "repo.clone"
                    }
                    .into(),
                )
                .unwrap();
            let error = service.request(request).err().unwrap();
            assert_eq!(error.code, expected);
            assert!(bootstrap_rejected_before_mutation(&error.code));
            assert_eq!(
                journal.get(&operation_id).err().unwrap().code,
                "OPERATION_NOT_FOUND"
            );
            store
                .observe(server, operation, ReceiptState::Rejected)
                .unwrap();
            let reopened = PendingOperations::new(store.path.clone());
            assert_eq!(reopened.read().unwrap()[0].state, ReceiptState::Rejected);
            reopened.acknowledge(server, operation).unwrap();
            assert!(reopened.read().unwrap().is_empty());
        }
        assert_eq!(
            fs::read_to_string(root.join("keep.txt")).unwrap(),
            "preserve this file"
        );
        assert!(!root.join(".git").exists());
        assert!(!missing.exists());
        assert!(git2::Repository::open(existing).is_ok());
    }
    #[cfg(unix)]
    #[test]
    fn stale_snapshot_rejection_never_enters_the_remote_operation_journal() {
        use crate::git::{
            journal::Journal,
            protocol::{Action, Path, Request},
            repository::{Output, Service},
        };
        use std::os::unix::ffi::OsStrExt;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        git2::Repository::init(&root).unwrap();
        let journal =
            Journal::open(temp.path().join("journal"), Uuid::new_v4().to_string()).unwrap();
        let mut service = Service::with_journal(journal.clone());
        let Output::Json(opened) = service
            .request(Request::Open {
                path: Path::new(root.as_os_str().as_bytes()),
            })
            .unwrap()
        else {
            panic!("repository response")
        };
        let repo_id = opened["repoId"].as_str().unwrap().to_owned();
        let Output::Json(status) = service
            .request(Request::Status {
                filter: None,
                repo_id: repo_id.clone(),
                page_size: 100,
                cursor: None,
            })
            .unwrap()
        else {
            panic!("status response")
        };
        fs::write(root.join("new.txt"), "Changed after the snapshot").unwrap();
        let operation_id = Uuid::new_v4().to_string();
        let error = service
            .request(Request::Start {
                operation_id: operation_id.clone(),
                repo_id,
                expected_snapshot: status["snapshot"].as_str().unwrap().into(),
                action: Action::Stage {
                    hunks: None,
                    entry_ids: vec!["old-entry".into()],
                },
            })
            .err()
            .unwrap();
        assert_eq!(error.code, "STALE_SNAPSHOT");
        assert!(rejected_before_mutation(&error.code));
        assert_eq!(
            journal.get(&operation_id).err().unwrap().code,
            "OPERATION_NOT_FOUND"
        );
        assert_eq!(
            fs::read_to_string(root.join("new.txt")).unwrap(),
            "Changed after the snapshot"
        );
    }

    /// A write whose reply was lost blocks every later write until its outcome is
    /// known. The agent journals before it executes, so a missing record is a
    /// definitive negative rather than a permanent dead end.
    #[test]
    fn a_never_journaled_operation_resolves_instead_of_blocking_writes() {
        use crate::git::{journal::Journal, repository::Service};

        let temp = tempfile::tempdir().unwrap();
        let journal =
            Journal::open(temp.path().join("journal"), Uuid::new_v4().to_string()).unwrap();
        let mut service = Service::with_journal(journal);
        let store = PendingOperations::new(temp.path().join("operations.json"));
        let server = Uuid::new_v4();
        let operation = Uuid::new_v4();

        store.begin(server, operation, "commit".into()).unwrap();
        store
            .observe(server, operation, ReceiptState::OutcomeUnknown)
            .unwrap();
        // An uncertain receipt cannot simply be dismissed.
        assert!(store.acknowledge(server, operation).is_err());

        // The real agent boundary reports the absence, not a code whitelist.
        let error = service
            .request(crate::git::protocol::Request::Get {
                operation_id: operation.to_string(),
            })
            .err()
            .unwrap();
        assert_eq!(error.code, "OPERATION_NOT_FOUND");

        store
            .observe(server, operation, ReceiptState::Rejected)
            .unwrap();
        assert!(store.read().unwrap()[0].state.complete());
        store.acknowledge(server, operation).unwrap();
        assert!(store.read().unwrap().is_empty());
    }

    #[test]
    fn a_confirmed_outcome_is_never_downgraded_by_a_missing_record() {
        let temp = tempfile::tempdir().unwrap();
        let store = PendingOperations::new(temp.path().join("operations.json"));
        let server = Uuid::new_v4();
        let operation = Uuid::new_v4();
        store.begin(server, operation, "commit".into()).unwrap();
        store
            .observe(server, operation, ReceiptState::Succeeded)
            .unwrap();
        assert!(store
            .observe(server, operation, ReceiptState::Rejected)
            .is_err());
        assert_eq!(store.read().unwrap()[0].state, ReceiptState::Succeeded);
    }
}
