use super::{canonical, child, path_valid, status, Operations};
use crate::{manager::Shared, ssh::sftp::Sftp};
use anyhow::{bail, Context, Result};
use russh_sftp::protocol::{FileAttributes, OpenFlags, StatusCode};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tauri::State;
use tauri_plugin_dialog::DialogExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

// Cancellation drops the operation future. Keep its transport alive just long
// enough to remove the staging file; never publish an incomplete upload.
struct RemoteTemp {
    sftp: Arc<Sftp>,
    path: Option<String>,
}
impl Drop for RemoteTemp {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let sftp = self.sftp.clone();
            tokio::spawn(async move {
                if !matches!(
                    tokio::time::timeout(Duration::from_secs(3), sftp.session.remove(&path)).await,
                    Ok(Ok(_))
                ) {
                    log::warn!("Could not remove an incomplete SFTP staging file; it may require manual cleanup");
                }
            });
        }
    }
}
async fn choose(app: &tauri::AppHandle, save: Option<&str>) -> Result<Option<PathBuf>> {
    let (send, receive) = tokio::sync::oneshot::channel();
    let dialog = app.dialog().file();
    let callback = move |path: Option<tauri_plugin_dialog::FilePath>| {
        let _ = send.send(path);
    };
    if let Some(name) = save {
        dialog
            .set_title("Download file")
            .set_file_name(name)
            .save_file(callback);
    } else {
        dialog.set_title("Upload file").pick_file(callback);
    }
    receive
        .await
        .context("File dialog closed unexpectedly")?
        .map(|path| path.into_path().map_err(anyhow::Error::msg))
        .transpose()
}
pub async fn upload(
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
    let staging = child(&folder, &format!(".porthop-upload-{}", Uuid::new_v4()))?;
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
        .await?
        .handle;
    let mut cleanup = RemoteTemp {
        sftp: sftp.clone(),
        path: Some(staging.clone()),
    };
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
    cleanup.path = None;
    Ok(destination)
}
pub async fn download(
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
    let staging = tempfile::NamedTempFile::new_in(parent)?;
    let mut destination = tokio::fs::File::from_std(staging.reopen()?);
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
    staging.persist(local).map_err(|error| error.error)?;
    Ok(())
}
#[tauri::command]
pub async fn files_upload(
    app: tauri::AppHandle,
    state: State<'_, Shared>,
    operations: State<'_, Operations>,
    id: Uuid,
    operation: Uuid,
    path: String,
) -> Result<Option<String>, String> {
    operations
        .run(&state, id, operation, async |server| {
            let Some(local) = choose(&app, None).await? else {
                return Ok(None);
            };
            let name = local
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let sftp = Arc::new(Sftp::connect(&server).await?);
            upload(sftp, &local, &path, |done, total| {
                operations.progress(operation, &name, done, total)
            })
            .await
            .map(Some)
        })
        .await
}
#[tauri::command]
pub async fn files_download(
    app: tauri::AppHandle,
    state: State<'_, Shared>,
    operations: State<'_, Operations>,
    id: Uuid,
    operation: Uuid,
    path: String,
) -> Result<Option<String>, String> {
    operations
        .run(&state, id, operation, async |server| {
            path_valid(&path)?;
            let name = path
                .rsplit('/')
                .next()
                .filter(|name| !name.is_empty())
                .context("Choose a file to download")?;
            let Some(local) = choose(&app, Some(name)).await? else {
                return Ok(None);
            };
            let sftp = Sftp::connect(&server).await?;
            download(&sftp, &path, &local, |done, total| {
                operations.progress(operation, name, done, total)
            })
            .await?;
            Ok(Some(local.to_string_lossy().into_owned()))
        })
        .await
}
