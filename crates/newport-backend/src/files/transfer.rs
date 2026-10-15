use super::{Operations, canonical, child, path_valid, status};
use crate::{manager::Shared, ssh::sftp::Sftp};
use anyhow::{Context, Result, bail};
use russh_sftp::protocol::{FileAttributes, OpenFlags, StatusCode};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

// Invalidating on drop also covers a cancelled or uncertain upload.
struct ListingInvalidation(Arc<crate::server_session::ServerSession>);
impl Drop for ListingInvalidation {
    fn drop(&mut self) {
        self.0.queries.files.invalidate(|_| true);
    }
}

/// Private native-client contract. The UI owns the picker; this process owns
/// connection validation and the atomic transfer. OsString preserves local paths
/// which cannot be represented as UTF-8, including download destinations.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeTransfer {
    id: Uuid,
    operation: Uuid,
    path: String,
    local_path: std::ffi::OsString,
    expected_connection: Vec<serde_json::Value>,
}

pub async fn native_transfer(
    state: &Shared,
    operations: &Operations,
    request: NativeTransfer,
    uploading: bool,
) -> Result<Option<String>, String> {
    let local = PathBuf::from(request.local_path);
    if !local.is_absolute() {
        return Err("A selected local path and captured connection are required".into());
    }
    let _invalidate = if uploading {
        Some(ListingInvalidation(
            crate::server_session::resolve(state, request.id).await?,
        ))
    } else {
        None
    };
    let expected_connection = serde_json::Value::Array(request.expected_connection);
    operations
        .run_with_connection(
            state,
            request.id,
            request.operation,
            Some(&expected_connection),
            async |server| {
                path_valid(&request.path)?;
                let sftp = Arc::new(Sftp::connect(&server).await?);
                operations.retry_upload_cleanup(sftp.clone());
                let name = if uploading {
                    local
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                } else {
                    request
                        .path
                        .rsplit('/')
                        .next()
                        .unwrap_or_default()
                        .to_owned()
                };
                let progress =
                    |done, total| operations.progress(request.operation, &name, done, total);
                if uploading {
                    upload(operations, sftp, &local, &request.path, progress)
                        .await
                        .map(Some)
                } else {
                    download(operations, &sftp, &request.path, &local, progress).await?;
                    Ok(Some(local.to_string_lossy().into_owned()))
                }
            },
        )
        .await
}

// Cancellation drops the operation future. Keep its transport alive just long
// enough to remove the staging file; never publish an incomplete upload.
struct RemoteTemp<'a> {
    operations: &'a Operations,
    sftp: Arc<Sftp>,
    record: Option<super::operations::recovery::Upload>,
    created: bool,
}
impl Drop for RemoteTemp<'_> {
    fn drop(&mut self) {
        if let Some(record) = self.record.take() {
            self.operations
                .cleanup_upload(self.sftp.clone(), record, self.created);
        }
    }
}

