//! Remote file operations. Local paths come exclusively from native dialogs.
mod cleanup;
mod download_cleanup;
pub mod editor;
pub mod operations;
pub mod project_index;
mod recovery_storage;
pub mod transfer;
pub(crate) mod vfs;
mod vfs_documents;
mod vfs_prefetch;
use crate::{manager::Shared, model::Server, remote::sftp::Session};
use anyhow::{Context, Result, bail};
use base64::Engine as _;
pub use operations::Operations;
use russh_sftp::{
    client::error::Error,
    protocol::{FileAttributes, OpenFlags, StatusCode},
};
use serde::Serialize;
use uuid::Uuid;
const IMAGE_LIMIT: usize = 16 * 1024 * 1024;
const TEXT_LIMIT: usize = 1024 * 1024;
const DIRECTORY_LIMIT: usize = 20_000;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    name: String,
    path: String,
    kind: &'static str,
    size: Option<u64>,
    modified: Option<u32>,
    permissions: Option<String>,
}
#[derive(Clone, Debug, Serialize)]
pub struct Listing {
    path: String,
    entries: Vec<Entry>,
}
#[derive(Clone, Debug, Serialize)]
pub struct Preview {
    kind: &'static str,
    content: String,
    mime: &'static str,
    truncated: bool,
}

/// One server's SFTP for Files, and the server its upload staging is
/// recorded under. It holds the server's link, which its sessions need.
#[derive(Clone)]
pub struct Remote {
    pub(crate) sftp: crate::remote::Sftp,
    server: Server,
}

impl Remote {
    pub(crate) fn new(server: Server, link: newport_ssh::Link) -> Self {
        Self {
            sftp: crate::remote::Sftp::new(link),
            server,
        }
    }

    pub(crate) fn server_id(&self) -> Uuid {
        self.server.id
    }

