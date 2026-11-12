//! Run only inside the disposable window station created by the native harness.
use super::super::Reply as SourceReply;
use super::*;
use crate::windows_limits::MAX_IMAGE_BYTES;
use crate::{CONCEALED_FORMAT, FILES_FORMAT};
use std::{collections::BTreeMap, path::Path, sync::Mutex, time::Duration};
#[path = "fixture.rs"]
mod fixture;
struct Bytes(Vec<u8>);
impl Payload for Bytes {
    fn bytes(&self) -> Option<&[u8]> {
        Some(&self.0)
    }
}
struct TestSource {
    current: AtomicBool,
    requests: AtomicUsize,
    values: BTreeMap<String, Vec<u8>>,
}
impl Source for TestSource {
    fn is_current(&self) -> bool {
        self.current.load(Ordering::Acquire)
    }
    fn request(&self, mime: &str, reply: SourceReply) {
        self.requests.fetch_add(1, Ordering::AcqRel);
        if let Some(bytes) = self.values.get(mime) {
            reply.complete(Box::new(Bytes(bytes.clone())));
        }
    }
    fn publication_revoked(&self) {
        self.current.store(false, Ordering::Release);
    }
}
fn snapshot(runtime: &tokio::runtime::Runtime, owner: &Owner) -> Snapshot {
    let mut result = None;
    fixture::wait(|| {
        result = runtime.block_on(owner.snapshot());
        result.is_some()
    });
    result.unwrap()
}
#[test]
#[ignore = "requires an owned noninteractive Windows station"]
fn isolated_native_owner_formats_privacy_and_shutdown() {
    fixture::isolated_station();
    let native = fixture::Native::new();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let owner = Owner::with_worker(WorkerExecutable::fixture());
    let unicode = clipboard_win::formats::CF_UNICODETEXT;
    let history = clipboard_win::register_format("CanIncludeInClipboardHistory")
        .unwrap()
        .get();
    for (policy, concealed) in [
        (vec![1, 0, 0, 0], false),
        (vec![0; 4], true),
        (vec![2; 4], true),
    ] {
        native.replace(vec![
            (unicode, fixture::text("native é\r\nfixture")),
            (history, policy),
        ]);
        let offer = snapshot(&runtime, &owner);
        assert_eq!(
            offer.formats.iter().any(|mime| mime == CONCEALED_FORMAT),
            concealed,
            "privacy classification"
        );
        let value = runtime
            .block_on(owner.read(offer.revision, "text/plain".into()))
            .unwrap();
        assert_eq!(value.bytes().unwrap(), "native é\nfixture".as_bytes());
    }
    native.replace(vec![(unicode, fixture::text("before publication"))]);
    let expected = snapshot(&runtime, &owner).revision;
    let source = Arc::new(TestSource {
        current: AtomicBool::new(true),
        requests: AtomicUsize::new(0),
        values: BTreeMap::from([
            ("text/plain".into(), b"remote fixture".to_vec()),
            ("image/png".into(), fixture::png()),
        ]),
    });
    let publication = runtime
        .block_on(owner.publish(
            expected,
            vec![
                "text/plain".into(),
                "image/png".into(),
                CONCEALED_FORMAT.into(),
            ],
            source.clone(),
        ))
        .unwrap();
    assert_eq!(
        source.requests.load(Ordering::Acquire),
        0,
        "publication must remain lazy"
    );
    fixture::assert_text(&native.read(unicode).unwrap(), "remote fixture");
    let dib = native.read(clipboard_win::formats::CF_DIBV5).unwrap();
    fixture::assert_pixels(
        &crate::windows_bitmap::dib_to_png(&dib, MAX_IMAGE_BYTES, MAX_BYTES).unwrap(),
    );
    assert!(
        native.read(clipboard_win::formats::CF_DIB).is_ok(),
        "Windows synthesizes legacy DIB"
    );
    assert!(
        publication.is_current(),
        "native conversion must retain publication ownership"
    );
    assert_eq!(
        snapshot(&runtime, &owner).origin,
        Some(publication.generation)
    );
    assert_eq!(&native.read(history).unwrap()[..4], &[0; 4]);
    drop(publication);
    native.replace(vec![(unicode, fixture::text("after publication"))]);
    let before = snapshot(&runtime, &owner);
    let release = native.hold();
    let source = Arc::new(TestSource {
        current: AtomicBool::new(true),
        requests: AtomicUsize::new(0),
        values: BTreeMap::from([("text/plain".into(), b"canceled".to_vec())]),
    });
    let future = owner.publish(before.revision, vec!["text/plain".into()], source.clone());
    let task = runtime.spawn(future);
    std::thread::sleep(Duration::from_millis(50));
    task.abort();
    fixture::wait(|| !source.is_current());
    release.store(true, Ordering::Release);
    fixture::assert_text(&native.read(unicode).unwrap(), "after publication");
    let (release, entered) = native.delayed();
    let offer = snapshot(&runtime, &owner);
    let reader_owner = owner.clone();
    let read =
        runtime.spawn(async move { reader_owner.read(offer.revision, "text/plain".into()).await });
    fixture::wait(|| entered.load(Ordering::Acquire));
    runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(1), owner.shutdown())
            .await
            .expect("shutdown must not wait for the foreign delayed owner")
            .expect("helper must be reaped");
    });
    read.abort();
    release.store(true, Ordering::Release);
}

