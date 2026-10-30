use super::{Operations, Remote, canonical, child, path_valid, status};
use crate::manager::Shared;
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
        self.0.files.listings.invalidate(|_| true);
        self.0.files.documents.invalidate(|_, _| true);
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
    let expected_connection = serde_json::Value::Array(request.expected_connection);
    operations
        .run_with_connection(
            state,
            request.id,
            request.operation,
            Some(&expected_connection),
            async |_server, owner| {
                let remote = owner.files.remote(&owner);
                let _invalidate = uploading.then(|| ListingInvalidation(owner));
                path_valid(&request.path)?;
                operations.retry_upload_cleanup(remote.clone());
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
                    upload(operations, &remote, &local, &request.path, progress)
                        .await
                        .map(Some)
                } else {
                    download(operations, &remote, &request.path, &local, progress).await?;
                    Ok(Some(local.to_string_lossy().into_owned()))
                }
            },
        )
        .await
}

// Cancellation drops the operation future. Remove the staging file over the
// server's shared session; never publish an incomplete upload.
pub(super) struct RemoteTemp<'a> {
    pub(super) operations: &'a Operations,
    pub(super) remote: Remote,
    pub(super) record: Option<super::operations::recovery::Upload>,
    pub(super) created: bool,
}
impl Drop for RemoteTemp<'_> {
    fn drop(&mut self) {
        if let Some(record) = self.record.take() {
            self.operations
                .cleanup_upload(self.remote.clone(), record, self.created);
        }
    }
}

/// Uploads `local` into `folder` on an SFTP session of its own, so it never
/// holds up other file operations.
pub async fn upload(
    operations: &Operations,
    remote: &Remote,
    local: &Path,
    folder: &str,
    progress: impl Fn(u64, Option<u64>),
) -> Result<String> {
    let name = local
        .file_name()
        .and_then(|name| name.to_str())
        .context("File name must be valid Unicode")?;
    let session = remote.sftp.dedicated().await?;
    let session = &session;
    let folder = canonical(session, folder).await?;
    let destination = child(&folder, name)?;
    match session.lstat(&destination).await {
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
        .track_upload(remote.cleanup_scope()?, staging.clone())
        .await?;
    let mut cleanup = RemoteTemp {
        operations,
        remote: remote.clone(),
        record: Some(record),
        created: false,
    };
    let handle = session
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
    let open = &handle;
    let offset = write_file(
        &mut source,
        |offset, data| async move {
            session.write(open, offset, data).await?;
            Ok(())
        },
        |offset| progress(offset, Some(total)),
    )
    .await?;
    session.close(handle).await?;
    if offset != total
        || !same_source(&metadata, &source.metadata().await?)?
        || !same_source(
            &metadata,
            &tokio::fs::metadata(local)
                .await
                .context("Local file changed during upload. Please try again.")?,
        )?
    {
        bail!("Local file changed during upload. Please try again.");
    }
    // Standard SFTP v3 rename must fail when the destination already exists.
    session
        .rename(&staging, &destination)
        .await
        .context("Could not finish upload; destination may already exist")?;
    let _ = operations
        .finish_upload(cleanup.record.take().unwrap())
        .await;
    Ok(destination)
}

// A pipelined upload can reach EOF before its first acknowledged write. Check
// both the open file and selected path again before publishing the remote file.
fn same_source(before: &std::fs::Metadata, after: &std::fs::Metadata) -> std::io::Result<bool> {
    if !after.is_file() || before.len() != after.len() || before.modified()? != after.modified()? {
        return Ok(false);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // Not ctime: metadata-only changes (Spotlight, quarantine and
        // last-used attributes on macOS) bump it without touching content.
        if (before.dev(), before.ino()) != (after.dev(), after.ino()) {
            return Ok(false);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if before.creation_time() != after.creation_time() {
            return Ok(false);
        }
    }
    Ok(true)
}
/// Size of one read or write request. 64 KiB is accepted by every SFTP server
/// (OpenSSH allows up to 255 KiB per request, but older servers cap a read at
/// 64 KiB, which would force extra round trips).
const CHUNK: usize = 64 * 1024;
/// Requests kept in flight at once per file.
const DEPTH: usize = 16;

/// Requests in flight at once; a completed one is returned as soon as it is
/// ready, whatever its start order. Dropping the set cancels the rest.
struct InFlight<F>(Vec<std::pin::Pin<Box<F>>>);
impl<F: std::future::Future> InFlight<F> {
    fn new() -> Self {
        Self(Vec::with_capacity(DEPTH))
    }
    fn len(&self) -> usize {
        self.0.len()
    }
    fn push(&mut self, future: F) {
        self.0.push(Box::pin(future));
    }
    /// Waits for any request to complete. Must not be called when empty.
    async fn next(&mut self) -> F::Output {
        std::future::poll_fn(|cx| {
            for index in 0..self.0.len() {
                if let std::task::Poll::Ready(output) = self.0[index].as_mut().poll(cx) {
                    drop(self.0.swap_remove(index));
                    return std::task::Poll::Ready(output);
                }
            }
            std::task::Poll::Pending
        })
        .await
    }
}

/// Copies `source` from its current position into an open remote file from
/// offset 0 with up to [`DEPTH`] write requests of [`CHUNK`] bytes in flight.
/// `progress` reports acknowledged bytes. Returns the bytes written. Any error
/// cancels the writes still in flight.
pub(crate) async fn write_file<W, F>(
    source: &mut (impl tokio::io::AsyncRead + Unpin),
    mut write: W,
    progress: impl Fn(u64),
) -> Result<u64>
where
    W: FnMut(u64, Vec<u8>) -> F,
    F: std::future::Future<Output = Result<()>>,
{
    let mut offset = 0u64;
    let mut acknowledged = 0u64;
    let mut inflight = InFlight::new();
    progress(0);
    loop {
        let mut buffer = vec![0; CHUNK];
        let mut filled = 0;
        while filled < CHUNK {
            let count = source.read(&mut buffer[filled..]).await?;
            if count == 0 {
                break;
            }
            filled += count;
        }
        if filled == 0 {
            break;
        }
        buffer.truncate(filled);
        if inflight.len() == DEPTH {
            acknowledged += inflight.next().await?;
            progress(acknowledged);
        }
        let request = write(offset, buffer);
        let length = filled as u64;
        inflight.push(async move { request.await.map(|()| length) });
        offset += length;
    }
    while inflight.len() > 0 {
        acknowledged += inflight.next().await?;
        progress(acknowledged);
    }
    Ok(offset)
}

/// The remote file holds more than the caller allowed.
#[derive(Debug)]
pub(crate) struct LimitExceeded;
impl std::fmt::Display for LimitExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("The remote file is larger than allowed")
    }
}
impl std::error::Error for LimitExceeded {}