/// Desktop adapters own selection and replacement consent; cancellation drops
/// this future before any transfer starts. Native callers supply their picked path.
pub trait FilePicker: Send + Sync {
    fn choose(
        &self,
        save: Option<&str>,
    ) -> impl std::future::Future<Output = Result<Option<PathBuf>>> + Send;
}
pub async fn upload(
    operations: &Operations,
    sftp: Arc<Sftp>,
    local: &Path,
    folder: &str,
    progress: impl Fn(u64, Option<u64>),
) -> Result<String> {
    let name = local
        .file_name()
        .and_then(|name| name.to_str())
        .context("File name must be valid Unicode")?;
    let folder = canonical(&sftp, folder).await?;
    let destination = child(&folder, name)?;
    match sftp.session.lstat(&destination).await {
        Ok(_) => bail!(
            "A remote item named {name} already exists. Rename your local file before uploading."
        ),
        Err(e) if status(&e, StatusCode::NoSuchFile) => {}
        Err(e) => return Err(e.into()),
    }
    let mut source = tokio::fs::File::open(local).await?;
    let metadata = source.metadata().await?;
    if !metadata.is_file() {
        bail!("Choose a regular file to upload");
    }
    let total = metadata.len();
    let staging = child(&folder, &format!(".newport-upload-{}", Uuid::new_v4()))?;
    let record = operations
        .track_upload(sftp.cleanup_scope().clone(), staging.clone())
        .await?;
    let mut cleanup = RemoteTemp {
        operations,
        sftp: sftp.clone(),
        record: Some(record),
        created: false,
    };
    let handle = sftp
        .session
        .open(
            &staging,
            OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::EXCLUDE,
            FileAttributes {
                permissions: Some(0o600),
                ..FileAttributes::empty()
            },
        )
        .await;
    let handle = match handle {
        Ok(handle) => handle.handle,
        Err(error) => {
            // A definitive negative reply means EXCLUDE did not create our
            // file. In particular, never remove an already-existing path.
            if matches!(error, russh_sftp::client::error::Error::Status(_)) {
                let _ = operations
                    .finish_upload(cleanup.record.take().unwrap())
                    .await;
            }
            return Err(error.into());
        }
    };
    cleanup.created = true;
    let mut offset = 0;
    let mut buffer = vec![0; 32 * 1024];
    progress(0, Some(total));
    loop {
        let count = source.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        sftp.session
            .write(&handle, offset, buffer[..count].to_vec())
            .await?;
        offset += count as u64;
        progress(offset, Some(total));
    }
    sftp.session.close(handle).await?;
    if offset != total {
        bail!("Local file changed during upload. Please try again.");
    }
    // Standard SFTP v3 rename must fail when the destination already exists.
    sftp.session
        .rename(&staging, &destination)
        .await
        .context("Could not finish upload; destination may already exist")?;
    let _ = operations
        .finish_upload(cleanup.record.take().unwrap())
        .await;
    Ok(destination)
}
pub async fn download(
    operations: &Operations,
    sftp: &Sftp,
    remote: &str,
    local: &Path,
    progress: impl Fn(u64, Option<u64>),
) -> Result<()> {
    path_valid(remote)?;
    let attrs = sftp.session.stat(remote).await?.attrs;
    if !attrs.is_regular() {
        bail!("Only regular files can be downloaded");
    }
    let parent = local
        .parent()
        .context("Download destination has no parent folder")?;
    let mut cleanup = operations.stage_download(sftp.server_id(), parent).await?;
    let result = async {
        let mut destination =
            tokio::fs::File::from_std(cleanup.staging.as_ref().unwrap().reopen()?);
        let handle = sftp
            .session
            .open(remote, OpenFlags::READ, FileAttributes::empty())
            .await?
            .handle;
        let mut offset = 0;
        progress(offset, attrs.size);
        loop {
            match sftp.session.read(&handle, offset, 32 * 1024).await {
                Ok(data) if data.data.is_empty() => break,
                Ok(data) => {
                    destination.write_all(&data.data).await?;
                    offset += data.data.len() as u64;
                    progress(offset, attrs.size);
                }
                Err(e) if status(&e, StatusCode::Eof) => break,
                Err(e) => return Err(e.into()),
            }
        }
        sftp.session.close(handle).await?;
        let after = sftp.session.stat(remote).await?.attrs;
        if attrs.size.is_some_and(|size| size != offset)
            || attrs.size != after.size
            || attrs.mtime != after.mtime
        {
            bail!("Remote file changed during download. Please try again.");
        }
        destination.sync_all().await?;
        drop(destination);
        // The native save dialog owns destination selection and replacement consent.
        match cleanup.staging.take().unwrap().persist(local) {
            Ok(_) => {
                cleanup.finish().await;
            }
            Err(error) => {
                cleanup.staging = Some(error.file);
                return Err(error.error.into());
            }
        }
        Ok(())
    }
    .await;
    if result.is_err() {
        cleanup.discard().await;
    }
    result
}

pub async fn files_upload(
    picker: &impl FilePicker,
    state: &Shared,
    operations: &Operations,
    id: Uuid,
    operation: Uuid,
    path: String,
    expected_connection: Option<serde_json::Value>,
) -> Result<Option<String>, String> {
    let _invalidate = ListingInvalidation(crate::server_session::resolve(state, id).await?);
    operations
        .run_with_connection(
            state,
            id,
            operation,
            expected_connection.as_ref(),
            async |server| {
                let Some(local) = picker.choose(None).await? else {
                    return Ok(None);
                };
                let name = local
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                let sftp = Arc::new(Sftp::connect(&server).await?);
                operations.retry_upload_cleanup(sftp.clone());
                upload(operations, sftp, &local, &path, |done, total| {
                    operations.progress(operation, &name, done, total)
                })
                .await
                .map(Some)
            },
        )
        .await
}
pub async fn files_download(
    picker: &impl FilePicker,
    state: &Shared,
    operations: &Operations,
    id: Uuid,
    operation: Uuid,
    path: String,
    expected_connection: Option<serde_json::Value>,
) -> Result<Option<String>, String> {
    operations
        .run_with_connection(
            state,
            id,
            operation,
            expected_connection.as_ref(),
            async |server| {
                path_valid(&path)?;
                let name = path
                    .rsplit('/')
                    .next()
                    .filter(|name| !name.is_empty())
                    .context("Choose a file to download")?;
                let Some(local) = picker.choose(Some(name)).await? else {
                    return Ok(None);
                };
                let sftp = Arc::new(Sftp::connect(&server).await?);
                operations.retry_upload_cleanup(sftp.clone());
                download(operations, &sftp, &path, &local, |done, total| {
                    operations.progress(operation, name, done, total)
                })
                .await?;
                Ok(Some(local.to_string_lossy().into_owned()))
            },
        )
        .await
}

#[cfg(test)]
mod native_contract_tests {
    use super::NativeTransfer;
    use serde_json::json;

