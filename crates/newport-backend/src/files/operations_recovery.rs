//! Recovery I/O must not own the operation registry or block Tokio workers.
//! Staging results retain cleanup ownership if their awaiting transfer is cancelled.
use super::super::{
    cleanup::UploadCleanup,
    download_cleanup::DownloadCleanup,
    recovery_storage::{Owners, scope_server},
};
use super::{super::Remote, Operations, Registry};
use anyhow::{Context, Result};
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::oneshot;
use uuid::Uuid;

/// How long cleanup may wait for the server's shared SFTP session.
const CLEANUP_SESSION: Duration = Duration::from_secs(10);
/// How long removing one staging file may take.
const CLEANUP_REMOVE: Duration = Duration::from_secs(3);

pub(in crate::files) struct Upload {
    id: Uuid,
    journal: Arc<Mutex<UploadCleanup>>,
}
struct UploadRegistration {
    operations: Operations,
    record: Option<Upload>,
}

impl Drop for UploadRegistration {
    fn drop(&mut self) {
        if let Some(record) = self.record.take() {
            // The registration has not reached remote OPEN. No remote file
            // exists, including when cancellation abandoned the worker's reply.
            self.operations.finish_upload(record);
        }
    }
}

pub(in crate::files) struct LocalTemp {
    operations: Operations,
    id: Uuid,
    journal: Arc<Mutex<DownloadCleanup>>,
    pub(in crate::files) staging: Option<tempfile::NamedTempFile>,
}
impl LocalTemp {
    fn remove_staging(&mut self) -> Option<oneshot::Receiver<Result<(), tokio::task::JoinError>>> {
        let file = self.staging.take()?;
        let downloads = self.journal.clone();
        let id = self.id;
        Some(self.operations.start_blocking(move || {
            // Keep the record if the selected volume is unavailable. A
            // restart can retry removal, but never publishes this payload.
            match file.close() {
                Ok(()) => downloads.lock().unwrap().finish(id),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    downloads.lock().unwrap().finish(id);
                }
                Err(error) => log::warn!("Could not remove download staging: {error}"),
            }
        }))
    }

    pub(in crate::files) async fn finish(&self) {
        let journal = self.journal.clone();
        let id = self.id;
        let _ = self
            .operations
            .start_blocking(move || journal.lock().unwrap().finish(id))
            .await;
    }

    pub(in crate::files) async fn discard(&mut self) {
        if let Some(worker) = self.remove_staging() {
            let _ = worker.await;
        }
    }
}
impl Drop for LocalTemp {
    fn drop(&mut self) {
        self.remove_staging();
    }
}

impl Operations {
    pub fn persistent(profile_directory: &Path) -> Self {
        Self {
            editor_writes: Default::default(),
            registry: Arc::new(Mutex::new(Registry::default())),
            uploads: Arc::new(Owners::open(profile_directory.join("upload-recovery.json"))),
            downloads: Arc::new(Owners::open(
                profile_directory.join("download-recovery.json"),
            )),
        }
    }

    fn start_blocking<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> oneshot::Receiver<Result<T, tokio::task::JoinError>> {
        let (send, receive) = oneshot::channel();
        // Track the worker independently of its waiter. Shutdown must wait for
        // durable publication and cleanup even after the transfer was cancelled.
        self.cleanup(async move {
            let result = tokio::task::spawn_blocking(work).await;
            if let Err(error) = &result {
                log::warn!("File recovery worker failed: {error}");
            }
            let _ = send.send(result);
        });
        receive
    }

    pub(in crate::files) async fn track_upload(
        &self,
        scope: serde_json::Value,
        path: String,
    ) -> Result<Upload> {
        let operations = self.clone();
        let mut record = self
            .start_blocking(move || {
                let journal = operations.uploads.get(scope_server(&scope)?)?;
                let id = journal.lock().unwrap().register(scope, path)?;
                Ok::<_, anyhow::Error>(UploadRegistration {
                    operations,
                    record: Some(Upload { id, journal }),
                })
            })
            .await
            .context("Upload recovery worker stopped")???;
        Ok(record.record.take().unwrap())
    }

    pub(in crate::files) fn finish_upload(
        &self,
        record: Upload,
    ) -> oneshot::Receiver<Result<(), tokio::task::JoinError>> {
        self.start_blocking(move || record.journal.lock().unwrap().finish(record.id, true))
    }

    pub(in crate::files) async fn stage_download(
        &self,
        server: Uuid,
        parent: &Path,
    ) -> Result<LocalTemp> {
        let operations = self.clone();
        let parent = parent.to_owned();
        self.start_blocking(move || {
            let journal = operations.downloads.get(server)?;
            let (id, staging) = journal.lock().unwrap().create(&parent)?;
            Ok(LocalTemp {
                operations,
                id,
                journal,
                staging: Some(staging),
            })
        })
        .await
        .context("Download recovery worker stopped")??
    }

    pub(in crate::files) fn cleanup_upload(&self, remote: Remote, record: Upload, created: bool) {
        // An interrupted OPEN may still create the file. Leave that entry
        // excluded until restart, when the old channel is known to be closed.
        if created {
            self.schedule_upload_cleanup(remote, Some(record));
        }
    }

