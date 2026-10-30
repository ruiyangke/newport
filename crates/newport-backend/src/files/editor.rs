//! Bounded, connection-scoped editing over SFTP. Saves replace atomically;
//! an observed remote edit is a conflict, never an implicit overwrite.
use super::{Operations, Remote, canonical, child, path_valid, status};
use crate::{manager::Shared, remote::sftp::Session};
use anyhow::{Context, Result, bail, ensure};
use russh_sftp::protocol::{FileAttributes, OpenFlags, Packet, StatusCode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use uuid::Uuid;

#[path = "editor_page.rs"]
mod page;

pub const LIMIT: usize = newport_domain::text_file::EDIT_LIMIT;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Request {
    id: Uuid,
    operation: Uuid,
    root: String,
    path: String,
    expected_connection: Vec<serde_json::Value>,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    revision: Option<String>,
    #[serde(default)]
    offset: Option<u64>,
    #[serde(default)]
    allow_cached: bool,
}
#[derive(Clone, Debug, Serialize)]
pub struct Document {
    pub(super) path: String,
    pub(super) content: String,
    pub(super) revision: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) page: Option<page::Page>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(super) cached: bool,
}
fn fingerprint(content: &str, attrs: &FileAttributes) -> String {
    let mut hash = Sha256::new();
    hash.update(content.as_bytes());
    hash.update(format!(
        "{:?}:{:?}:{:?}",
        attrs.permissions, attrs.uid, attrs.gid
    ));
    hash.finalize().iter().map(|b| format!("{b:02x}")).collect()
}
fn valid_text(bytes: Vec<u8>) -> Result<String> {
    ensure!(
        bytes.len() <= LIMIT,
        "This file exceeds the 16 MiB editing limit."
    );
    ensure!(
        !newport_domain::text_file::has_binary_signature(&bytes),
        "This is a binary file. Download it from Files to open it in a compatible app."
    );
    let text = String::from_utf8(bytes).context(
        "This file is binary or uses an unsupported encoding. Only UTF-8 text can be displayed.",
    )?;
    newport_domain::text_file::validate(&text).map_err(anyhow::Error::msg)?;
    Ok(text)
}
fn inside(root: &str, path: &str) -> bool {
    path.strip_prefix(root.trim_end_matches('/'))
        .is_some_and(|tail| tail.starts_with('/') && tail.len() > 1)
}
async fn target(session: &Session, root: &str, path: &str) -> Result<String> {
    path_valid(root)?;
    path_valid(path)?;
    ensure!(
        root.starts_with('/') && path.starts_with('/'),
        "Choose an absolute worktree path."
    );
    ensure!(
        canonical(session, root).await? == root,
        "The worktree path changed. Reopen the project."
    );
    ensure!(inside(root, path), "The file is outside this worktree.");
    ensure!(
        canonical(session, path).await? == path,
        "Symbolic links cannot be edited here. Open the original file instead."
    );
    Ok(path.to_owned())
}
async fn read(session: &Session, path: &str) -> Result<(Document, FileAttributes)> {
    read_bounded(session, path, LIMIT).await
}
async fn read_bounded(
    session: &Session,
    path: &str,
    limit: usize,
) -> Result<(Document, FileAttributes)> {
    let attrs = session.lstat(path).await?.attrs;
    ensure!(attrs.is_regular(), "Only regular files can be edited.");
    ensure!(
        attrs.size.is_some_and(|size| size <= limit as u64),
        "This file exceeds the 16 MiB editing limit."
    );
    let handle = session
        .open(path, OpenFlags::READ, FileAttributes::empty())
        .await?
        .handle;
    let result = async {
        let mut bytes = Vec::with_capacity(attrs.size.unwrap_or(0) as usize);
        const CHUNK: usize = 32 * 1024;
        let size = attrs.size.unwrap() as usize;
        // Several requests in flight avoid one round trip per chunk.
        for base in (0..size).step_by(CHUNK * 4) {
            let handle = &handle;
            let block = |n: usize| async move {
                let offset = base + n * CHUNK;
                page::bytes(
                    session,
                    handle,
                    offset as u64,
                    size.saturating_sub(offset).min(CHUNK),
                )
                .await
            };
            let (a, b, c, d) = tokio::try_join!(block(0), block(1), block(2), block(3))?;
            for part in [a, b, c, d] {
                bytes.extend(part);
            }
            ensure!(
                bytes.len() == (base + CHUNK * 4).min(size),
                "The file changed while being read. Try opening it again."
            );
            if base == 0 {
                ensure!(
                    !newport_domain::text_file::has_binary_signature(&bytes)
                        && !newport_domain::text_file::has_binary_controls(&bytes),
                    "This is a binary file. Download it from Files to open it in a compatible app."
                );
            }
        }
        let after = session.fstat(&handle).await?.attrs;
        ensure!(
            attrs.size == after.size
                && attrs.mtime == after.mtime
                && attrs.permissions == after.permissions,
            "The file changed while being read. Try opening it again."
        );
        let content = valid_text(bytes)?;
        let revision = fingerprint(&content, &attrs);
        Ok((
            Document {
                path: path.into(),
                content,
                revision,
                page: None,
                cached: false,
            },
            attrs,
        ))
    }
    .await;
    let closed = session.close(handle).await;
    let document = result?;
    closed?;
    Ok(document)
}
// OpenSSH's atomic replace extension. Never fall back to delete + rename or
// truncation: a failed/disconnected save must leave the original file intact.
async fn replace(session: &Session, from: &str, to: &str) -> Result<()> {
    let mut bytes = Vec::new();
    for path in [from, to] {
        bytes.extend_from_slice(&(path.len() as u32).to_be_bytes());
        bytes.extend_from_slice(path.as_bytes());
    }
    match session.extended("posix-rename@openssh.com", bytes).await? {
        Packet::Status(reply) if reply.status_code == StatusCode::Ok => Ok(()),
        Packet::Status(reply) => bail!(
            "Atomic save was refused ({:?}). The original file was not replaced.",
            reply.status_code
        ),
        _ => bail!("Save could not be confirmed. Reopen the remote file before retrying."),
    }
}
async fn save(
    operations: &Operations,
    remote: &Remote,
    session: &Session,
    request: &Request,
) -> Result<Document> {
    ensure!(
        request.offset.is_none(),
        "Large-file sections are read-only."
    );
    let content = request.content.as_ref().context("Missing file content")?;
    valid_text(content.as_bytes().to_vec())?;
    let expected = request
        .revision
        .as_deref()
        .context("Reopen the file before saving.")?;
    let path = target(session, &request.root, &request.path).await?;
    let (before, attrs) = read(session, &path).await?;
    ensure!(
        before.revision == expected,
        "The remote file changed. Your edits are kept; review and resolve the remote changes before saving."
    );
    let parent = path.rsplit_once('/').context("Invalid file path")?.0;
    let staging = child(parent, &format!(".newport-upload-{}", Uuid::new_v4()))?;
    let record = operations
        .track_upload(remote.cleanup_scope()?, staging.clone())
        .await?;
    let mut cleanup = super::transfer::RemoteTemp {
        operations,
        remote: remote.clone(),
        record: Some(record),
        created: false,
    };
    let opened = session
        .open(
            &staging,
            OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::EXCLUDE,
            FileAttributes {
                permissions: Some(0o600),
                ..FileAttributes::empty()
            },
        )
        .await;
    let handle = match opened {
        Ok(handle) => handle.handle,
        Err(error) => {
            if matches!(error, russh_sftp::client::error::Error::Status(_)) {
                let _ = operations
                    .finish_upload(cleanup.record.take().unwrap())
                    .await;
            }
            return Err(error.into());
        }
    };
    cleanup.created = true;
    let result = async {
        const CHUNK: usize = 32 * 1024;
        for (batch, data) in content.as_bytes().chunks(CHUNK * 4).enumerate() {
            let handle = &handle;
            let block = |n: usize| async move {
                let start = (n * CHUNK).min(data.len());
                let end = (start + CHUNK).min(data.len());
                if start < end {
                    session
                        .write(
                            handle,
                            (batch * CHUNK * 4 + start) as u64,
                            data[start..end].to_vec(),
                        )
                        .await?;
                }
                Ok::<_, anyhow::Error>(())
            };
            tokio::try_join!(block(0), block(1), block(2), block(3))?;
        }
        // Preserve executable bits and ownership; refuse if that is impossible.
        session
            .fsetstat(
                &handle,
                FileAttributes {
                    permissions: attrs.permissions,
                    uid: attrs.uid,
                    gid: attrs.gid,
                    ..FileAttributes::empty()
                },
            )
            .await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let closed = session.close(handle).await;
    result?;
    closed?;
    target(session, &request.root, &path).await?;
    let (latest, _) = read(session, &path).await?;
    ensure!(
        latest.revision == expected,
        "The remote file changed during save. Your edits are kept; the remote file was not replaced."
    );
    // SFTP has no compare-and-swap primitive. The second check narrows, but
    // cannot eliminate, a race with an unrelated remote writer at rename.
    replace(session, &staging, &path).await.context(
        "Save was not confirmed. Your buffer is kept; reload the remote version before retrying.",
    )?;
    let _ = operations
        .finish_upload(cleanup.record.take().unwrap())
        .await;
    Ok(Document {
        path,
        content: content.clone(),
        revision: fingerprint(content, &attrs),
        page: None,
        cached: false,
    })
}
pub(super) async fn load_document(
    owner: &Arc<crate::server_session::ServerSession>,
    root: &str,
    path: &str,
) -> Result<Document> {
    let session = owner.files.remote(owner).sftp.shared().await?;
    let path = target(&session, root, path).await?;
    page::read_document(&session, &path, None, None).await
}
pub async fn dispatch(
    state: &Shared,
    operations: &Operations,
    request: Request,
    write: bool,
) -> Result<Document, String> {
    let expected = serde_json::Value::Array(request.expected_connection.clone());
    operations
        .run_with_connection(
            state,
            request.id,
            request.operation,
            Some(&expected),
            async |_server, owner| {
                let _serial = if write {
                    Some(operations.editor_writes.lock().await)
                } else {
                    None
                };
                if !write && request.offset.is_none() {
                    return owner
                        .files
                        .documents
                        .load(&owner, &request.root, &request.path, request.allow_cached)
                        .await
                        .map_err(anyhow::Error::msg);
                }
                let remote = owner.files.remote(&owner);
                let session = remote.sftp.shared().await?;
                operations.retry_upload_cleanup(remote.clone());
                if write {
                    owner
                        .files
                        .documents
                        .invalidate(|root, path| root == request.root && path == request.path);
                    let result = save(operations, &remote, &session, &request).await;
                    // Even a successful save is revalidated before a later cached open.
                    // Its old base cannot survive a failed or uncertain write either.
                    owner
                        .files
                        .documents
                        .invalidate(|root, path| root == request.root && path == request.path);
                    if let Ok(document) = &result {
                        owner.files.documents.saved(&request.root, document.clone());
                    }
                    result
                } else {
                    let path = target(&session, &request.root, &request.path).await?;
                    page::read_document(
                        &session,
                        &path,
                        request.offset,
                        request.revision.as_deref(),
                    )
                    .await
                }
            },
        )
        .await
}

/// Recheck the size at read time: a stale listing must not cause a large download.
pub(super) async fn prefetch_document(
    owner: &Arc<crate::server_session::ServerSession>,
    root: &str,
    path: &str,
    limit: usize,
) -> Result<Document> {
    let session = owner.files.remote(owner).sftp.shared().await?;
    let path = target(&session, root, path).await?;
    let (document, _) = read_bounded(&session, &path, limit).await?;
    ensure!(
        !newport_domain::text_file::has_long_lines(&document.content),
        "Skip generated long-line files."
    );
    Ok(document)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    #[ignore = "Run with cargo xtask test ssh -- files::editor"]
    async fn atomic_save_conflict_and_path_guards_over_sftp() {
        let server = super::super::integration_tests::server();
        let remote = super::super::integration_tests::remote(&server);
        let session = remote.sftp.shared().await.unwrap();
        let path = format!("/Documents/editor-{}.rs", Uuid::new_v4());
        let original = "\u{feff}fn main() {}\r\n";
        let handle = session
            .open(
                &path,
                OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::EXCLUDE,
                FileAttributes {
                    permissions: Some(0o755),
                    ..FileAttributes::empty()
                },
            )
            .await
            .unwrap()
            .handle;
        session
            .write(&handle, 0, original.as_bytes().to_vec())
            .await
            .unwrap();
        session.close(handle).await.unwrap();
        let (before, attrs) = read(&session, &path).await.unwrap();
        assert_eq!(before.content, original);
        assert!(
            read_bounded(&session, &path, 4).await.is_err(),
            "prefetch must refuse files exceeding its current byte budget"
        );
        assert_eq!(
            read_bounded(&session, &path, 256 * 1024)
                .await
                .unwrap()
                .0
                .content,
            original
        );
        let operations = Operations::default();
        let mut request = Request {
            id: server.id,
            operation: Uuid::new_v4(),
            root: "/Documents".into(),
            path: path.clone(),
            expected_connection: vec![],
            offset: None,
            allow_cached: false,
            revision: Some(before.revision.clone()),
            content: Some("fn main() { println!(\"hello 世界\"); }\n".into()),
        };
        let saved = save(&operations, &remote, &session, &request)
            .await
            .unwrap();
        let (after, after_attrs) = read(&session, &path).await.unwrap();
        assert_eq!(after.content, request.content.clone().unwrap());
        assert_eq!(after.revision, saved.revision);
        assert_eq!(after_attrs.permissions, attrs.permissions);
        assert_eq!(after_attrs.uid, attrs.uid);
        request.content = Some("stale editor must not overwrite".into());
        assert!(
            save(&operations, &remote, &session, &request)
                .await
                .unwrap_err()
                .to_string()
                .contains("remote file changed")
        );
        assert_eq!(
            read(&session, &path).await.unwrap().0.content,
            after.content
        );
        assert!(
            target(
                &session,
                "/Documents",
                "/Documents/../Documents/hello 世界.txt"
            )
            .await
            .is_err()
        );
        assert!(target(&session, "/Documents", "/other.txt").await.is_err());
        request.revision = Some(after.revision);
        request.content = Some("".into());
        save(&operations, &remote, &session, &request)
            .await
            .unwrap();
        assert_eq!(read(&session, &path).await.unwrap().0.content, "");
        // Exercise real remote writes and content-hash conflict checks on a large file.
        let large = format!("{}\n", "x".repeat(200)).repeat(63_000);
        assert!(large.len() > 12 * 1024 * 1024);
        request.revision = Some(read(&session, &path).await.unwrap().0.revision);
        request.content = Some(large.clone());
        let saved = save(&operations, &remote, &session, &request)
            .await
            .unwrap();
        let opened = page::read_document(&session, &path, None, None)
            .await
            .unwrap();
        assert!(opened.page.is_none());
        assert_eq!(opened.content, large);
        request.revision = Some(saved.revision);
        request.content = Some(format!("// edit\n{large}"));
        let changed = save(&operations, &remote, &session, &request)
            .await
            .unwrap();
        assert_eq!(changed.content, request.content.clone().unwrap());
        assert!(
            save(&operations, &remote, &session, &request)
                .await
                .unwrap_err()
                .to_string()
                .contains("remote file changed")
        );
        session.remove(&path).await.unwrap();
    }
    #[tokio::test]
    async fn editor_requires_current_connection_before_opening_ssh() {
        let directory = tempfile::tempdir().unwrap();
        let server: crate::model::Server = serde_json::from_value(serde_json::json!({
            "id":Uuid::new_v4(),"name":"Scope fixture","sshHost":"127.0.0.1","sshPort":9,"sshUser":"fixture"
        })).unwrap();
        let mut manager =
            crate::manager::Manager::new(crate::profile::Store::for_test(directory.path().into()));
        manager.config.servers.push(server.clone());
        let state = crate::manager::share(manager);
        for write in [false, true] {
            let request = Request {
                id: server.id,
                operation: Uuid::new_v4(),
                root: "/repo".into(),
                path: "/repo/file".into(),
                expected_connection: vec![],
                offset: None,
                allow_cached: false,
                content: Some("changed".into()),
                revision: Some("old".into()),
            };
            let error = dispatch(&state, &Operations::default(), request, write)
                .await
                .unwrap_err();
            assert!(error.contains("connection changed"), "{error}");
        }
    }
    #[test]
    fn bounded_utf8_and_precise_worktree_boundary() {
        assert!(inside("/repo", "/repo/src/main.rs"));
        assert!(!inside("/repo", "/repository/main.rs"));
        assert!(!inside("/repo", "/repo"));
        assert!(valid_text(b"hello\r\n\t".to_vec()).is_ok());
        assert!(valid_text(vec![b'x'; LIMIT + 1]).is_err());
        assert!(valid_text(vec![0xff]).is_err());
        assert!(valid_text(b"a\0b".to_vec()).is_err());
    }
    #[test]
    fn revision_covers_content_and_permissions() {
        let attrs = FileAttributes {
            permissions: Some(0o100644),
            ..FileAttributes::empty()
        };
        let original = fingerprint("same length", &attrs);
        assert_ne!(original, fingerprint("SAME LENGTH", &attrs));
        let executable = FileAttributes {
            permissions: Some(0o100755),
            ..attrs.clone()
        };
        assert_ne!(original, fingerprint("same length", &executable));
        assert_eq!(original, fingerprint("same length", &attrs));
    }
}