// The hook runs on the publishing native thread after releasing the clipboard.
// A second HWND copies before acknowledgment, without depending on scheduler timing.
type CloseHook = (std::thread::ThreadId, Box<dyn FnOnce() + Send>);
static AFTER_CLOSE: Mutex<Option<CloseHook>> = Mutex::new(None);
pub(super) fn after_native_close() {
    let action = {
        let mut hook = AFTER_CLOSE.lock().unwrap();
        if hook
            .as_ref()
            .is_some_and(|(thread, _)| *thread == std::thread::current().id())
        {
            hook.take().map(|(_, action)| action)
        } else {
            None
        }
    };
    if let Some(action) = action {
        action();
    }
}
struct RacingSource(TestSource);
impl Source for RacingSource {
    fn is_current(&self) -> bool {
        self.0.is_current()
    }
    fn request(&self, mime: &str, reply: SourceReply) {
        self.0.request(mime, reply);
    }
    fn publication_revoked(&self) {
        self.0.publication_revoked();
    }
    fn publication_confirmed(&self) -> bool {
        *AFTER_CLOSE.lock().unwrap() = Some((
            std::thread::current().id(),
            Box::new(|| {
                fixture::copy_on_current_thread("newer local copy");
            }),
        ));
        true
    }
}
#[test]
#[ignore = "requires an owned noninteractive Windows station"]
fn a_copy_before_publication_acknowledgment_keeps_local_origin() {
    fixture::isolated_station();
    let native = fixture::Native::new();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let owner = Owner::with_worker(WorkerExecutable::fixture());
    native.replace(vec![(
        clipboard_win::formats::CF_UNICODETEXT,
        fixture::text("before publication"),
    )]);
    let expected = snapshot(&runtime, &owner).revision;
    let source = Arc::new(RacingSource(TestSource {
        current: AtomicBool::new(true),
        requests: AtomicUsize::new(0),
        values: BTreeMap::from([("text/plain".into(), b"remote copy".to_vec())]),
    }));
    let publication = runtime
        .block_on(owner.publish(expected, vec!["text/plain".into()], source.clone()))
        .unwrap();
    assert!(!publication.is_current());
    let offer = snapshot(&runtime, &owner);
    assert_eq!(
        offer.origin, None,
        "another owner's copy must never inherit imported provenance"
    );
    let data = runtime
        .block_on(owner.read(offer.revision, "text/plain".into()))
        .unwrap();
    assert_eq!(data.bytes().unwrap(), b"newer local copy");
    runtime
        .block_on(owner.shutdown())
        .expect("helper must be reaped");
}
#[test]
#[ignore = "requires an owned noninteractive Windows station"]
fn unsupported_file_publication_preserves_the_current_clipboard() {
    fixture::isolated_station();
    let native = fixture::Native::new();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let owner = Owner::with_worker(WorkerExecutable::fixture());
    native.replace(vec![(
        clipboard_win::formats::CF_UNICODETEXT,
        fixture::text("retained local copy"),
    )]);
    let expected = snapshot(&runtime, &owner).revision;
    let source = Arc::new(TestSource {
        current: AtomicBool::new(true),
        requests: AtomicUsize::new(0),
        values: BTreeMap::new(),
    });
    assert!(
        runtime
            .block_on(owner.publish(expected, vec![FILES_FORMAT.into()], source.clone()))
            .is_none()
    );
    assert!(!source.is_current());
    fixture::assert_text(
        &native.read(clipboard_win::formats::CF_UNICODETEXT).unwrap(),
        "retained local copy",
    );
    runtime
        .block_on(owner.shutdown())
        .expect("helper must be reaped");
}

