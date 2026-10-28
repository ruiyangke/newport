use crate::{manager::Shared, model::Server};
use anyhow::Result;
use serde::Serialize;
use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    sync::{Arc, Mutex},
};
use tokio::sync::watch;
use uuid::Uuid;

#[derive(Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub name: String,
    pub completed: u64,
    pub total: Option<u64>,
}
struct Operation {
    server: Uuid,
    cancel: watch::Sender<bool>,
    progress: Progress,
}
#[derive(Clone, Default)]
pub struct Operations {
    pub(super) editor_writes: Arc<tokio::sync::Mutex<()>>,
    registry: Arc<Mutex<Registry>>,
    uploads: Arc<super::recovery_storage::Owners<super::cleanup::UploadCleanup>>,
    downloads: Arc<super::recovery_storage::Owners<super::download_cleanup::DownloadCleanup>>,
}
#[path = "operations_recovery.rs"]
pub(super) mod recovery;
#[derive(Default)]
struct Registry {
    entries: HashMap<Uuid, Operation>,
    cancelled: VecDeque<Uuid>,
    closing: bool,
    cleanup: Vec<tokio::task::JoinHandle<()>>,
}
struct Guard<'a>(&'a Operations, Uuid);

fn file_error(error: anyhow::Error) -> String {
    use russh_sftp::client::error::Error;
    match error.downcast_ref::<Error>() {
        Some(Error::Timeout) => "The file connection timed out. Refresh the folder and check the destination before retrying a transfer.".into(),
        // RawSftpSession reports a closed response channel this way. Keep other
        // protocol errors intact rather than misclassifying every unexpected reply.
        Some(Error::UnexpectedBehavior(message)) if message == "sender dropped" =>
            "The file connection closed before the operation finished. Refresh the folder and check the destination before retrying a transfer.".into(),
        _ => format!("{error:#}"),
    }
}
impl Drop for Guard<'_> {
    fn drop(&mut self) {
        self.0.registry.lock().unwrap().entries.remove(&self.1);
    }
}
impl Operations {
    /// For a deleted server: removes its partial downloads on this computer
    /// and its transfer recovery records. Call on a blocking worker.
    pub(crate) fn forget_server(&self, server: Uuid) -> Result<(), String> {
        let uploads = self.uploads.forget(server);
        let downloads = self.downloads.forget(server);
        uploads.and(downloads).map_err(|error| format!("{error:#}"))
    }
    pub async fn run<T, F>(
        &self,
        state: &Shared,
        server: Uuid,
        id: Uuid,
        work: F,
    ) -> Result<T, String>
    where
        F: AsyncFnOnce(Server) -> Result<T>,
    {
        self.run_inner(state, server, id, None, work).await
    }

    pub async fn run_with_connection<T, F>(
        &self,
        state: &Shared,
        server: Uuid,
        id: Uuid,
        expected_connection: Option<&serde_json::Value>,
        work: F,
    ) -> Result<T, String>
    where
        F: AsyncFnOnce(Server) -> Result<T>,
    {
        match expected_connection {
            Some(expected) => {
                self.run_inner(state, server, id, Some(expected), work)
                    .await
            }
            None => self.run(state, server, id, work).await,
        }
    }

