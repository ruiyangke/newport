//! Remote file operations. Local paths come exclusively from native dialogs.
pub mod operations;
pub mod transfer;
use crate::{manager::Shared, ssh::sftp::Sftp};
use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
pub use operations::Operations;
use russh_sftp::{
    client::error::Error,
    protocol::{FileAttributes, OpenFlags, StatusCode},
};
use serde::Serialize;
use tauri::State;
use uuid::Uuid;
const PREVIEW_LIMIT: usize = 16 * 1024 * 1024;
const TEXT_LIMIT: usize = 1024 * 1024;
const DIRECTORY_LIMIT: usize = 20_000;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    name: String,
    path: String,
    kind: &'static str,
    size: Option<u64>,
    modified: Option<u32>,
    permissions: Option<String>,
}
#[derive(Debug, Serialize)]
pub struct Listing {
    path: String,
    entries: Vec<Entry>,
}
#[derive(Debug, Serialize)]
pub struct Preview {
    kind: &'static str,
    content: String,
    mime: &'static str,
}

pub fn child(parent: &str, name: &str) -> Result<String> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\0']) {
        bail!("Invalid remote file name");
    }
    Ok(format!("{}/{name}", parent.trim_end_matches('/')))
}
fn path_valid(path: &str) -> Result<()> {
    if path.is_empty() || path.len() > 16_384 || path.contains('\0') {
        bail!("Invalid remote path");
    }
    Ok(())
}
fn status(error: &Error, code: StatusCode) -> bool {
    matches!(error, Error::Status(s) if s.status_code == code)
}
async fn canonical(sftp: &Sftp, path: &str) -> Result<String> {
    path_valid(path)?;
    sftp.session
        .realpath(path)
        .await?
        .files
        .into_iter()
        .next()
        .map(|entry| entry.filename)
        .context("Server returned no canonical path")
}
pub async fn list(sftp: &Sftp, path: &str) -> Result<Listing> {
    let path = canonical(sftp, path).await?;
    let handle = sftp.session.opendir(&path).await?.handle;
    let mut entries = Vec::new();
    let result = async {
        loop {
            let batch = match sftp.session.readdir(&handle).await {
                Ok(batch) => batch,
                Err(e) if status(&e, StatusCode::Eof) => break,
                Err(e) => return Err(e.into()),
            };
            if batch.files.is_empty() {
                bail!("Server returned an empty directory response without finishing");
            }
            for entry in batch.files {
                if entry.filename == "." || entry.filename == ".." {
                    continue;
                }
                if entries.len() == DIRECTORY_LIMIT {
                    bail!("Folder contains more than 20,000 items. Open a subfolder by path.");
                }
                let full = child(&path, &entry.filename)?;
                let attrs = if entry.attrs.is_symlink() {
                    sftp.session
                        .stat(&full)
                        .await
                        .map(|response| response.attrs)
                        .unwrap_or(entry.attrs)
                } else {
                    entry.attrs
                };
                let kind = if attrs.is_dir() {
                    "directory"
                } else if attrs.is_symlink() {
                    "symlink"
                } else if attrs.is_regular() {
                    "file"
                } else {
                    "other"
                };
                entries.push(Entry {
                    name: entry.filename,
                    path: full,
                    kind,
                    size: attrs.size,
                    modified: attrs.mtime,
                    permissions: attrs
                        .permissions
                        .map(|mode| format!("{:03o}", mode & 0o7777)),
                });
            }
        }
        Ok(())
    }
    .await;
    let closed = sftp.session.close(handle).await;
    result?;
    closed?;
    entries.sort_by(|a, b| {
        (a.kind != "directory")
            .cmp(&(b.kind != "directory"))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(Listing { path, entries })
}
pub async fn preview(sftp: &Sftp, path: &str) -> Result<Preview> {
    path_valid(path)?;
    let metadata = sftp.session.stat(path).await?.attrs;
    if !metadata.is_regular() {
        bail!("Only regular files can be previewed");
    }
    if metadata
        .size
        .is_some_and(|size| size > PREVIEW_LIMIT as u64)
    {
        bail!("Preview is limited to 16 MiB. Download this file to open it.");
    }
    let handle = sftp
        .session
        .open(path, OpenFlags::READ, FileAttributes::empty())
        .await?
        .handle;
    let mut bytes = Vec::new();
    loop {
        match sftp
            .session
            .read(&handle, bytes.len() as u64, 32 * 1024)
            .await
        {
            Ok(data) if data.data.is_empty() => break,
            Ok(data) => {
                if bytes.len() + data.data.len() > PREVIEW_LIMIT {
                    bail!("File grew beyond the 16 MiB preview limit. Download it to open it.");
                }
                bytes.extend_from_slice(&data.data);
            }
            Err(e) if status(&e, StatusCode::Eof) => break,
            Err(e) => return Err(e.into()),
        }
    }
    sftp.session.close(handle).await?;
    classify(bytes)
}
fn classify(bytes: Vec<u8>) -> Result<Preview> {
    let mime = if bytes.starts_with(b"%PDF-") {
        "application/pdf"
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        "image/gif"
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        "image/webp"
    } else {
        "text/plain"
    };
    if mime == "text/plain" {
        if bytes.len() > TEXT_LIMIT {
            bail!("Text preview is limited to 1 MiB. Download this file to open it.");
        }
        let content = String::from_utf8(bytes)
            .context("This binary file cannot be previewed. Download it to open it.")?;
        if content
            .chars()
            .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t'))
        {
            bail!("This binary file cannot be previewed. Download it to open it.");
        }
        Ok(Preview {
            kind: "text",
            content,
            mime,
        })
    } else {
        Ok(Preview {
            kind: if mime == "application/pdf" {
                "pdf"
            } else {
                "image"
            },
            content: STANDARD.encode(bytes),
            mime,
        })
    }
}
#[tauri::command]
pub async fn files_list(
    state: State<'_, Shared>,
    operations: State<'_, Operations>,
    id: Uuid,
    operation: Uuid,
    path: String,
) -> Result<Listing, String> {
    operations
        .run(&state, id, operation, async |server| {
            let sftp = Sftp::connect(&server).await?;
            tokio::time::timeout(std::time::Duration::from_secs(60), list(&sftp, &path))
                .await
                .context("Folder listing timed out")?
        })
        .await
}
#[tauri::command]
pub async fn files_preview(
    state: State<'_, Shared>,
    operations: State<'_, Operations>,
    id: Uuid,
    operation: Uuid,
    path: String,
) -> Result<Preview, String> {
    operations
        .run(&state, id, operation, async |server| {
            let sftp = Sftp::connect(&server).await?;
            tokio::time::timeout(std::time::Duration::from_secs(60), preview(&sftp, &path))
                .await
                .context("File preview timed out")?
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remote_paths_are_literal_and_names_cannot_escape() {
        assert_eq!(
            child("/home/a", "hello ' 世界.txt").unwrap(),
            "/home/a/hello ' 世界.txt"
        );
        for name in ["", ".", "..", "a/b", "a\0b"] {
            assert!(child("/tmp", name).is_err());
        }
        assert_eq!(child("/", "tmp").unwrap(), "/tmp");
    }
    #[test]
    fn preview_uses_content_and_never_interprets_markup() {
        assert_eq!(
            classify(b"<script>alert(1)</script>".to_vec())
                .unwrap()
                .kind,
            "text"
        );
        assert_eq!(classify(b"%PDF-1.7\n".to_vec()).unwrap().kind, "pdf");
        assert!(classify(vec![0, 1, 2]).is_err());
        assert!(classify(vec![b'a'; TEXT_LIMIT + 1]).is_err());
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::model::{AuthMethod, Server};
    use std::sync::Arc;
    #[tokio::test]
    #[ignore = "Run with scripts/test-ssh.sh"]
    async fn sftp_browse_preview_and_atomic_transfers() {
        let server = Server {
            id: Uuid::new_v4(),
            name: "Files fixture".into(),
            ssh_user: "fixture".into(),
            ssh_host: "127.0.0.1".into(),
            ssh_port: std::env::var("PORTHOP_TEST_SSH_PORT")
                .unwrap()
                .parse()
                .unwrap(),
            identity_file: Some(std::env::var("PORTHOP_TEST_IDENTITY").unwrap()),
            agent_source: None,
            agent_key_fingerprint: None,
            auth_method: AuthMethod::PublicKey,
            clipboard_enabled: false,
            browser_enabled: false,
        };
        let sftp = Arc::new(Sftp::connect(&server).await.unwrap());
        let root = list(&sftp, ".").await.unwrap();
        assert_eq!(root.path, "/");
        assert!(root
            .entries
            .iter()
            .any(|entry| entry.name == "Documents" && entry.kind == "directory"));
        assert!(preview(&sftp, "/Documents/hello 世界.txt")
            .await
            .unwrap()
            .content
            .contains("<script>inert text</script>"));
        assert!(preview(&sftp, "/binary.dat")
            .await
            .unwrap_err()
            .to_string()
            .contains("binary"));
        assert!(preview(&sftp, "/large.txt")
            .await
            .unwrap_err()
            .to_string()
            .contains("16 MiB"));
        assert!(list(&sftp, "/missing").await.is_err());
        let directory = tempfile::tempdir().unwrap();
        let local = directory.path().join("upload ' 世界.dat");
        let bytes: Vec<u8> = (0..300_000).map(|n| (n % 251) as u8).collect();
        std::fs::write(&local, &bytes).unwrap();
        let remote = transfer::upload(sftp.clone(), &local, "/Documents", |_, _| {})
            .await
            .unwrap();
        assert!(
            transfer::upload(sftp.clone(), &local, "/Documents", |_, _| {})
                .await
                .unwrap_err()
                .to_string()
                .contains("already exists")
        );
        let saved = directory.path().join("download.dat");
        std::fs::write(&saved, b"old bytes").unwrap();
        transfer::download(&sftp, &remote, &saved, |_, _| {})
            .await
            .unwrap();
        assert_eq!(std::fs::read(&saved).unwrap(), bytes);
        assert!(transfer::download(&sftp, "/missing", &saved, |_, _| {})
            .await
            .is_err());
        assert_eq!(std::fs::read(&saved).unwrap(), bytes);
        let folders = list(&sftp, "/Documents").await.unwrap();
        assert!(!folders
            .entries
            .iter()
            .any(|entry| entry.name.starts_with(".porthop-upload")));
        // Dropping the transfer before completion removes the unpublished staging file.
        let cancelled = directory.path().join("cancelled.bin");
        std::fs::write(&cancelled, vec![0u8; 1024 * 1024]).unwrap();
        let (send, mut receive) = tokio::sync::mpsc::unbounded_channel();
        {
            let future =
                transfer::upload(sftp.clone(), &cancelled, "/Documents", move |done, _| {
                    if done > 0 {
                        let _ = send.send(());
                    }
                });
            tokio::pin!(future);
            tokio::select! { result = &mut future => panic!("Transfer completed before cancellation: {result:?}"), _ = receive.recv() => {} }
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let folders = list(&sftp, "/Documents").await.unwrap();
        assert!(!folders.entries.iter().any(
            |entry| entry.name.starts_with(".porthop-upload") || entry.name == "cancelled.bin"
        ));
    }
}