#[test]
#[ignore = "child process for the isolated station harness; requires explicit termination"]
fn stalled_foreign_owner_process() {
    fixture::stalled_process();
}

#[test]
#[ignore = "requires the isolated station harness and a separately owned child process"]
fn cross_process_stalled_owner_does_not_block_shutdown() {
    let probe = fixture::Probe::new();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let owner = Owner::with_worker(WorkerExecutable::fixture());
    let operation = match fixture::probe_mode().as_str() {
        "render" => {
            let offer = snapshot(&runtime, &owner);
            let reader = owner.clone();
            runtime.spawn(async move {
                let _ = reader.read(offer.revision, "text/plain".into()).await;
            })
        }
        "privacy" => {
            let observer = owner.clone();
            runtime.spawn(async move {
                assert!(
                    observer.snapshot().await.is_none(),
                    "unclassified privacy cannot become an eligible offer"
                );
            })
        }
        "destroy" => {
            let offer = snapshot(&runtime, &owner);
            let source = Arc::new(TestSource {
                current: AtomicBool::new(true),
                requests: AtomicUsize::new(0),
                values: BTreeMap::from([(
                    "text/plain".into(),
                    b"owned publication fixture".to_vec(),
                )]),
            });
            let future = owner.publish(offer.revision, vec!["text/plain".into()], source);
            runtime.spawn(async move {
                let _ = future.await;
            })
        }
        _ => unreachable!(),
    };
    probe.blocked();
    runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(1), owner.shutdown())
            .await
            .expect("native owner must finish while the foreign callback is still blocked")
            .expect("blocked helper must be reaped");
    });
    operation.abort();
    if let Err(error) = runtime.block_on(operation) {
        assert!(error.is_cancelled(), "clipboard operation failed: {error}");
    }
    // The harness terminates only its child after observing shutdown completion.
    probe.terminated();
    let native = fixture::Native::new();
    native.replace(vec![(
        clipboard_win::formats::CF_UNICODETEXT,
        fixture::text("recovered local copy"),
    )]);
    let recovered = Owner::with_worker(WorkerExecutable::fixture());
    let offer = snapshot(&runtime, &recovered);
    assert!(offer.origin.is_none());
    let value = runtime
        .block_on(recovered.read(offer.revision, "text/plain".into()))
        .unwrap();
    assert_eq!(value.bytes().unwrap(), b"recovered local copy");
    runtime
        .block_on(recovered.shutdown())
        .expect("recovery helper must be reaped");
    probe.recovered();
}

#[test]
#[ignore = "helper entry requires the isolated station harness and inherited private pipe"]
fn clipboard_worker_entry() {
    fixture::isolated_station();
    process::run_from_stdio(open_files).expect("clipboard worker protocol");
}

#[test]
fn canceled_commit_cannot_be_admitted_and_claim_wins_later_cancellation() {
    let canceled = Attempt::new();
    drop(Cancel(canceled.clone()));
    assert!(!canceled.admit(|| panic!("canceled admission must not ask the source")));
    let committed = Attempt::new();
    assert!(committed.admit(|| true));
    drop(Cancel(committed.clone()));
    assert!(committed.claimed());
    assert!(!committed.canceled());
}

#[test]
fn uncertainty_requires_a_different_live_owner_and_changed_sequence() {
    let previous = Identity {
        owner: 13,
        sequence: 9,
    };
    assert!(!new_copy_after(
        previous,
        Identity {
            owner: 0,
            sequence: 10
        }
    ));
    assert!(!new_copy_after(
        previous,
        Identity {
            owner: 13,
            sequence: 10
        }
    ));
    assert!(!new_copy_after(
        previous,
        Identity {
            owner: 14,
            sequence: 9
        }
    ));
    assert!(new_copy_after(
        previous,
        Identity {
            owner: 14,
            sequence: 10
        }
    ));
}