    async fn run_inner<T, F>(
        &self,
        state: &Shared,
        server: Uuid,
        id: Uuid,
        expected_connection: Option<&serde_json::Value>,
        work: F,
    ) -> Result<T, String>
    where
        F: AsyncFnOnce(Server) -> Result<T>,
    {
        let (profile, owner) = state.current(server)?;
        let mut retired = owner.cancellation();
        let (server, mut cancel, _guard) = {
            // Subscribe before registering work. Retirement cancels this exact
            // incarnation even if a profile edit races operation registration.
            if expected_connection.is_some() && !state.configuration_available() {
                return Err("Server configuration is unavailable.".into());
            }
            if let Some(expected) = expected_connection {
                let profile = serde_json::to_value(&profile).map_err(|error| error.to_string())?;
                let snapshot = serde_json::json!({"instanceId":state.instance_id,
                    "runtime":{"connectionRevisions":{server.to_string():owner.revision()}}});
                if newport_domain::connection_scope::connection(&profile, &snapshot) != *expected {
                    return Err(
                        "The server connection changed before the file transfer could start."
                            .into(),
                    );
                }
            }
            let mut registry = self.registry.lock().unwrap();
            if registry.closing {
                return Err("Newport is shutting down".into());
            }
            if let Some(index) = registry
                .cancelled
                .iter()
                .position(|cancelled| *cancelled == id)
            {
                registry.cancelled.remove(index);
                return Err("File operation cancelled".into());
            }
            let entries = &mut registry.entries;
            if entries.len() >= 8 {
                return Err("Too many file operations. Wait for one to finish.".into());
            }
            if entries.contains_key(&id) {
                return Err("File operation already exists".into());
            }
            let (sender, receiver) = watch::channel(false);
            entries.insert(
                id,
                Operation {
                    server,
                    cancel: sender,
                    progress: Progress::default(),
                },
            );
            (profile, receiver, Guard(self, id))
        };
        if *retired.borrow() {
            return Err("File operation cancelled".into());
        }
        let result = tokio::select! {
            biased;
            _ = retired.changed() => Err(anyhow::anyhow!("File operation cancelled")),
            _ = cancel.changed() => Err(anyhow::anyhow!("File operation cancelled")),
            result = work(server) => result,
        };
        if owner.is_revoked() {
            return Err("File operation cancelled".into());
        }
        result.map_err(file_error)
    }
    pub fn progress(&self, id: Uuid, name: &str, completed: u64, total: Option<u64>) {
        if let Some(entry) = self.registry.lock().unwrap().entries.get_mut(&id) {
            entry.progress = Progress {
                name: name.into(),
                completed,
                total,
            };
        }
    }
    pub fn cancel_server(&self, server: Option<Uuid>) {
        for entry in self.registry.lock().unwrap().entries.values() {
            if server.is_none_or(|id| id == entry.server) {
                let _ = entry.cancel.send(true);
            }
        }
    }

    pub(super) fn cleanup(&self, work: impl Future<Output = ()> + Send + 'static) {
        let mut registry = self.registry.lock().unwrap();
        registry.cleanup.retain(|task| !task.is_finished());
        registry.cleanup.push(tokio::spawn(work));
    }