    /// Removes this server's staging files left by uploads that didn't
    /// finish.
    pub fn retry_upload_cleanup(&self, remote: Remote) {
        self.schedule_upload_cleanup(remote, None);
    }

    fn schedule_upload_cleanup(&self, remote: Remote, release: Option<Upload>) {
        let operations = self.clone();
        self.cleanup(async move {
            let owners = operations.uploads.clone();
            let server = remote.server_id();
            let scope = match remote.cleanup_scope() {
                Ok(scope) => scope,
                Err(error) => {
                    log::warn!("Could not scope this server's upload recovery: {error:#}");
                    return;
                }
            };
            let pending = operations
                .start_blocking(move || {
                    let journal = match &release {
                        Some(record) => record.journal.clone(),
                        None => owners.get(server)?,
                    };
                    let pending = {
                        let mut uploads = journal.lock().unwrap();
                        if let Some(record) = release {
                            uploads.release(record.id);
                        }
                        uploads.take(&scope)
                    };
                    Ok::<_, anyhow::Error>((journal, pending))
                })
                .await
                .context("Upload recovery worker stopped")
                .and_then(|result| result.context("Upload recovery worker failed"))
                .and_then(|result| result);
            let (journal, pending) = match pending {
                Ok(pending) => pending,
                Err(error) => {
                    log::warn!("Could not load this server's upload recovery: {error:#}");
                    return;
                }
            };
            if pending.is_empty() {
                return;
            }
            let session = match tokio::time::timeout(CLEANUP_SESSION, remote.sftp.shared()).await {
                Ok(Ok(session)) => Arc::new(session),
                _ => {
                    log::warn!("SFTP staging cleanup deferred until the next file connection");
                    let _ = tokio::task::spawn_blocking(move || {
                        let mut uploads = journal.lock().unwrap();
                        for (id, _) in pending {
                            uploads.finish(id, false);
                        }
                    })
                    .await;
                    return;
                }
            };
            for (id, path) in pending {
                let uploads = journal.clone();
                let session = session.clone();
                // The session's channel lives only as long as its link.
                let remote = remote.clone();
                operations.cleanup(async move {
                    let _link = remote;
                    let removed =
                        match tokio::time::timeout(CLEANUP_REMOVE, session.remove(&path)).await {
                            Ok(Ok(_)) => true,
                            Ok(Err(error)) => super::super::status(
                                &error,
                                russh_sftp::protocol::StatusCode::NoSuchFile,
                            ),
                            Err(_) => false,
                        };
                    let _ = tokio::task::spawn_blocking(move || {
                        uploads.lock().unwrap().finish(id, removed)
                    })
                    .await;
                    if !removed {
                        log::warn!("SFTP staging cleanup deferred until the next file connection");
                    }
                });
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Operation, Progress, files_cancel, files_progress};
    use super::*;
    use serde_json::{Value, json};
    use std::time::Duration;

    // Recovery does real durable file I/O, which can be slow on busy CI hosts.
    // The stall guards below remain locked until released, so this is only a
    // deadlock deadline, not a filesystem performance assertion.
    const RECOVERY_TIMEOUT: Duration = Duration::from_secs(30);

    struct Release(Option<std::sync::mpsc::Sender<()>>);
    impl Drop for Release {
        fn drop(&mut self) {
            if let Some(send) = self.0.take() {
                let _ = send.send(());
            }
        }
    }

    async fn stall<T: Send + 'static>(operations: &Operations, queue: Arc<Mutex<T>>) -> Release {
        let (release, wait) = std::sync::mpsc::channel();
        let release = Release(Some(release));
        let (started, ready) = oneshot::channel();
        operations.start_blocking(move || {
            let _guard = queue.lock().unwrap();
            let _ = started.send(());
            // Release also runs during panic unwinding, before runtime shutdown.
            // Never let elapsed time unlock a journal and hide a shared lock.
            let _ = wait.recv();
        });
        ready.await.unwrap();
        release
    }

    async fn poll_once<F: std::future::Future>(future: std::pin::Pin<&mut F>) {
        tokio::select! {
            biased;
            _ = future => panic!("stalled recovery must not finish"),
            _ = std::future::ready(()) => {},
        }
    }

    fn server() -> Uuid {
        Uuid::from_u128(1)
    }

    fn entries(profile: &Path, file: &str) -> Value {
        serde_json::from_slice(
            &std::fs::read(
                super::super::super::recovery_storage::directory(&profile.join(file))
                    .join(server().to_string())
                    .join("journal.json"),
            )
            .unwrap(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn stalled_upload_journal_keeps_progress_cancellation_and_downloads_responsive() {
        let profile = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let operations = Operations::persistent(profile.path());
        let release = stall(&operations, operations.uploads.get(server()).unwrap()).await;
        let id = Uuid::new_v4();
        let (cancel, cancelled) = tokio::sync::watch::channel(false);
        operations.registry.lock().unwrap().entries.insert(
            id,
            Operation {
                server: Uuid::new_v4(),
                cancel,
                progress: Progress::default(),
            },
        );
        operations.progress(id, "payload", 10, Some(20));
        assert_eq!(files_progress(&operations, id).unwrap().completed, 10);
        files_cancel(&operations, id);
        assert!(*cancelled.borrow());
        let download = tokio::time::timeout(
            RECOVERY_TIMEOUT,
            operations.stage_download(server(), folder.path()),
        )
        .await
        .expect("download staging must not wait for the upload journal")
        .unwrap();
        drop(download);
        operations.registry.lock().unwrap().entries.remove(&id);
        drop(release);
        operations.shutdown().await;
        assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn cancelled_upload_registration_is_retired_after_its_worker_finishes() {
        let profile = tempfile::tempdir().unwrap();
        let operations = Operations::persistent(profile.path());
        let release = stall(&operations, operations.uploads.get(server()).unwrap()).await;
        let scope = json!([server()]);
        let mut request = Box::pin(operations.track_upload(
            scope.clone(),
            format!("/work/.newport-upload-{}", Uuid::new_v4()),
        ));
        poll_once(request.as_mut()).await;
        drop(request);
        drop(release);
        operations.shutdown().await;
        assert!(
            operations
                .uploads
                .get(server())
                .unwrap()
                .lock()
                .unwrap()
                .take(&scope)
                .is_empty()
        );
        assert!(
            entries(profile.path(), "upload-recovery.json")["entries"]
                .as_object()
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn cancelled_download_staging_removes_file_and_record_after_worker_finishes() {
        let profile = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let operations = Operations::persistent(profile.path());
        let release = stall(&operations, operations.downloads.get(server()).unwrap()).await;
        let mut request = Box::pin(operations.stage_download(server(), folder.path()));
        poll_once(request.as_mut()).await;
        drop(request);
        drop(release);
        operations.shutdown().await;
        assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 0);
        assert!(
            entries(profile.path(), "download-recovery.json")["entries"]
                .as_object()
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn acknowledged_upload_record_stays_active_until_explicit_retirement() {
        let profile = tempfile::tempdir().unwrap();
        let operations = Operations::persistent(profile.path());
        let scope = json!([server()]);
        let id = operations
            .track_upload(
                scope.clone(),
                format!("/work/.newport-upload-{}", Uuid::new_v4()),
            )
            .await
            .unwrap();
        assert!(
            operations
                .uploads
                .get(server())
                .unwrap()
                .lock()
                .unwrap()
                .take(&scope)
                .is_empty()
        );
        assert_eq!(
            entries(profile.path(), "upload-recovery.json")["entries"]
                .as_object()
                .unwrap()
                .len(),
            1
        );
        operations.finish_upload(id).await.unwrap().unwrap();
        assert!(
            entries(profile.path(), "upload-recovery.json")["entries"]
                .as_object()
                .unwrap()
                .is_empty()
        );
        operations.shutdown().await;
    }

    #[tokio::test]
    async fn abandoned_completed_worker_replies_still_release_both_staging_owners() {
        let profile = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let operations = Operations::persistent(profile.path());
        let mut upload = Box::pin(operations.track_upload(
            json!([server()]),
            format!("/work/.newport-upload-{}", Uuid::new_v4()),
        ));
        let mut download = Box::pin(operations.stage_download(server(), folder.path()));
        poll_once(upload.as_mut()).await;
        poll_once(download.as_mut()).await;
        tokio::time::timeout(RECOVERY_TIMEOUT, async {
            while !operations
                .registry
                .lock()
                .unwrap()
                .cleanup
                .iter()
                .all(|task| task.is_finished())
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        // Both replies are queued, but their callers have not accepted them.
        assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 1);
        drop(upload);
        drop(download);
        operations.shutdown().await;
        assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 0);
        for journal in ["upload-recovery.json", "download-recovery.json"] {
            assert!(
                entries(profile.path(), journal)["entries"]
                    .as_object()
                    .unwrap()
                    .is_empty()
            );
        }
    }

    #[tokio::test]
    async fn stalled_server_journals_do_not_queue_another_servers_transfers() {
        let profile = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let operations = Operations::persistent(profile.path());
        let upload_gate = stall(&operations, operations.uploads.get(server()).unwrap()).await;
        let download_gate = stall(&operations, operations.downloads.get(server()).unwrap()).await;
        let other = Uuid::new_v4();
        tokio::time::timeout(RECOVERY_TIMEOUT, async {
            let record = operations
                .track_upload(
                    json!([other]),
                    format!("/work/.newport-upload-{}", Uuid::new_v4()),
                )
                .await
                .unwrap();
            let mut download = operations
                .stage_download(other, folder.path())
                .await
                .unwrap();
            operations.finish_upload(record).await.unwrap().unwrap();
            download.discard().await;
        })
        .await
        .expect("another server must not wait on either stalled journal");
        drop(upload_gate);
        drop(download_gate);
        operations.shutdown().await;
        assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 0);
    }
}