    /// The saved connection identity without the transient connection
    /// revision: staging cleanup may retry after reconnecting, but never on a
    /// different endpoint, user, authentication profile or server record.
    fn cleanup_scope(&self) -> Result<serde_json::Value> {
        Ok(newport_domain::connection_scope::connection(
            &serde_json::to_value(&self.server)?,
            &serde_json::Value::Null,
        ))
    }
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
async fn canonical(session: &Session, path: &str) -> Result<String> {
    path_valid(path)?;
    session
        .realpath(path)
        .await?
        .files
        .into_iter()
        .next()
        .map(|entry| entry.filename)
        .context("Server returned no canonical path")
}
pub async fn list(session: &Session, path: &str) -> Result<Listing> {
    let path = canonical(session, path).await?;
    let handle = session.opendir(&path).await?.handle;
    let mut entries = Vec::new();
    let result = async {
        loop {
            let batch = match session.readdir(&handle).await {
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
                    session
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
    let closed = session.close(handle).await;
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
pub async fn preview(session: &Session, path: &str) -> Result<Preview> {
    path_valid(path)?;
    let metadata = session.stat(path).await?.attrs;
    if !metadata.is_regular() {
        bail!("Only regular files can be previewed");
    }
    let handle = session
        .open(path, OpenFlags::READ, FileAttributes::empty())
        .await?
        .handle;
    // Read at most one byte beyond the visible prefix to distinguish an exact
    // limit from truncation, even if the file grows or stat omits its size.
    let result = async {
        let mut bytes = Vec::new();
        loop {
            // Sniff a bounded header before applying text validation. SFTP may
            // return short chunks, including part of an image signature.
            let limit = if bytes.len() < 12 {
                12
            } else if image_mime(&bytes).is_some() {
                IMAGE_LIMIT + 1
            } else {
                TEXT_LIMIT + 1
            };
            if bytes.len() >= limit {
                break;
            }
            let length = (limit - bytes.len()).min(32 * 1024) as u32;
            match session.read(&handle, bytes.len() as u64, length).await {
                Ok(data) if data.data.is_empty() => break,
                Ok(data) => {
                    if data.data.len() > length as usize {
                        bail!("SFTP server returned more preview data than requested");
                    }
                    bytes.extend_from_slice(&data.data);
                    // Reject binary content before downloading the rest.
                    if bytes.len() >= 12 && image_mime(&bytes).is_none() {
                        validate_text_prefix(&bytes)?;
                    }
                }
                Err(e) if status(&e, StatusCode::Eof) => break,
                Err(e) => return Err(e.into()),
            }
        }
        classify(bytes)
    }
    .await;
    // Close on validation and read errors as well as success.
    let closed = session.close(handle).await;
    let preview = result?;
    closed?;
    Ok(preview)
}

const BINARY_PREVIEW: &str = "This binary format is unsupported. Only text and supported images can be previewed. Download this file to open it.";

fn image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else {
        None
    }
}

fn validate_text_prefix(bytes: &[u8]) -> Result<()> {
    // PDF headers can be valid ASCII; do not mistake them for source text.
    if bytes.starts_with(b"%PDF-") {
        bail!(BINARY_PREVIEW);
    }
    let valid = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        // A read can end inside a UTF-8 character. Validate the complete prefix.
        Err(error) if error.error_len().is_none() => {
            std::str::from_utf8(&bytes[..error.valid_up_to()])?
        }
        Err(_) => bail!(BINARY_PREVIEW),
    };
    if valid
        .chars()
        .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t'))
    {
        bail!(BINARY_PREVIEW);
    }
    Ok(())
}

fn classify(mut bytes: Vec<u8>) -> Result<Preview> {
    if let Some(mime) = image_mime(&bytes) {
        if bytes.len() > IMAGE_LIMIT {
            bail!("Image exceeds the 16 MiB preview limit. Download it to view the full image.");
        }
        return Ok(Preview {
            kind: "image",
            content: base64::engine::general_purpose::STANDARD.encode(bytes),
            mime,
            truncated: false,
        });
    }
    validate_text_prefix(&bytes)?;
    let truncated = bytes.len() > TEXT_LIMIT;
    if truncated {
        bytes.truncate(TEXT_LIMIT);
        if let Err(error) = std::str::from_utf8(&bytes) {
            if error.error_len().is_some() {
                bail!(BINARY_PREVIEW);
            }
            bytes.truncate(error.valid_up_to());
        }
    }
    let content = String::from_utf8(bytes).map_err(|_| anyhow::anyhow!(BINARY_PREVIEW))?;
    Ok(Preview {
        kind: "text",
        content,
        mime: "text/plain",
        truncated,
    })
}

pub async fn files_list(
    state: &Shared,
    operations: &Operations,
    id: Uuid,
    operation: Uuid,
    path: String,
    expected_connection: Option<serde_json::Value>,
) -> Result<Listing, String> {
    files_list_query(
        state,
        operations,
        id,
        operation,
        path,
        expected_connection,
        false,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn files_list_query(
    state: &Shared,
    operations: &Operations,
    id: Uuid,
    operation: Uuid,
    path: String,
    expected_connection: Option<serde_json::Value>,
    force: bool,
    project_root: Option<String>,
) -> Result<Listing, String> {
    operations.run_with_connection(state, id, operation, expected_connection.as_ref(), async |_server, owner| {
        let weak = std::sync::Arc::downgrade(&owner);
        let cleanup = operations.clone();
        let query = owner.files.listings.get_or_insert(path.clone(), crate::query::Policy {
            stale_after: std::time::Duration::from_secs(5),
            retry_after: std::time::Duration::from_secs(2),
            timeout: std::time::Duration::from_secs(65),
            observation_lease: std::time::Duration::from_secs(1),
        }, move || {
            let weak = weak.clone(); let path = path.clone(); let cleanup = cleanup.clone();
            async move {
                let owner = weak.upgrade().ok_or("Server session closed.")?;
                let mut cancel = owner.cancellation();
                if *cancel.borrow() { return Err("Server session closed.".into()); }
                tokio::select! {
                    biased;
                    _ = cancel.changed() => Err("Server session closed.".into()),
                    result = async {
                        let remote = owner.files.remote(&owner);
                        let session = remote.sftp.shared().await.map_err(|e| e.to_string())?;
                        cleanup.retry_upload_cleanup(remote);
                        list(&session, &path).await.map(std::sync::Arc::new).map_err(|e| e.to_string())
                    } => result,
                }
            }
        }).map_err(anyhow::Error::msg)?;
        if owner.is_revoked() { query.close(); }
        let result = if force { query.refresh().await } else { query.read().await };
        result.map(|listing| {
            if let Some(root)=project_root.as_deref() { owner.files.documents.prefetch(&owner,root,&listing); }
            (*listing).clone()
        }).map_err(anyhow::Error::msg)
    }).await
}
pub async fn files_preview(
    state: &Shared,
    operations: &Operations,
    id: Uuid,
    operation: Uuid,
    path: String,
    expected_connection: Option<serde_json::Value>,
) -> Result<Preview, String> {
    operations
        .run_with_connection(
            state,
            id,
            operation,
            expected_connection.as_ref(),
            async |_server, owner| {
                let remote = owner.files.remote(&owner);
                let session = remote.sftp.shared().await?;
                operations.retry_upload_cleanup(remote);
                tokio::time::timeout(std::time::Duration::from_secs(60), preview(&session, &path))
                    .await
                    .context("File preview timed out")?
            },
        )
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn listing_and_preview_on_a_link_that_stopped_trying_fail_with_its_error() {
        let directory = tempfile::tempdir().unwrap();
        let mut server: crate::model::Server = serde_json::from_value(serde_json::json!({
            "id":Uuid::new_v4(), "name":"Stopped link", "sshUser":"fixture",
            "sshHost":"inner.invalid", "sshPort":22
        }))
        .unwrap();
        // A jump host that isn't saved stops the link until Retry.
        server.proxy = Some(crate::model::Proxy::Jump(Uuid::new_v4()));
        let mut manager =
            crate::manager::Manager::new(crate::profile::Store::for_test(directory.path().into()));
        manager.config.servers.push(server.clone());
        manager.sessions.session(&server, 0);
        let state = crate::manager::share(manager);
        let operations = Operations::default();
        let prompt = std::time::Duration::from_secs(5);
        let listed = tokio::time::timeout(
            prompt,
            files_list(
                &state,
                &operations,
                server.id,
                Uuid::new_v4(),
                "/".into(),
                None,
            ),
        )
        .await
        .expect("the listing to fail at once");
        let error = listed.err().unwrap();
        assert!(error.starts_with("The jump host can't be used"), "{error}");
        let previewed = tokio::time::timeout(
            prompt,
            files_preview(
                &state,
                &operations,
                server.id,
                Uuid::new_v4(),
                "/a".into(),
                None,
            ),
        )
        .await
        .expect("the preview to fail at once");
        assert_eq!(previewed.err().unwrap(), error);
    }

    #[tokio::test]
    #[ignore = "Run with cargo xtask test ssh -- files::tests::query"]
    async fn query_lists_over_sftp_reuses_fresh_data_and_validates_cached_reads() {
        let directory = tempfile::tempdir().unwrap();
        let server: crate::model::Server = serde_json::from_value(serde_json::json!({
            "id":Uuid::new_v4(), "name":"File query fixture", "sshUser":"fixture", "sshHost":"127.0.0.1",
            "sshPort":std::env::var("NEWPORT_TEST_SSH_PORT").unwrap().parse::<u16>().unwrap(),
            "identityFile":std::env::var("NEWPORT_TEST_IDENTITY").unwrap(), "agentInstallApproved":false
        })).unwrap();
        let mut manager =
            crate::manager::Manager::new(crate::profile::Store::for_test(directory.path().into()));
        manager.config.servers.push(server.clone());
        let owner = manager.sessions.session(&server, 0);
        let state = crate::manager::share(manager);
        let operations = Operations::default();
        let first = files_list(
            &state,
            &operations,
            server.id,
            Uuid::new_v4(),
            "/".into(),
            None,
        )
        .await
        .unwrap();
        assert!(first.entries.iter().any(|entry| entry.name == "Documents"));
        let query = owner
            .files
            .listings
            .get_or_insert(
                "/".into(),
                crate::query::Policy {
                    stale_after: std::time::Duration::from_secs(5),
                    retry_after: std::time::Duration::from_secs(2),
                    timeout: std::time::Duration::from_secs(65),
                    observation_lease: std::time::Duration::from_secs(1),
                },
                || async { panic!("existing query loader must be retained") },
            )
            .unwrap();
        let revision = query.snapshot().revision;
        files_list(
            &state,
            &operations,
            server.id,
            Uuid::new_v4(),
            "/".into(),
            None,
        )
        .await
        .unwrap();
        assert_eq!(query.snapshot().revision, revision);
        assert!(
            files_list(
                &state,
                &operations,
                server.id,
                Uuid::new_v4(),
                "/".into(),
                Some(serde_json::json!(["obsolete"]))
            )
            .await
            .is_err()
        );
        files_list_query(
            &state,
            &operations,
            server.id,
            Uuid::new_v4(),
            "/".into(),
            None,
            true,
            None,
        )
        .await
        .unwrap();
        assert!(query.snapshot().revision > revision);
        assert!(owner.agent_runtime.snapshot().instances.is_empty());
        assert!(owner.lifecycle.progress.snapshot().is_none());
        crate::manager::shutdown(&state).await;
        assert!(query.snapshot().data.is_none());
    }

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
    fn text_preview_bounds_preserve_utf8_and_mark_only_partial_content() {
        let exact = classify(vec![b'a'; TEXT_LIMIT]).unwrap();
        assert!(!exact.truncated);
        assert_eq!(exact.content.len(), TEXT_LIMIT);
        let mut split = vec![b'a'; TEXT_LIMIT - 1];
        split.extend_from_slice("雪".as_bytes());
        let partial = classify(split).unwrap();
        assert!(partial.truncated);
        assert_eq!(partial.content.len(), TEXT_LIMIT - 1);
        assert!(classify(vec![0xe9, 0x9b]).is_err());
        assert!(validate_text_prefix(&[0xe9, 0x9b]).is_ok());
        let literal = "\u{feff}<script>literal</script>\n雪\t🦀";
        assert_eq!(
            classify(literal.as_bytes().to_vec()).unwrap().content,
            literal
        );
        assert_eq!(classify(vec![]).unwrap().content, "");
    }

    #[test]
    fn text_preview_rejects_documents_archives_and_non_utf8() {
        for binary in [b"%PDF-1.7\n".as_slice(), b"PK\x03\x04", b"\xff\xfeh\0i\0"] {
            assert!(classify(binary.to_vec()).is_err(), "accepted {binary:?}");
        }
    }

    #[test]
    fn image_preview_uses_signatures_and_never_truncates() {
        for (header, mime) in [
            (b"\x89PNG\r\n\x1a\n".as_slice(), "image/png"),
            (b"\xff\xd8\xff", "image/jpeg"),
            (b"GIF87a", "image/gif"),
            (b"GIF89a", "image/gif"),
            (b"RIFF\0\0\0\0WEBP", "image/webp"),
        ] {
            let preview = classify(header.to_vec()).unwrap();
            assert_eq!(preview.kind, "image");
            assert_eq!(preview.mime, mime);
            assert!(!preview.truncated);
            assert_eq!(
                base64::engine::general_purpose::STANDARD
                    .decode(preview.content)
                    .unwrap(),
                header
            );
        }
        assert_eq!(image_mime(b"RIFF\0\0\0\0WAVE"), None);
        let mut image = b"GIF89a".to_vec();
        image.resize(IMAGE_LIMIT, 0);
        assert!(!classify(image.clone()).unwrap().truncated);
        image.push(0);
        assert!(classify(image).unwrap_err().to_string().contains("16 MiB"));
    }

    #[test]
    fn preview_uses_content_and_never_interprets_markup() {
        assert_eq!(
            classify(b"<script>alert(1)</script>".to_vec())
                .unwrap()
                .kind,
            "text"
        );
        assert!(classify(b"%PDF-1.7\n".to_vec()).is_err());
        assert!(classify(vec![0, 1, 2]).is_err());
        assert!(classify(vec![b'a'; TEXT_LIMIT + 1]).unwrap().truncated);
    }
}

#[cfg(test)]
pub(super) mod integration_tests {
    use super::*;
    use crate::{
        model::{AuthMethod, Server},
        server_session::ServerSession,
    };
    use std::{sync::Arc, time::Duration};

    pub(in crate::files) fn server() -> Server {
        Server {
            id: Uuid::new_v4(),
            name: "Files fixture".into(),
            ssh_user: "fixture".into(),
            ssh_host: "127.0.0.1".into(),
            ssh_port: std::env::var("NEWPORT_TEST_SSH_PORT")
                .unwrap()
                .parse()
                .unwrap(),
            identity_file: Some(std::env::var("NEWPORT_TEST_IDENTITY").unwrap()),
            agent_source: None,
            agent_key_fingerprint: None,
            auth_method: AuthMethod::PublicKey,
            clipboard_enabled: false,
            browser_enabled: false,
            agent_install_approved: true,
            setup_complete: true,
            proxy: None,
        }
    }

    pub(in crate::files) fn remote(server: &Server) -> Remote {
        Remote::new(
            server.clone(),
            crate::ssh::link(server, crate::ssh::no_jump_hosts),
        )
    }

    /// An app holding the fixture server, and that server's session.
    pub(in crate::files) fn app() -> (tempfile::TempDir, Shared, Arc<ServerSession>, Server) {
        let directory = tempfile::tempdir().unwrap();
        let server = server();
        let mut manager =
            crate::manager::Manager::new(crate::profile::Store::for_test(directory.path().into()));
        manager.config.servers.push(server.clone());
        let owner = manager.sessions.session(&server, 0);
        (directory, crate::manager::share(manager), owner, server)
    }

    /// The number of the next file handle on `session`. The fixture numbers
    /// each session's handles in order, so two probes tell how many files
    /// were opened on that session between them.
    async fn next_handle(session: &Session) -> u64 {
        let handle = session
            .open(
                "/Documents/hello 世界.txt",
                OpenFlags::READ,
                FileAttributes::empty(),
            )
            .await
            .unwrap()
            .handle;
        let number = handle.parse().unwrap();
        session.close(handle).await.unwrap();
        number
    }

    async fn preview_hello(state: &Shared, operations: &Operations, server: Uuid) -> Preview {
        files_preview(
            state,
            operations,
            server,
            Uuid::new_v4(),
            "/Documents/hello 世界.txt".into(),
            None,
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    #[ignore = "Run with cargo xtask test ssh"]
    async fn preview_and_the_editor_share_one_sftp_session() {
        let (_directory, state, owner, server) = app();
        let operations = Operations::default();
        let shared = owner.files.remote(&owner).sftp.shared().await.unwrap();
        let before = next_handle(&shared).await;
        preview_hello(&state, &operations, server.id).await;
        let previewed = next_handle(&shared).await;
        assert_eq!(previewed, before + 2, "the preview used another session");
        let document = owner
            .files
            .documents
            .load(&owner, "/Documents", "/Documents/hello 世界.txt", false)
            .await
            .unwrap();
        assert!(document.content.contains("<script>inert text</script>"));
        assert_eq!(
            next_handle(&shared).await,
            previewed + 2,
            "the editor used another session"
        );
        drop(shared);
        crate::manager::shutdown(&state).await;
    }

    #[tokio::test]
    #[ignore = "Run with cargo xtask test ssh"]
    async fn a_transfer_has_its_own_sftp_session_and_does_not_hold_up_a_preview() {
        let (_directory, state, owner, server) = app();
        let operations = Operations::default();
        let remote = owner.files.remote(&owner);
        let shared = remote.sftp.shared().await.unwrap();
        let before = next_handle(&shared).await;
        let directory = tempfile::tempdir().unwrap();
        let local = directory
            .path()
            .join(format!("transfer-{}.bin", Uuid::new_v4()));
        let size = 16 * 1024 * 1024;
        std::fs::write(&local, vec![7u8; size]).unwrap();
        let (sent, mut started) = tokio::sync::mpsc::unbounded_channel();
        let upload = transfer::upload(
            &operations,
            &remote,
            &local,
            "/Documents",
            move |done, _| {
                if done > 0 {
                    let _ = sent.send(());
                }
            },
        );
        tokio::pin!(upload);
        tokio::select! {
            result = &mut upload => panic!("the upload finished before the preview: {result:?}"),
            _ = started.recv() => {}
        }
        // The upload now waits mid-transfer, its writes in flight.
        let preview = tokio::time::timeout(
            Duration::from_secs(10),
            preview_hello(&state, &operations, server.id),
        )
        .await
        .expect("the preview waited for the upload");
        assert!(preview.content.contains("<script>inert text</script>"));
        assert_eq!(
            next_handle(&shared).await,
            before + 2,
            "the upload opened its file on the shared session"
        );
        let uploaded = upload.await.unwrap();
        assert_eq!(
            shared.stat(&uploaded).await.unwrap().attrs.size,
            Some(size as u64)
        );
        shared.remove(&uploaded).await.unwrap();
        drop(shared);
        operations.shutdown().await;
        crate::manager::shutdown(&state).await;
    }

    #[tokio::test]
    #[ignore = "Run with cargo xtask test ssh"]
    async fn sftp_browse_preview_and_atomic_transfers() {
        let remote = remote(&server());
        let session = remote.sftp.shared().await.unwrap();
        let root = list(&session, ".").await.unwrap();
        assert_eq!(root.path, "/");
        assert!(
            root.entries
                .iter()
                .any(|entry| entry.name == "Documents" && entry.kind == "directory")
        );
        assert!(
            preview(&session, "/Documents/hello 世界.txt")
                .await
                .unwrap()
                .content
                .contains("<script>inert text</script>")
        );
        assert!(
            preview(&session, "/binary.dat")
                .await
                .unwrap_err()
                .to_string()
                .contains("binary")
        );
        assert!(preview(&session, "/large.txt").await.unwrap().truncated);
        assert!(list(&session, "/missing").await.is_err());
        let directory = tempfile::tempdir().unwrap();
        let local = directory.path().join("upload ' 世界.dat");
        let bytes: Vec<u8> = (0..300_000).map(|n| (n % 251) as u8).collect();
        std::fs::write(&local, &bytes).unwrap();
        let operations = Operations::default();
        let uploaded = transfer::upload(&operations, &remote, &local, "/Documents", |_, _| {})
            .await
            .unwrap();
        assert!(
            transfer::upload(&operations, &remote, &local, "/Documents", |_, _| {})
                .await
                .unwrap_err()
                .to_string()
                .contains("already exists")
        );
        // Exercise the image signature/size path over the real SFTP transport.
        // A misleading extension must not turn image bytes into source text.
        let image = directory.path().join("image.txt");
        let png = base64::engine::general_purpose::STANDARD.decode(
            "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/l9sAAAAASUVORK5CYII="
        ).unwrap();
        std::fs::write(&image, &png).unwrap();
        let remote_image = transfer::upload(&operations, &remote, &image, "/Documents", |_, _| {})
            .await
            .unwrap();
        let image_preview = preview(&session, &remote_image).await.unwrap();
        assert_eq!(image_preview.kind, "image");
        assert_eq!(image_preview.mime, "image/png");
        assert!(!image_preview.truncated);
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(image_preview.content)
                .unwrap(),
            png
        );
        let saved = directory.path().join("download.dat");
        std::fs::write(&saved, b"old bytes").unwrap();
        transfer::download(
            &crate::files::Operations::default(),
            &remote,
            &uploaded,
            &saved,
            |_, _| {},
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(&saved).unwrap(), bytes);
        assert!(
            transfer::download(
                &crate::files::Operations::default(),
                &remote,
                "/missing",
                &saved,
                |_, _| {}
            )
            .await
            .is_err()
        );
        assert_eq!(std::fs::read(&saved).unwrap(), bytes);
        let folders = list(&session, "/Documents").await.unwrap();
        assert!(
            !folders
                .entries
                .iter()
                .any(|entry| entry.name.starts_with(".newport-upload"))
        );
        // Dropping the transfer before completion removes the unpublished staging file.
        let cancelled = directory.path().join("cancelled.bin");
        std::fs::write(&cancelled, vec![0u8; 1024 * 1024]).unwrap();
        let (send, mut receive) = tokio::sync::mpsc::unbounded_channel();
        {
            let future = transfer::upload(
                &operations,
                &remote,
                &cancelled,
                "/Documents",
                move |done, _| {
                    if done > 0 {
                        let _ = send.send(());
                    }
                },
            );
            tokio::pin!(future);
            tokio::select! { result = &mut future => panic!("Transfer completed before cancellation: {result:?}"), _ = receive.recv() => {} }
        }
        operations.shutdown().await;
        let folders = list(&session, "/Documents").await.unwrap();
        assert!(!folders.entries.iter().any(
            |entry| entry.name.starts_with(".newport-upload") || entry.name == "cancelled.bin"
        ));
    }
}