    pub async fn shutdown(&self) {
        {
            let mut registry = self.registry.lock().unwrap();
            registry.closing = true;
            for entry in registry.entries.values() {
                let _ = entry.cancel.send(true);
            }
        }
        // Dropping each operation registers staging-file cleanup before its
        // guard removes the registry entry. Reject new work throughout shutdown.
        let drain = async {
            while !self.registry.lock().unwrap().entries.is_empty() {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            // A cancelled staging worker can hand ownership to a cleanup
            // worker while draining. Include that follow-up before returning.
            loop {
                let tasks = std::mem::take(&mut self.registry.lock().unwrap().cleanup);
                if tasks.is_empty() {
                    break;
                }
                for task in tasks {
                    let _ = task.await;
                }
            }
        };
        if tokio::time::timeout(std::time::Duration::from_secs(5), drain)
            .await
            .is_err()
        {
            log::warn!("File cleanup timed out during shutdown; remote staging files may remain");
        }
    }
}
pub fn files_cancel(operations: &Operations, operation: Uuid) {
    let mut registry = operations.registry.lock().unwrap();
    if let Some(entry) = registry.entries.get(&operation) {
        let _ = entry.cancel.send(true);
    } else {
        // Client cancellation can arrive before operation registration.
        if registry.cancelled.len() == 128 {
            registry.cancelled.pop_front();
        }
        registry.cancelled.push_back(operation);
    }
}
pub fn files_progress(operations: &Operations, operation: Uuid) -> Option<Progress> {
    operations
        .registry
        .lock()
        .unwrap()
        .entries
        .get(&operation)
        .map(|entry| entry.progress.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{manager::Manager, profile::Store};
    use serde_json::{Value, json};
    use std::sync::Arc;

    #[test]
    fn file_connection_errors_are_actionable_without_hiding_other_failures() {
        use russh_sftp::client::error::Error;
        assert!(file_error(Error::Timeout.into()).starts_with("The file connection timed out."));
        let closed = anyhow::Error::from(Error::UnexpectedBehavior("sender dropped".into()))
            .context("Could not finish upload");
        assert!(file_error(closed).starts_with("The file connection closed"));
        assert_eq!(
            file_error(Error::UnexpectedBehavior("bad packet".into()).into()),
            "bad packet"
        );
        assert_eq!(
            file_error(anyhow::anyhow!("File operation cancelled")),
            "File operation cancelled"
        );
        assert_eq!(
            file_error(anyhow::anyhow!("Permission denied")),
            "Permission denied"
        );
    }

    fn fixture() -> (tempfile::TempDir, Shared, Uuid, Value) {
        let directory = tempfile::tempdir().unwrap();
        let mut manager = Manager::new(Store::for_test(directory.path().into()));
        let server = Uuid::new_v4();
        manager.config.servers.push(
            serde_json::from_value(json!({
                "id": server, "name": "Fixture", "sshHost": "unconnected.invalid",
                "sshUser": "fixture", "sshPort": 22
            }))
            .unwrap(),
        );
        let connection = scope(&manager);
        (
            directory,
            crate::manager::share(manager),
            server,
            connection,
        )
    }

    fn scope(manager: &Manager) -> Value {
        let snapshot = serde_json::to_value(manager.snapshot()).unwrap();
        newport_domain::connection_scope::connection(&snapshot["config"]["servers"][0], &snapshot)
    }

    async fn rejected_before_work(
        state: &Shared,
        server: Uuid,
        connection: &Value,
        expected_error: &str,
    ) {
        let operations = Operations::default();
        let result = operations
            .run_with_connection::<(), _>(
                state,
                server,
                Uuid::new_v4(),
                Some(connection),
                async |_| panic!("Rejected handover must not open a dialog or start transfer work"),
            )
            .await;
        assert!(result.unwrap_err().contains(expected_error));
        assert!(operations.registry.lock().unwrap().entries.is_empty());
    }

    #[tokio::test]
    async fn stale_connection_fields_reject_before_registration_or_work() {
        for (field, value) in [
            ("sshHost", json!("replacement.invalid")),
            ("sshUser", json!("replacement")),
            ("sshPort", json!(2222)),
            ("authMethod", json!("password")),
            ("identityFile", json!("/unused/fixture-key")),
            ("agentSource", json!("replacement")),
            ("agentKeyFingerprint", json!("replacement")),
        ] {
            let (_directory, state, server, connection) = fixture();
            {
                let mut manager = state.lock().await;
                let mut profile = serde_json::to_value(&manager.config.servers[0]).unwrap();
                profile[field] = value;
                manager.config.servers[0] = serde_json::from_value(profile).unwrap();
            }
            rejected_before_work(&state, server, &connection, "server connection changed").await;
        }
    }

    #[tokio::test]
    async fn changed_revision_and_backend_instance_reject_before_work() {
        let (_directory, state, server, connection) = fixture();
        state.lock().await.advance_connection_revision(server);
        rejected_before_work(&state, server, &connection, "server connection changed").await;

        // Keep the entire saved profile unchanged while replacing the backend.
        let (directory, state, server, connection) = fixture();
        let mut replacement = Manager::new(Store::for_test(directory.path().into()));
        replacement.config = state.lock().await.config.clone();
        let restarted = crate::manager::share(replacement);
        rejected_before_work(&restarted, server, &connection, "server connection changed").await;
    }

    #[tokio::test]
    async fn unavailable_deleted_and_invalid_connection_reject_before_work() {
        let (_directory, state, server, connection) = fixture();
        state.lock().await.load_error = Some("fixture config load failure".into());
        rejected_before_work(&state, server, &connection, "configuration is unavailable").await;
        state.lock().await.load_error = None;
        rejected_before_work(&state, server, &json!({}), "server connection changed").await;
        state.lock().await.config.servers.clear();
        rejected_before_work(&state, server, &connection, "Server no longer exists").await;
    }

    #[tokio::test]
    async fn matching_scope_and_presentation_changes_allow_work_and_unscoped_reads() {
        let (_directory, state, server, connection) = fixture();
        let operations = Operations::default();
        for presentation_change in [false, true] {
            if presentation_change {
                let mut manager = state.lock().await;
                let profile = &mut manager.config.servers[0];
                profile.name = "Renamed fixture".into();
                profile.clipboard_enabled = true;
                profile.browser_enabled = true;
            }
            let id = Uuid::new_v4();
            let captured = operations
                .run_with_connection(&state, server, id, Some(&connection), async |profile| {
                    assert!(
                        operations
                            .registry
                            .lock()
                            .unwrap()
                            .entries
                            .contains_key(&id)
                    );
                    assert!(
                        state.try_lock().is_ok(),
                        "Work must release the manager lock"
                    );
                    Ok(profile)
                })
                .await
                .unwrap();
            assert_eq!(captured.id, server);
            assert_eq!(captured.clipboard_enabled, presentation_change);
            assert!(operations.registry.lock().unwrap().entries.is_empty());
        }

        state.lock().await.config.servers[0].ssh_host = "replacement.invalid".into();
        let captured = operations
            .run(&state, server, Uuid::new_v4(), async |profile| Ok(profile))
            .await
            .unwrap();
        assert_eq!(captured.ssh_host, "replacement.invalid");
    }

    #[tokio::test]
    async fn file_work_does_not_wait_for_profile_writer_and_retirement_cancels_it() {
        let (_directory, state, server, connection) = fixture();
        let operations = Operations::default();
        let mut manager = state.lock().await;
        let (started, ready) = tokio::sync::oneshot::channel();
        let operation = operations.run_with_connection::<(), _>(
            &state,
            server,
            Uuid::new_v4(),
            Some(&connection),
            async |_| {
                started.send(()).unwrap();
                std::future::pending().await
            },
        );
        tokio::pin!(operation);
        tokio::select! {
            result = &mut operation => panic!("work must stay pending: {result:?}"),
            result = tokio::time::timeout(std::time::Duration::from_secs(1), ready) => {
                result.expect("registration must not wait for the profile writer").unwrap();
            }
        }
        manager.config.servers[0].ssh_host = "replacement.invalid".into();
        drop(manager);
        assert_eq!(operation.await.unwrap_err(), "File operation cancelled");
        assert!(operations.registry.lock().unwrap().entries.is_empty());
        rejected_before_work(&state, server, &connection, "server connection changed").await;
    }

    #[tokio::test]
    async fn profile_change_cancels_registered_work_with_its_captured_profile() {
        let (_directory, state, server, connection) = fixture();
        let operations = Operations::default();
        let id = Uuid::new_v4();
        let (started, ready) = tokio::sync::oneshot::channel();
        let work = operations.run_with_connection::<(), _>(
            &state,
            server,
            id,
            Some(&connection),
            async |profile| {
                started.send(profile).unwrap();
                std::future::pending().await
            },
        );
        let edit = async {
            let captured = ready.await.unwrap();
            assert_eq!(captured.ssh_host, "unconnected.invalid");
            let mut manager = state.lock().await;
            assert!(
                operations
                    .registry
                    .lock()
                    .unwrap()
                    .entries
                    .contains_key(&id)
            );
            manager.config.servers[0].ssh_host = "replacement.invalid".into();
            manager.advance_connection_revision(server);
            operations.cancel_server(Some(server));
        };
        let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            tokio::join!(work, edit)
        })
        .await
        .unwrap();
        assert_eq!(result.unwrap_err(), "File operation cancelled");
        assert!(operations.registry.lock().unwrap().entries.is_empty());
    }

    #[tokio::test]
    async fn shutdown_waits_for_cleanup_registered_by_cancelled_operation() {
        let operations = Arc::new(Operations::default());
        let id = Uuid::new_v4();
        let (cancel, mut cancelled) = watch::channel(false);
        operations.registry.lock().unwrap().entries.insert(
            id,
            Operation {
                server: Uuid::new_v4(),
                cancel,
                progress: Progress::default(),
            },
        );
        let (started, ready) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel();
        let work = operations.clone();
        let operation = tokio::spawn(async move {
            let _guard = Guard(&work, id);
            cancelled.changed().await.unwrap();
            work.cleanup(async move {
                started.send(()).unwrap();
                released.await.unwrap();
            });
        });
        let work = operations.clone();
        let shutdown = tokio::spawn(async move { work.shutdown().await });
        tokio::time::timeout(std::time::Duration::from_secs(1), ready)
            .await
            .unwrap()
            .unwrap();
        assert!(operations.registry.lock().unwrap().closing);
        assert!(!shutdown.is_finished());
        release.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), shutdown)
            .await
            .unwrap()
            .unwrap();
        operation.await.unwrap();
        assert!(operations.registry.lock().unwrap().entries.is_empty());
        operations.shutdown().await;
    }
}