/// Copies an open remote file from offset 0 into `destination` with up to
/// [`DEPTH`] read requests of [`CHUNK`] bytes in flight, assembled in order.
/// `read` returns None (or no data) at the end of the file; a reply shorter
/// than requested is re-requested for the missing part. `size`, when known,
/// stops requests beyond the expected end except one probe for the real end.
/// Any error cancels the reads still in flight. Returns the bytes copied.
/// With `max`, data beyond it fails with [`LimitExceeded`] before it reaches
/// `destination`.
pub(crate) async fn read_file_capped<R, F>(
    destination: &mut (impl tokio::io::AsyncWrite + Unpin),
    mut read: R,
    size: Option<u64>,
    max: Option<u64>,
    progress: impl Fn(u64),
) -> Result<u64>
where
    R: FnMut(u64, u32) -> F,
    F: std::future::Future<Output = Result<Option<Vec<u8>>>>,
{
    let mut size = size;
    let mut next = 0u64;
    let mut written = 0u64;
    // The lowest offset known to be at or past the end.
    let mut end: Option<u64> = None;
    let mut missing: Vec<(u64, u32)> = Vec::new();
    let mut arrived: std::collections::BTreeMap<u64, Vec<u8>> = Default::default();
    let mut inflight = InFlight::new();
    progress(0);
    loop {
        // Bounded by the window, so a slow request holds back at most DEPTH chunks.
        while inflight.len() < DEPTH {
            let (offset, length) = if let Some(gap) = missing.pop() {
                if end.is_some_and(|end| gap.0 >= end) {
                    continue;
                }
                gap
            } else if next >= written + (DEPTH * CHUNK) as u64
                || end.is_some_and(|end| next >= end)
                || size.is_some_and(|size| next > size)
            {
                break;
            } else {
                let length = match size {
                    Some(size) if next < size => (size - next).min(CHUNK as u64) as u32,
                    _ => CHUNK as u32,
                };
                next += u64::from(length);
                (next - u64::from(length), length)
            };
            let request = read(offset, length);
            inflight.push(async move { (offset, length, request.await) });
        }
        if inflight.len() == 0 {
            break;
        }
        let (offset, length, reply) = inflight.next().await;
        match reply? {
            Some(data) if !data.is_empty() => {
                if data.len() as u64 > u64::from(length) {
                    bail!("The server sent more data than was requested");
                }
                if size.is_some_and(|size| offset >= size) {
                    size = None;
                }
                if (data.len() as u64) < u64::from(length) {
                    let got = data.len() as u32;
                    missing.push((offset + u64::from(got), length - got));
                }
                arrived.insert(offset, data);
            }
            _ => end = Some(end.map_or(offset, |end| end.min(offset))),
        }
        while let Some(data) = arrived.remove(&written) {
            if end.is_some_and(|end| written >= end) {
                break;
            }
            let keep = end.map_or(data.len() as u64, |end| {
                (end - written).min(data.len() as u64)
            });
            if max.is_some_and(|max| written.saturating_add(keep) > max) {
                return Err(LimitExceeded.into());
            }
            destination.write_all(&data[..keep as usize]).await?;
            written += keep;
            progress(written);
        }
    }
    if end.is_none_or(|end| written < end) {
        bail!("Remote read ended with a gap");
    }
    Ok(written)
}
const CHANGED: &str = "Remote file changed during download. Please try again.";

