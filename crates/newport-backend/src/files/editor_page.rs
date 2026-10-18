//! Bounded viewing for large files. A page is never an editable document revision.
use super::*;
pub const PAGE_SIZE: usize = 64 * 1024;
#[derive(Clone, Debug, Serialize)]
pub struct Page {
    pub offset: u64,
    pub next: Option<u64>,
    pub size: u64,
    pub reason: &'static str,
}

pub(super) async fn bytes(sftp: &Sftp, handle: &str, offset: u64, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(limit);
    while bytes.len() < limit {
        match sftp
            .session
            .read(
                handle,
                offset + bytes.len() as u64,
                (limit - bytes.len()).min(32 * 1024) as u32,
            )
            .await
        {
            Ok(data) if data.data.is_empty() => break,
            Ok(data) => {
                ensure!(
                    data.data.len() <= limit - bytes.len(),
                    "The server returned too much file data."
                );
                bytes.extend(data.data);
            }
            Err(e) if status(&e, StatusCode::Eof) => break,
            Err(e) => return Err(e.into()),
        }
    }
    Ok(bytes)
}
fn revision(attrs: &FileAttributes, prefix: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(format!(
        "{:?}:{:?}:{:?}:{:?}:{:?}",
        attrs.size, attrs.mtime, attrs.permissions, attrs.uid, attrs.gid
    ));
    hash.update(prefix);
    format!(
        "page:{}",
        hash.finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    )
}
fn page_text(mut bytes: Vec<u8>, more: bool) -> Result<String> {
    if more {
        // Keep a multibyte character and CRLF pair on the same page. The next
        // offset advances by the bytes actually returned, never the requested size.
        if let Err(error) = std::str::from_utf8(&bytes) {
            ensure!(
                error.error_len().is_none(),
                "This file is binary or uses an unsupported encoding. Only UTF-8 text can be displayed."
            );
            bytes.truncate(error.valid_up_to());
        }
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
    }
    super::valid_text(bytes)
}
pub async fn read_document(
    sftp: &Sftp,
    path: &str,
    offset: Option<u64>,
    expected: Option<&str>,
) -> Result<Document> {
    let attrs = sftp.session.lstat(path).await?.attrs;
    ensure!(attrs.is_regular(), "Only regular files can be displayed.");
    let size = attrs
        .size
        .context("The server did not return this file's size.")?;
    if size <= LIMIT as u64 && offset.is_none() {
        let (document, _) = super::read(sftp, path).await?;
        if !newport_domain::text_file::has_long_lines(&document.content) {
            return Ok(document);
        }
    }
    let start = offset.unwrap_or(0);
    ensure!(start < size, "The file size changed. Reload the file.");
    let handle = sftp
        .session
        .open(path, OpenFlags::READ, FileAttributes::empty())
        .await?
        .handle;
    let result = async {
        let prefix = bytes(sftp, &handle, 0, size.min(4096) as usize).await?;
        ensure!(
            !newport_domain::text_file::has_binary_signature(&prefix)
                && !newport_domain::text_file::has_binary_controls(&prefix),
            "This is a binary file. Download it from Files to open it in a compatible app."
        );
        let revision = revision(&attrs, &prefix);
        if offset.is_some() {
            ensure!(
                expected == Some(revision.as_str()),
                "The file changed. Reload it before viewing another section."
            );
        }
        let count = (size - start).min(PAGE_SIZE as u64) as usize;
        let raw = bytes(sftp, &handle, start, count).await?;
        ensure!(
            raw.len() == count,
            "The file changed while being read. Reload it."
        );
        let content = page_text(raw, start + (count as u64) < size)?;
        let after = sftp.session.fstat(&handle).await?.attrs;
        let after_prefix = bytes(sftp, &handle, 0, size.min(4096) as usize).await?;
        ensure!(
            revision == self::revision(&after, &after_prefix),
            "The file changed while being read. Reload it."
        );
        let end = start + content.len() as u64;
        ensure!(end > start, "This file cannot be displayed as UTF-8 text.");
        Ok(Document {
            cached: false,
            path: path.into(),
            content,
            revision,
            page: Some(Page {
                offset: start,
                next: (end < size).then_some(end),
                size,
                reason: if size > LIMIT as u64 {
                    "size"
                } else {
                    "long_lines"
                },
            }),
        })
    }
    .await;
    let closed = sftp.session.close(handle).await;
    let document = result?;
    closed?;
    Ok(document)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    #[ignore = "Run with npm run test:ssh -- files::editor"]
    async fn large_files_are_bounded_readonly_and_complete_over_sftp() {
        let server: crate::model::Server = serde_json::from_value(serde_json::json!({
            "id":Uuid::new_v4(),"name":"Paged editor fixture","sshUser":"fixture","sshHost":"127.0.0.1",
            "sshPort":std::env::var("NEWPORT_TEST_SSH_PORT").unwrap().parse::<u16>().unwrap(),
            "identityFile":std::env::var("NEWPORT_TEST_IDENTITY").unwrap(),"agentInstallApproved":false
        })).unwrap();
        let sftp = Arc::new(Sftp::connect(&server).await.unwrap());
        let path = format!("/Documents/large-{}.txt", Uuid::new_v4());
        // A multibyte character straddles the first boundary, and the final page is partial.
        let source = format!(
            "{}世界\r\n{}last",
            "x".repeat(PAGE_SIZE - 1),
            "hello 世界\r\n".repeat(LIMIT / 10)
        );
        let handle = sftp
            .session
            .open(
                &path,
                OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::EXCLUDE,
                FileAttributes::empty(),
            )
            .await
            .unwrap()
            .handle;
        for (i, chunk) in source.as_bytes().chunks(32 * 1024).enumerate() {
            sftp.session
                .write(&handle, (i * 32 * 1024) as u64, chunk.to_vec())
                .await
                .unwrap();
        }
        sftp.session.close(handle).await.unwrap();
        let first = read_document(&sftp, &path, None, None).await.unwrap();
        assert!(first.content.len() <= PAGE_SIZE);
        assert!(first.page.is_some());
        // The editable reader and saving protocol cannot mistake a page for a complete file.
        assert!(super::super::read(&sftp, &path).await.is_err());
        let request = Request {
            id: server.id,
            operation: Uuid::new_v4(),
            root: "/Documents".into(),
            path: path.clone(),
            expected_connection: vec![],
            content: Some("partial overwrite".into()),
            revision: Some(first.revision.clone()),
            offset: Some(0),
            allow_cached: false,
        };
        assert!(
            save(&Operations::default(), sftp.clone(), &request)
                .await
                .unwrap_err()
                .to_string()
                .contains("read-only")
        );
        assert!(
            read_document(&sftp, &path, Some(0), Some("stale"))
                .await
                .is_err()
        );
        let mut assembled = first.content.clone();
        let mut next = first.page.as_ref().unwrap().next;
        while let Some(offset) = next {
            let page = read_document(&sftp, &path, Some(offset), Some(&first.revision))
                .await
                .unwrap();
            assert!(page.content.len() <= PAGE_SIZE);
            next = page.page.unwrap().next;
            assembled.push_str(&page.content);
        }
        assert_eq!(assembled, source);
        assert_eq!(
            read_document(&sftp, &path, Some(0), Some(&first.revision))
                .await
                .unwrap()
                .content,
            first.content
        );
        assert!(
            read_document(
                &sftp,
                &path,
                Some(source.len() as u64),
                Some(&first.revision)
            )
            .await
            .is_err()
        );
        // An observed replacement invalidates the old viewer; PDF stays out of the text path.
        let handle = sftp
            .session
            .open(&path, OpenFlags::WRITE, FileAttributes::empty())
            .await
            .unwrap()
            .handle;
        sftp.session
            .write(&handle, 0, b"changed".to_vec())
            .await
            .unwrap();
        assert!(
            read_document(&sftp, &path, Some(0), Some(&first.revision))
                .await
                .is_err()
        );
        sftp.session
            .write(&handle, 0, b"%PDF-1.7".to_vec())
            .await
            .unwrap();
        assert!(
            read_document(&sftp, &path, None, None)
                .await
                .unwrap_err()
                .to_string()
                .contains("binary")
        );
        sftp.session.close(handle).await.unwrap();
        sftp.session.remove(&path).await.unwrap();
    }
    #[test]
    fn page_boundaries_preserve_utf8_crlf_and_long_lines() {
        let source = format!("{}世界\r\nlast", "x".repeat(PAGE_SIZE - 1));
        let first = page_text(source.as_bytes()[..PAGE_SIZE].to_vec(), true).unwrap();
        let second = page_text(source.as_bytes()[first.len()..].to_vec(), false).unwrap();
        assert_eq!(first + &second, source);
        assert_eq!(page_text(b"line\r".to_vec(), true).unwrap(), "line");
        assert!(page_text(vec![0xff], true).is_err());
        assert!(page_text(vec![0xe4], false).is_err());
        assert!(page_text(b"%PDF-1.7".to_vec(), false).is_err());
        assert!(page_text(b"text\0binary".to_vec(), false).is_err());
    }
}