struct FileSource(TestSource);
impl Source for FileSource {
    fn is_current(&self) -> bool {
        self.0.is_current()
    }
    fn request(&self, format: &str, reply: SourceReply) {
        self.0.request(format, reply);
    }
    fn preparation_confirmed(&self) -> bool {
        self.is_current()
    }
    fn publication_revoked(&self) {
        self.0.publication_revoked();
    }
}
struct FixtureFiles {
    folder: std::path::PathBuf,
    paths: Vec<std::path::PathBuf>,
    // Like the file cache's lease: a handle without delete sharing pins each file.
    _handles: Vec<std::fs::File>,
}
impl RetainedFiles for FixtureFiles {
    fn folder(&self) -> &Path {
        &self.folder
    }
    fn paths(&self) -> &[std::path::PathBuf] {
        &self.paths
    }
}
fn open_files(folder: &Path) -> io::Result<Files> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;
    let mut paths = Vec::new();
    let mut handles = Vec::new();
    for entry in std::fs::read_dir(folder)? {
        let path = entry?.path();
        handles.push(
            std::fs::OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ)
                .open(&path)?,
        );
        paths.push(path);
    }
    Ok(Arc::new(FixtureFiles {
        folder: folder.to_owned(),
        paths,
        _handles: handles,
    }))
}
struct FileFixture(std::path::PathBuf);
impl FileFixture {
    fn new() -> Self {
        let base = std::path::PathBuf::from(
            std::env::var_os("NEWPORT_CLIPBOARD_TEST_BASE")
                .expect("file clipboard fixture requires the isolated cache capability"),
        );
        assert!(base.is_absolute());
        std::fs::create_dir_all(base.parent().unwrap()).unwrap();
        Self(base.join(&uuid::Uuid::new_v4().simple().to_string()[..16]))
    }
    fn files(&self) -> Files {
        let folder = self.0.join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("clipboard-文書.txt"), b"abc").unwrap();
        open_files(&folder).unwrap()
    }
}
impl Drop for FileFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
/// Removing a pinned copy fails, so its folder survives the attempt.
fn pinned(folder: &Path) -> bool {
    let _ = std::fs::remove_dir_all(folder);
    folder.exists()
}
fn file_source() -> Arc<FileSource> {
    Arc::new(FileSource(TestSource {
        current: AtomicBool::new(true),
        requests: AtomicUsize::new(0),
        values: BTreeMap::from([("text/plain".into(), b"mixed remote text".to_vec())]),
    }))
}
#[test]
#[ignore = "requires an owned noninteractive station and isolated file cache"]
fn prepared_files_publish_hdrop_copy_effect_and_survive_paste_grace() {
    fixture::isolated_station();
    let native = fixture::Native::new();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    for mixed in [false, true] {
        let owner = Owner::with_worker(WorkerExecutable::fixture());
        let cache = FileFixture::new();
        let files = cache.files();
        let folder = files.folder().to_owned();
        let paths = files
            .paths()
            .iter()
            .map(|path| path.to_str().unwrap())
            .collect::<Vec<_>>();
        let expected = crate::windows_attachment::dropfiles(&paths, MAX_BYTES).unwrap();
        native.replace(vec![(
            clipboard_win::formats::CF_UNICODETEXT,
            fixture::text("retained native copy"),
        )]);
        let before = snapshot(&runtime, &owner);
        let source = file_source();
        let formats = if mixed {
            vec!["text/plain".into(), FILES_FORMAT.into()]
        } else {
            vec![FILES_FORMAT.into()]
        };
        let publication = runtime
            .block_on(owner.publish(before.revision, formats, source.clone()))
            .unwrap();
        if !mixed {
            assert_eq!(publication.preparing_revision(), Some(before.revision));
            assert_eq!(
                snapshot(&runtime, &owner).origin,
                None,
                "file preparation preserves real native origin"
            );
            fixture::assert_text(
                &native.read(clipboard_win::formats::CF_UNICODETEXT).unwrap(),
                "retained native copy",
            );
        }
        assert!(
            runtime
                .block_on(owner.attach_files(publication.generation, files.clone()))
                .is_some()
        );
        assert!(
            !publication.cancel_preparation(),
            "a committed file copy cannot be canceled as preparation"
        );
        assert!(publication.is_current());
        assert!(
            runtime
                .block_on(owner.attach_files(publication.generation, files.clone()))
                .is_none(),
            "a file copy is not replayed"
        );
        let bytes = native.read(clipboard_win::formats::CF_HDROP).unwrap();
        assert_eq!(&bytes[..expected.len()], &expected);
        let effect = clipboard_win::register_format("Preferred DropEffect")
            .unwrap()
            .get();
        assert_eq!(&native.read(effect).unwrap()[..4], &1u32.to_le_bytes());
        if mixed {
            fixture::assert_text(
                &native.read(clipboard_win::formats::CF_UNICODETEXT).unwrap(),
                "mixed remote text",
            );
        }
        let after = snapshot(&runtime, &owner);
        assert_eq!(after.origin, Some(publication.generation));
        assert!(after.formats.iter().any(|format| format == FILES_FORMAT));
        drop(files);
        assert!(pinned(&folder), "the native copy pins its file lease");
        native.replace(vec![(
            clipboard_win::formats::CF_UNICODETEXT,
            fixture::text("new local copy"),
        )]);
        assert!(snapshot(&runtime, &owner).origin.is_none());
        drop(publication);
        runtime
            .block_on(owner.shutdown())
            .expect("file helper must be reaped");
        assert!(!pinned(&folder), "a stopped owner releases its file leases");
    }
}
#[test]
#[ignore = "requires an owned noninteractive station and isolated file cache"]
fn canceled_file_preparation_never_empties_the_native_clipboard() {
    fixture::isolated_station();
    let native = fixture::Native::new();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let owner = Owner::with_worker(WorkerExecutable::fixture());
    let cache = FileFixture::new();
    native.replace(vec![(
        clipboard_win::formats::CF_UNICODETEXT,
        fixture::text("keep this native copy"),
    )]);
    let before = snapshot(&runtime, &owner);
    let source = file_source();
    let publication = runtime
        .block_on(owner.publish(before.revision, vec![FILES_FORMAT.into()], source.clone()))
        .unwrap();
    let release = native.hold();
    let attaching = runtime.spawn(owner.attach_files(publication.generation, cache.files()));
    std::thread::sleep(Duration::from_millis(50));
    attaching.abort();
    assert!(publication.cancel_preparation());
    release.store(true, Ordering::Release);
    assert!(runtime.block_on(attaching).unwrap_err().is_cancelled());
    assert!(!source.is_current());
    fixture::assert_text(
        &native.read(clipboard_win::formats::CF_UNICODETEXT).unwrap(),
        "keep this native copy",
    );
    let after = snapshot(&runtime, &owner);
    assert_eq!(after.origin, None);
    assert_eq!(after.revision, before.revision);
    runtime
        .block_on(owner.shutdown())
        .expect("canceled file helper must be reaped");
}
#[test]
#[ignore = "partial file helper entry requires the isolated station and inherited pipe"]
fn clipboard_worker_partial_files_entry() {
    fixture::isolated_station();
    native::fail_file_data_for_fixture();
    process::run_from_stdio(open_files).expect("partial clipboard worker protocol");
}
#[test]
#[ignore = "requires an owned noninteractive station and isolated file cache"]
fn partial_file_commit_keeps_imported_provenance_and_retained_files() {
    fixture::isolated_station();
    let native = fixture::Native::new();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let mut executable = WorkerExecutable::fixture();
    executable.arguments[1] =
        "provider::windows::tests::clipboard_worker_partial_files_entry".into();
    let owner = Owner::with_worker(executable);
    let cache = FileFixture::new();
    let files = cache.files();
    let folder = files.folder().to_owned();
    native.replace(vec![(
        clipboard_win::formats::CF_UNICODETEXT,
        fixture::text("before partial native commit"),
    )]);
    let before = snapshot(&runtime, &owner);
    let source = file_source();
    let publication = runtime
        .block_on(owner.publish(before.revision, vec![FILES_FORMAT.into()], source.clone()))
        .unwrap();
    assert!(
        runtime
            .block_on(owner.attach_files(publication.generation, files))
            .is_none()
    );
    assert!(
        !publication.cancel_preparation(),
        "partial native mutation cannot revert to cancelable preparation"
    );
    let after = snapshot(&runtime, &owner);
    assert_eq!(after.origin, Some(publication.generation));
    assert!(!after.formats.iter().any(|format| format == FILES_FORMAT));
    assert!(source.is_current());
    assert!(publication.is_current());
    assert!(
        pinned(&folder),
        "partial receipt must retain the prepared files"
    );
    native.replace(vec![(
        clipboard_win::formats::CF_UNICODETEXT,
        fixture::text("after partial commit"),
    )]);
    assert!(snapshot(&runtime, &owner).origin.is_none());
    drop(publication);
    runtime
        .block_on(owner.shutdown())
        .expect("partial file helper must be reaped");
}