/// Reads a download of the size `stat` reported. More bytes than that mean the
/// file changed, so the read stops there instead of filling the disk.
async fn read_download<R, F>(
    destination: &mut (impl tokio::io::AsyncWrite + Unpin),
    read: R,
    size: Option<u64>,
    progress: impl Fn(u64),
) -> Result<u64>
where
    R: FnMut(u64, u32) -> F,
    F: std::future::Future<Output = Result<Option<Vec<u8>>>>,
{
    read_file_capped(destination, read, size, size, progress)
        .await
        .map_err(|error| {
            if error.is::<LimitExceeded>() {
                anyhow::anyhow!(CHANGED)
            } else {
                error
            }
        })
}
/// Downloads `path` to `local` on an SFTP session of its own, so it never
/// holds up other file operations.
pub async fn download(
    operations: &Operations,
    remote: &Remote,
    path: &str,
    local: &Path,
    progress: impl Fn(u64, Option<u64>),
) -> Result<()> {
    path_valid(path)?;
    let session = remote.sftp.dedicated().await?;
    let session = &session;
    let attrs = session.stat(path).await?.attrs;
    if !attrs.is_regular() {
        bail!("Only regular files can be downloaded");
    }
    let parent = local
        .parent()
        .context("Download destination has no parent folder")?;
    let mut cleanup = operations
        .stage_download(remote.server_id(), parent)
        .await?;
    let result = async {
        let mut destination =
            tokio::fs::File::from_std(cleanup.staging.as_ref().unwrap().reopen()?);
        let handle = session
            .open(path, OpenFlags::READ, FileAttributes::empty())
            .await?
            .handle;
        let open = &handle;
        let offset = read_download(
            &mut destination,
            |offset, len| async move {
                match session.read(open, offset, len).await {
                    Ok(data) => Ok(Some(data.data)),
                    Err(e) if status(&e, StatusCode::Eof) => Ok(None),
                    Err(e) => Err(e.into()),
                }
            },
            attrs.size,
            |offset| progress(offset, attrs.size),
        )
        .await?;
        session.close(handle).await?;
        let after = session.stat(path).await?.attrs;
        if attrs.size.is_some_and(|size| size != offset)
            || attrs.size != after.size
            || attrs.mtime != after.mtime
        {
            bail!(CHANGED);
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
mod pipelining_tests {
    use super::{
        CHANGED, CHUNK, DEPTH, LimitExceeded, read_download, read_file_capped, same_source,
        write_file,
    };
    use anyhow::{Result, bail};

    async fn read_file<R, F>(
        destination: &mut (impl tokio::io::AsyncWrite + Unpin),
        read: R,
        size: Option<u64>,
        progress: impl Fn(u64),
    ) -> Result<u64>
    where
        R: FnMut(u64, u32) -> F,
        F: std::future::Future<Output = Result<Option<Vec<u8>>>>,
    {
        read_file_capped(destination, read, size, None, progress).await
    }
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    use std::time::Duration;

    #[tokio::test]
    async fn source_changes_after_eof_are_detected_before_publication() {
        use std::io::Write;
        for mutation in ["append", "rewrite", "replace", "permissions", "unchanged"] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("source");
            std::fs::write(&path, vec![1; CHUNK * 4]).unwrap();
            let mut source = tokio::fs::File::open(&path).await.unwrap();
            let before = source.metadata().await.unwrap();
            let changed = std::sync::atomic::AtomicBool::new(false);
            let offset = write_file(
                &mut source,
                |_, _| async { Ok(()) },
                |done| {
                    if done == 0 || changed.swap(true, Ordering::SeqCst) {
                        return;
                    }
                    match mutation {
                        "append" => std::fs::OpenOptions::new()
                            .append(true)
                            .open(&path)
                            .unwrap()
                            .write_all(b"more")
                            .unwrap(),
                        "rewrite" => {
                            std::fs::write(&path, vec![2; CHUNK * 4]).unwrap();
                            std::fs::File::options()
                                .write(true)
                                .open(&path)
                                .unwrap()
                                .set_modified(before.modified().unwrap() + Duration::from_secs(1))
                                .unwrap();
                        }
                        "replace" => {
                            std::fs::rename(&path, directory.path().join("original")).unwrap();
                            std::fs::write(&path, vec![3; CHUNK * 4]).unwrap();
                            std::fs::File::options()
                                .write(true)
                                .open(&path)
                                .unwrap()
                                .set_modified(before.modified().unwrap() + Duration::from_secs(1))
                                .unwrap();
                        }
                        // A metadata-only change keeps the content.
                        "permissions" => {
                            let mut permissions = std::fs::metadata(&path).unwrap().permissions();
                            permissions.set_readonly(true);
                            std::fs::set_permissions(&path, permissions).unwrap();
                        }
                        "unchanged" => (),
                        _ => unreachable!(),
                    }
                },
            )
            .await
            .unwrap();
            assert!(changed.load(Ordering::SeqCst));
            assert_eq!(
                offset,
                before.len(),
                "the source reached EOF before mutation"
            );
            let unchanged = same_source(&before, &source.metadata().await.unwrap()).unwrap()
                && same_source(&before, &std::fs::metadata(&path).unwrap()).unwrap();
            assert_eq!(
                unchanged,
                matches!(mutation, "permissions" | "unchanged"),
                "{mutation}"
            );
        }
    }

    /// Requests started, running and finished, and the most running at once.
    #[derive(Default)]
    struct Load {
        started: AtomicUsize,
        running: AtomicUsize,
        most: AtomicUsize,
        dropped: AtomicUsize,
    }
    /// Counts one request while it runs; a request dropped before it ends
    /// counts as cancelled.
    struct Running(Arc<Load>, bool);
    impl Running {
        fn new(load: &Arc<Load>) -> Self {
            load.started.fetch_add(1, Ordering::AcqRel);
            let running = load.running.fetch_add(1, Ordering::AcqRel) + 1;
            load.most.fetch_max(running, Ordering::AcqRel);
            Self(load.clone(), false)
        }
        fn finish(mut self) {
            self.1 = true;
        }
    }
    impl Drop for Running {
        fn drop(&mut self) {
            self.0.running.fetch_sub(1, Ordering::AcqRel);
            if !self.1 {
                self.0.dropped.fetch_add(1, Ordering::AcqRel);
            }
        }
    }

    fn pattern(length: usize) -> Vec<u8> {
        (0..length).map(|index| (index % 251) as u8).collect()
    }
    /// Earlier chunks answer later than later ones.
    fn delay(offset: u64) -> Duration {
        Duration::from_millis(200u64.saturating_sub(offset / CHUNK as u64 * 7))
    }
    fn monotonic() -> (Arc<Mutex<Vec<u64>>>, impl Fn(u64)) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let record = seen.clone();
        (seen, move |done| record.lock().unwrap().push(done))
    }
    fn assert_monotonic(seen: &Mutex<Vec<u64>>, last: Option<u64>) {
        let seen = seen.lock().unwrap();
        assert_eq!(seen[0], 0);
        assert!(seen.windows(2).all(|pair| pair[0] <= pair[1]), "{seen:?}");
        if let Some(last) = last {
            assert_eq!(seen.last(), Some(&last));
        }
    }

    /// A remote file whose replies come back in reverse order.
    struct Remote {
        data: Vec<u8>,
        load: Arc<Load>,
        offsets: Arc<Mutex<Vec<u64>>>,
        /// The most bytes one reply holds.
        cap: usize,
        fail_at: Option<u64>,
    }
    /// One pending read: the bytes at an offset, or `None` at the end.
    type Read =
        std::pin::Pin<Box<dyn std::future::Future<Output = Result<Option<Vec<u8>>>> + Send>>;

    impl Remote {
        fn new(data: Vec<u8>) -> Self {
            Self {
                data,
                load: Arc::default(),
                offsets: Arc::default(),
                cap: usize::MAX,
                fail_at: None,
            }
        }
        fn reader(&self) -> impl FnMut(u64, u32) -> Read + use<> {
            let data = self.data.clone();
            let load = self.load.clone();
            let offsets = self.offsets.clone();
            let (cap, fail_at) = (self.cap, self.fail_at);
            move |offset, length| {
                let (data, load, offsets) = (data.clone(), load.clone(), offsets.clone());
                Box::pin(async move {
                    offsets.lock().unwrap().push(offset);
                    let running = Running::new(&load);
                    tokio::time::sleep(delay(offset)).await;
                    if fail_at == Some(offset) {
                        bail!("Connection lost");
                    }
                    running.finish();
                    let start = (offset as usize).min(data.len());
                    let end = (start + (length as usize).min(cap)).min(data.len());
                    Ok((end > start).then(|| data[start..end].to_vec()))
                })
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn reads_complete_out_of_order_and_assemble_in_order() {
        for size in [None, Some(40 * CHUNK as u64 + 11)] {
            let data = pattern(40 * CHUNK + 11);
            let remote = Remote::new(data.clone());
            let (seen, progress) = monotonic();
            let mut copy = Vec::new();
            let copied = read_file(&mut copy, remote.reader(), size, progress)
                .await
                .unwrap();
            assert_eq!(copied, data.len() as u64);
            assert!(copy == data, "bytes differ");
            assert_monotonic(&seen, Some(data.len() as u64));
            // Several requests overlap, and never more than the window.
            let most = remote.load.most.load(Ordering::Acquire);
            assert!((2..=DEPTH).contains(&most), "{most}");
            assert_eq!(most, DEPTH);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_known_size_reads_nothing_beyond_one_probe_for_the_end() {
        for length in [40 * CHUNK + 11, 40 * CHUNK, 0, 5] {
            let remote = Remote::new(pattern(length));
            let mut copy = Vec::new();
            read_file(&mut copy, remote.reader(), Some(length as u64), |_| {})
                .await
                .unwrap();
            assert_eq!(copy.len(), length);
            let mut offsets = remote.offsets.lock().unwrap().clone();
            offsets.sort();
            let chunks = length.div_ceil(CHUNK);
            // The chunks, then the one request that finds the end.
            assert_eq!(offsets.len(), chunks + 1, "{length}");
            assert_eq!(*offsets.last().unwrap(), length as u64);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn the_end_in_the_middle_of_a_batch_stops_at_the_last_byte() {
        // Unknown size: requests beyond the end are answered with the end.
        let data = pattern(3 * CHUNK + 5);
        let remote = Remote::new(data.clone());
        let mut copy = Vec::new();
        let copied = read_file(&mut copy, remote.reader(), None, |_| {})
            .await
            .unwrap();
        assert_eq!(copied, data.len() as u64);
        assert!(copy == data);
        // The window holds back requests while the first chunk is outstanding.
        assert!(remote.offsets.lock().unwrap().len() <= 3 + 1 + DEPTH);
        // A file that turns out longer than the size announced is read whole.
        let data = pattern(5 * CHUNK + 5);
        let remote = Remote::new(data.clone());
        let mut copy = Vec::new();
        read_file(&mut copy, remote.reader(), Some(2 * CHUNK as u64), |_| {})
            .await
            .unwrap();
        assert!(copy == data);
        // A file that turns out shorter ends at the real end.
        let data = pattern(CHUNK + 5);
        let remote = Remote::new(data.clone());
        let mut copy = Vec::new();
        read_file(&mut copy, remote.reader(), Some(9 * CHUNK as u64), |_| {})
            .await
            .unwrap();
        assert!(copy == data);
    }

    #[tokio::test(start_paused = true)]
    async fn a_reply_longer_than_its_request_is_refused() {
        let long = |_: u64, length: u32| async move { Ok(Some(vec![1u8; length as usize + 1])) };
        for (size, max) in [(Some(10), None), (None, None), (None, Some(1 << 20))] {
            let mut copy = Vec::new();
            let error = read_file_capped(&mut copy, long, size, max, |_| {})
                .await
                .unwrap_err();
            assert!(error.to_string().contains("more data"), "{error}");
            assert!(copy.is_empty());
        }
    }

    #[tokio::test]
    async fn a_download_stops_at_its_reported_size_when_the_server_keeps_sending() {
        let endless = |_: u64, length: u32| async move { Ok(Some(vec![1u8; length as usize])) };
        let mut copy = Vec::new();
        let error = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            read_download(&mut copy, endless, Some(1024), |_| {}),
        )
        .await
        .expect("a lying server must not keep the download reading")
        .unwrap_err();
        assert_eq!(error.to_string(), CHANGED);
        assert_eq!(copy.len(), 1024);
    }

    #[tokio::test(start_paused = true)]
    async fn a_cap_fails_a_file_that_yields_more_than_allowed() {
        // Announced empty, yet data arrives.
        let remote = Remote::new(pattern(5));
        let mut copy = Vec::new();
        let error = read_file_capped(&mut copy, remote.reader(), Some(0), Some(0), |_| {})
            .await
            .unwrap_err();
        assert!(error.is::<LimitExceeded>());
        assert!(copy.is_empty());
        // Grown past the announced size, by a few bytes and by many chunks.
        for length in [3 * CHUNK + 1, 40 * CHUNK] {
            let remote = Remote::new(pattern(length));
            let mut copy = Vec::new();
            let error = read_file_capped(
                &mut copy,
                remote.reader(),
                Some(3 * CHUNK as u64),
                Some(3 * CHUNK as u64),
                |_| {},
            )
            .await
            .unwrap_err();
            assert!(error.is::<LimitExceeded>(), "{length}");
            assert!(copy.len() <= 3 * CHUNK);
        }
        // Exactly the cap is fine.
        let remote = Remote::new(pattern(3 * CHUNK));
        let mut copy = Vec::new();
        let copied = read_file_capped(
            &mut copy,
            remote.reader(),
            Some(3 * CHUNK as u64),
            Some(3 * CHUNK as u64),
            |_| {},
        )
        .await
        .unwrap();
        assert_eq!(copied, 3 * CHUNK as u64);
    }

    #[tokio::test(start_paused = true)]
    async fn short_replies_are_completed_without_gaps_or_repeats() {
        let data = pattern(10 * CHUNK + 3);
        let mut remote = Remote::new(data.clone());
        remote.cap = 10_000;
        let mut copy = Vec::new();
        let copied = read_file(&mut copy, remote.reader(), None, |_| {})
            .await
            .unwrap();
        assert_eq!(copied, data.len() as u64);
        assert!(copy == data);
    }

    #[tokio::test(start_paused = true)]
    async fn a_read_error_cancels_the_requests_still_in_flight() {
        let data = pattern(40 * CHUNK);
        let mut remote = Remote::new(data.clone());
        remote.fail_at = Some(3 * CHUNK as u64);
        let (seen, progress) = monotonic();
        let mut copy = Vec::new();
        let error = read_file(&mut copy, remote.reader(), None, progress)
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "Connection lost");
        // Nothing past the failed chunk was written or requested afterwards.
        assert!(copy.len() <= 3 * CHUNK);
        assert!(copy[..] == data[..copy.len()]);
        assert!(remote.offsets.lock().unwrap().len() <= 3 + DEPTH);
        assert_eq!(remote.load.running.load(Ordering::Acquire), 0);
        assert!(remote.load.dropped.load(Ordering::Acquire) > 1);
        assert_monotonic(&seen, None);
    }

    #[tokio::test(start_paused = true)]
    async fn a_failing_destination_ends_the_copy_and_cancels_the_reads() {
        struct Full;
        impl tokio::io::AsyncWrite for Full {
            fn poll_write(
                self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
                _: &[u8],
            ) -> std::task::Poll<std::io::Result<usize>> {
                std::task::Poll::Ready(Err(std::io::Error::other("Disk full")))
            }
            fn poll_flush(
                self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                std::task::Poll::Ready(Ok(()))
            }
            fn poll_shutdown(
                self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                std::task::Poll::Ready(Ok(()))
            }
        }
        let remote = Remote::new(pattern(8 * CHUNK));
        let error = read_file(&mut Full, remote.reader(), None, |_| {})
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "Disk full");
        assert_eq!(remote.load.running.load(Ordering::Acquire), 0);
    }

    /// A remote file receiving writes that complete in reverse order.
    struct Sink {
        file: Arc<Mutex<Vec<u8>>>,
        load: Arc<Load>,
        fail_at: Option<u64>,
        offsets: Arc<Mutex<Vec<u64>>>,
    }
    impl Sink {
        fn new() -> Self {
            Self {
                file: Arc::default(),
                load: Arc::default(),
                fail_at: None,
                offsets: Arc::default(),
            }
        }
        fn writer(
            &self,
        ) -> impl FnMut(
            u64,
            Vec<u8>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<()>> + Send>,
        > + use<> {
            let (file, load, offsets) =
                (self.file.clone(), self.load.clone(), self.offsets.clone());
            let fail_at = self.fail_at;
            move |offset, data| {
                let (file, load, offsets) = (file.clone(), load.clone(), offsets.clone());
                Box::pin(async move {
                    offsets.lock().unwrap().push(offset);
                    let running = Running::new(&load);
                    if fail_at == Some(offset) {
                        bail!("No space left on the server");
                    }
                    tokio::time::sleep(delay(offset)).await;
                    running.finish();
                    let mut file = file.lock().unwrap();
                    let end = offset as usize + data.len();
                    if file.len() < end {
                        file.resize(end, 0);
                    }
                    file[offset as usize..end].copy_from_slice(&data);
                    Ok(())
                })
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn writes_land_at_their_offsets_with_a_full_window_in_flight() {
        for length in [0, 7, CHUNK, 40 * CHUNK + 7] {
            let data = pattern(length);
            let sink = Sink::new();
            let (seen, progress) = monotonic();
            let written = write_file(&mut &data[..], sink.writer(), progress)
                .await
                .unwrap();
            assert_eq!(written, length as u64);
            assert!(*sink.file.lock().unwrap() == data, "{length}");
            assert_monotonic(&seen, Some(length as u64));
            let most = sink.load.most.load(Ordering::Acquire);
            if length > DEPTH * CHUNK {
                assert_eq!(most, DEPTH);
            }
            assert!(most <= DEPTH);
            assert_eq!(sink.offsets.lock().unwrap().len(), length.div_ceil(CHUNK));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_write_error_cancels_the_writes_in_flight_and_sends_no_more() {
        let data = pattern(60 * CHUNK);
        let mut sink = Sink::new();
        sink.fail_at = Some(20 * CHUNK as u64);
        let (seen, progress) = monotonic();
        let error = write_file(&mut &data[..], sink.writer(), progress)
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "No space left on the server");
        // Nothing is sent once the failing write is answered.
        assert!(sink.offsets.lock().unwrap().len() <= 21 + DEPTH);
        assert_eq!(sink.load.running.load(Ordering::Acquire), 0);
        assert!(sink.load.dropped.load(Ordering::Acquire) > 1);
        assert_monotonic(&seen, None);
    }
}