    #[test]
    fn native_transfer_requires_picker_path_and_connection() {
        let args = json!({
            "id": uuid::Uuid::new_v4(), "operation": uuid::Uuid::new_v4(),
            "path": "/remote/file.txt", "localPath": std::ffi::OsString::from("/tmp/file.txt"),
            "expectedConnection": ["captured"]
        });
        assert!(serde_json::from_value::<NativeTransfer>(args.clone()).is_ok());
        for field in ["localPath", "id", "operation", "path", "expectedConnection"] {
            let mut missing = args.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(
                serde_json::from_value::<NativeTransfer>(missing).is_err(),
                "{field}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn native_path_contract_preserves_non_utf8_directory_names() {
        use std::os::unix::ffi::OsStringExt;
        let path = std::ffi::OsString::from_vec(b"/tmp/\xff/file.txt".to_vec());
        let args = json!({
            "id": uuid::Uuid::new_v4(), "operation": uuid::Uuid::new_v4(),
            "path": "/remote/file.txt", "localPath": path,
            "expectedConnection": ["captured"]
        });
        let parsed: NativeTransfer = serde_json::from_value(args).unwrap();
        assert_eq!(parsed.local_path, path);
    }
}

#[cfg(test)]
mod picker_tests {
    use super::*;
    use crate::{files::operations::files_cancel, manager::Manager, profile::Store};
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    };

    struct CancelPicker(Mutex<Vec<Option<String>>>);
    impl FilePicker for CancelPicker {
        async fn choose(&self, save: Option<&str>) -> Result<Option<PathBuf>> {
            self.0.lock().unwrap().push(save.map(str::to_owned));
            Ok(None)
        }
    }
    fn fixture() -> (tempfile::TempDir, Shared, Uuid, serde_json::Value) {
        let directory = tempfile::tempdir().unwrap();
        let mut manager = Manager::new(Store::for_test(directory.path().into()));
        let id = Uuid::new_v4();
        manager.config.servers.push(serde_json::from_value(serde_json::json!({
            "id":id,"name":"Picker fixture","sshHost":"127.0.0.1","sshPort":9,"sshUser":"fixture"
        })).unwrap());
        let snapshot = serde_json::to_value(manager.snapshot()).unwrap();
        let scope = newport_domain::connection_scope::connection(
            &snapshot["config"]["servers"][0],
            &snapshot,
        );
        (directory, crate::manager::share(manager), id, scope)
    }

    #[tokio::test]
    async fn dismissed_pickers_release_operations_and_stale_requests_never_open_them() {
        let (_directory, state, id, scope) = fixture();
        let operations = Operations::default();
        let picker = CancelPicker(Mutex::new(Vec::new()));
        let operation = Uuid::new_v4();
        assert_eq!(
            files_upload(
                &picker,
                &state,
                &operations,
                id,
                operation,
                "/remote".into(),
                Some(scope.clone())
            )
            .await
            .unwrap(),
            None
        );
        // Reusing the id proves the dismissed dialog released its registration.
        assert_eq!(
            files_download(
                &picker,
                &state,
                &operations,
                id,
                operation,
                "/remote/file.txt".into(),
                Some(scope.clone())
            )
            .await
            .unwrap(),
            None
        );
        state.lock().await.advance_connection_revision(id);
        let error = files_upload(
            &picker,
            &state,
            &operations,
            id,
            operation,
            "/remote".into(),
            Some(scope),
        )
        .await
        .unwrap_err();
        assert!(error.contains("connection changed"), "{error}");
        assert_eq!(
            *picker.0.lock().unwrap(),
            vec![None, Some("file.txt".into())]
        );
        assert!(crate::files::operations::files_progress(&operations, operation).is_none());
    }

    struct PendingPicker {
        opened: tokio::sync::Notify,
        dropped: AtomicBool,
    }
    struct Waiting<'a>(&'a AtomicBool);
    impl Drop for Waiting<'_> {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    impl FilePicker for PendingPicker {
        async fn choose(&self, _: Option<&str>) -> Result<Option<PathBuf>> {
            let _waiting = Waiting(&self.dropped);
            self.opened.notify_one();
            std::future::pending().await
        }
    }
    #[tokio::test]
    async fn cancelling_while_picker_is_open_drops_selection_and_releases_registration() {
        let (_directory, state, id, scope) = fixture();
        let operations = Operations::default();
        let picker = PendingPicker {
            opened: tokio::sync::Notify::new(),
            dropped: AtomicBool::new(false),
        };
        let operation = Uuid::new_v4();
        let upload = files_upload(
            &picker,
            &state,
            &operations,
            id,
            operation,
            "/remote".into(),
            Some(scope),
        );
        tokio::pin!(upload);
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            tokio::select! {
                result = &mut upload => panic!("Transfer finished before cancellation: {result:?}"),
                _ = picker.opened.notified() => {}
            }
            files_cancel(&operations, operation);
            let error = upload.await.unwrap_err();
            assert!(error.contains("cancelled"), "{error}");
        })
        .await
        .unwrap();
        assert!(picker.dropped.load(Ordering::SeqCst));
        assert!(crate::files::operations::files_progress(&operations, operation).is_none());
    }
}
