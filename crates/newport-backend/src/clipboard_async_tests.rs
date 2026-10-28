use super::*;
use crate::desktop_integration::{
    ClipboardAccess, ClipboardData, ClipboardFuture, Desktop, DesktopIntegration,
};
use crate::integration_permissions::Gate;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicIsize, AtomicU64, AtomicUsize, Ordering},
};

struct NativeOnly(Arc<dyn ClipboardAccess>);
impl DesktopIntegration for NativeOnly {
    fn clipboard_access(&self) -> Option<Arc<dyn ClipboardAccess>> {
        Some(self.0.clone())
    }
    fn on_main_thread(&self, _: Box<dyn FnOnce() + Send>) -> Result<(), String> {
        panic!("native clipboard dispatched to UI")
    }
    fn clipboard_revision(&self) -> Option<isize> {
        panic!("native clipboard used legacy revision")
    }
    fn clipboard_offer(&self, _: Option<isize>) -> Option<(isize, Vec<String>)> {
        panic!("native clipboard used legacy metadata")
    }
    fn read_clipboard_format(&self, _: isize, _: &str) -> Option<Vec<u8>> {
        panic!("native clipboard used legacy payload read")
    }
    fn open_url(&self, _: &str) -> Result<(), String> {
        panic!("unexpected URL")
    }
}
struct State {
    revision: AtomicIsize,
    origin: AtomicU64,
    connected: AtomicBool,
    offers: AtomicUsize,
    metadata_completed: AtomicUsize,
    reads: AtomicUsize,
    cancelled: AtomicUsize,
    released: AtomicUsize,
    metadata: tokio::sync::Semaphore,
    payload: tokio::sync::Semaphore,
    bytes: Vec<u8>,
}
struct Mock(Arc<State>);
struct Data(Arc<State>, isize);
impl ClipboardData for Data {
    fn bytes(&self) -> Option<&[u8]> {
        (self.0.connected.load(Ordering::Acquire)
            && self.0.revision.load(Ordering::Acquire) == self.1)
            .then_some(&self.0.bytes)
    }
}
impl Drop for Data {
    fn drop(&mut self) {
        self.0.released.fetch_add(1, Ordering::AcqRel);
    }
}
struct Pending {
    state: Arc<State>,
    completed: bool,
}
impl Drop for Pending {
    fn drop(&mut self) {
        if !self.completed {
            self.state.cancelled.fetch_add(1, Ordering::AcqRel);
        }
    }
}
impl ClipboardAccess for Mock {
    fn offer(&self, previous: Option<isize>) -> ClipboardFuture<ClipboardOffer> {
        let state = self.0.clone();
        Box::pin(async move {
            state.offers.fetch_add(1, Ordering::AcqRel);
            let mut pending = Pending {
                state: state.clone(),
                completed: false,
            };
            state.metadata.acquire().await.ok()?.forget();
            pending.completed = true;
            state.metadata_completed.fetch_add(1, Ordering::AcqRel);
            let revision = state.revision.load(Ordering::Acquire);
            (state.connected.load(Ordering::Acquire) && previous != Some(revision)).then(|| {
                ClipboardOffer {
                    revision,
                    formats: vec![
                        "text/plain".into(),
                        "TARGETS".into(),
                        "invalid\nformat".into(),
                    ],
                    origin: match state.origin.load(Ordering::Acquire) {
                        0 => ClipboardOrigin::Local,
                        publication => ClipboardOrigin::Imported { publication },
                    },
                }
            })
        })
    }
    fn read(&self, revision: isize, format: String) -> ClipboardFuture<Box<dyn ClipboardData>> {
        let state = self.0.clone();
        Box::pin(async move {
            if revision != state.revision.load(Ordering::Acquire)
                || !matches!(
                    format.as_str(),
                    "text/plain" | newport_platform::clipboard::LOCAL_PATHS_FORMAT
                )
                || state.origin.load(Ordering::Acquire) != 0
                || !state.connected.load(Ordering::Acquire)
            {
                return None;
            }
            state.reads.fetch_add(1, Ordering::AcqRel);
            let mut pending = Pending {
                state: state.clone(),
                completed: false,
            };
            state.payload.acquire().await.ok()?.forget();
            pending.completed = true;
            Some(Box::new(Data(state, revision)) as Box<dyn ClipboardData>)
        })
    }
}
fn setup(payloads: usize) -> (Arc<State>, Desktop) {
    setup_with(payloads, vec![b'a'; clipboard_wire::CHUNK * 2 + 3])
}
fn setup_with(payloads: usize, bytes: Vec<u8>) -> (Arc<State>, Desktop) {
    let state = Arc::new(State {
        revision: AtomicIsize::new(42),
        origin: AtomicU64::new(0),
        connected: AtomicBool::new(true),
        offers: AtomicUsize::new(0),
        metadata_completed: AtomicUsize::new(0),
        reads: AtomicUsize::new(0),
        cancelled: AtomicUsize::new(0),
        released: AtomicUsize::new(0),
        metadata: tokio::sync::Semaphore::new(100),
        payload: tokio::sync::Semaphore::new(payloads),
        bytes,
    });
    let desktop = Arc::new(NativeOnly(Arc::new(Mock(state.clone()))));
    (state, desktop)
}
fn ack(gate: &Gate) {
    assert!(gate.acknowledge(crate::agent::control::Applied {
        permissions: gate.snapshot().desired,
        error: None
    }));
}
fn gate() -> Arc<Gate> {
    let gate = Arc::new(Gate::new(true, false));
    ack(&gate);
    gate
}
fn request(revision: i64) -> clipboard_wire::Request {
    clipboard_wire::Request {
        id: 1,
        revision,
        format: "text/plain".into(),
    }
}
async fn started(counter: &AtomicUsize) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while counter.load(Ordering::Acquire) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn blocked_metadata_is_bounded_and_does_not_block_clipboard_demand() {
    let (state, desktop) = setup(1);
    state.metadata.acquire_many(100).await.unwrap().forget();
    let gate = gate();
    let mut metadata = metadata::Metadata::default();
    for _ in 0..20 {
        metadata.start(&desktop, &gate);
    }
    started(&state.offers).await;
    assert_eq!(state.offers.load(Ordering::Acquire), 1);
    // Run the same competing branches as integration traffic. A pending native
    // metadata request must let a demanded representation finish independently.
    tokio::select! {
        _ = metadata.next(&gate) => panic!("blocked metadata unexpectedly completed"),
        reply = read_requested_checked(&desktop, request(42), Some(gate.clone())) => {
            assert!(reply.chunks.is_some());
        }
    }
    metadata.cancel();
    assert!(metadata.next(&gate).await.unwrap().is_none());
    assert!(!metadata.pending());
    assert_eq!(state.cancelled.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn completed_metadata_cannot_cross_a_permission_epoch() {
    let (state, desktop) = setup(1);
    let gate = gate();
    let mut metadata = metadata::Metadata::default();
    metadata.start(&desktop, &gate);
    // A queued task must not adopt a newer grant before its first poll.
    gate.update(false, false);
    gate.update(true, false);
    ack(&gate);
    assert!(metadata.next(&gate).await.unwrap().is_none());
    assert_eq!(state.offers.load(Ordering::Acquire), 0);
    metadata.start(&desktop, &gate);
    started(&state.metadata_completed).await;
    gate.update(false, false);
    gate.update(true, false);
    ack(&gate);
    assert!(metadata.next(&gate).await.unwrap().is_none());
    metadata.start(&desktop, &gate);
    let (ticket, offer) = metadata.next(&gate).await.unwrap().unwrap();
    assert_eq!(offer.revision, 42);
    metadata.observed(ticket, offer.revision);
    metadata.start(&desktop, &gate);
    assert!(metadata.next(&gate).await.unwrap().is_none());
    // The same native copy must be offered again after a coalesced off/on.
    gate.update(false, false);
    gate.update(true, false);
    ack(&gate);
    metadata.start(&desktop, &gate);
    let (fresh, offer) = metadata.next(&gate).await.unwrap().unwrap();
    assert_ne!(fresh, ticket);
    assert_eq!(offer.revision, 42);
    gate.update(false, false);
    metadata.start(&desktop, &gate);
    assert!(!metadata.pending());
    assert_eq!(state.offers.load(Ordering::Acquire), 4);
}

#[tokio::test]
async fn dropping_metadata_owner_cancels_a_native_wait() {
    let (state, desktop) = setup(1);
    state.metadata.acquire_many(100).await.unwrap().forget();
    let mut metadata = metadata::Metadata::default();
    metadata.start(&desktop, &gate());
    started(&state.offers).await;
    drop(metadata);
    started(&state.cancelled).await;
}

#[tokio::test]
async fn imported_observations_never_become_host_copies_but_local_copies_and_clears_do() {
    let (state, desktop) = setup(1);
    let gate = gate();
    let mut metadata = metadata::Metadata::default();
    metadata.start(&desktop, &gate);
    let (ticket, local) = metadata.next(&gate).await.unwrap().unwrap();
    assert_eq!(host_offer(&local).unwrap().formats, ["text/plain"]);
    metadata.observed(ticket, local.revision);
    state.origin.store(7, Ordering::Release);
    state.revision.store(43, Ordering::Release);
    metadata.start(&desktop, &gate);
    let (ticket, imported) = metadata.next(&gate).await.unwrap().unwrap();
    assert_eq!(
        imported.origin,
        ClipboardOrigin::Imported { publication: 7 }
    );
    assert!(host_offer(&imported).is_none());
    assert!(
        read_requested_checked(&desktop, request(43), Some(gate.clone()))
            .await
            .chunks
            .is_none()
    );
    metadata.observed(ticket, imported.revision);
    metadata.start(&desktop, &gate);
    assert!(metadata.next(&gate).await.unwrap().is_none());
    // Reconnection/re-grant must preserve imported classification too.
    gate.update(false, false);
    gate.update(true, false);
    ack(&gate);
    metadata.start(&desktop, &gate);
    let (_, imported) = metadata.next(&gate).await.unwrap().unwrap();
    assert!(host_offer(&imported).is_none());
    // A user copying identical data creates a real local copy with a new
    // revision. Suppression depends on origin, never a content comparison.
    state.origin.store(0, Ordering::Release);
    state.revision.store(44, Ordering::Release);
    metadata.start(&desktop, &gate);
    let (_, copied) = metadata.next(&gate).await.unwrap().unwrap();
    assert_eq!(host_offer(&copied).unwrap().revision, 44);
    let cleared = host_offer(&ClipboardOffer::local(45, vec![])).unwrap();
    assert!(
        clipboard_wire::Offer::decode(&cleared.encode())
            .unwrap()
            .formats
            .is_empty()
    );
}

#[test]
fn forwarded_offers_never_carry_the_local_paths_format() {
    use newport_platform::clipboard::{FILES_FORMAT, LOCAL_PATHS_FORMAT};
    let formats = || {
        vec![
            "text/plain".to_owned(),
            FILES_FORMAT.to_owned(),
            LOCAL_PATHS_FORMAT.to_owned(),
        ]
    };
    assert_eq!(wire_offer((7, formats())).formats, ["text/plain"]);
    assert_eq!(
        wire_offer_for((7, formats()), true).formats,
        ["text/plain", FILES_FORMAT]
    );
    let local = ClipboardOffer::local(7, formats());
    assert_eq!(host_offer(&local).unwrap().formats, ["text/plain"]);
}

#[test]
fn the_password_hint_travels_in_forwarded_offers() {
    use newport_platform::clipboard::CONCEALED_FORMAT;
    let offer = (
        7,
        vec!["text/plain".to_owned(), CONCEALED_FORMAT.to_owned()],
    );
    let wire = wire_offer(offer);
    assert_eq!(wire.formats, ["text/plain", CONCEALED_FORMAT]);
    // The broker's offer encoding round-trips it.
    assert_eq!(
        clipboard_wire::Offer::decode(&wire.encode())
            .unwrap()
            .formats,
        wire.formats
    );
}

#[tokio::test]
async fn remote_reads_of_the_local_paths_format_are_refused() {
    let (state, desktop) = setup(100);
    let gate = gate();
    let mut local = request(42);
    local.format = newport_platform::clipboard::LOCAL_PATHS_FORMAT.into();
    let reply = read_requested_checked(&desktop, local, Some(gate.clone())).await;
    assert!(reply.chunks.is_none());
    assert!(reply.data.is_none());
    assert_eq!(state.reads.load(Ordering::Acquire), 0);
    assert!(
        read_requested_checked(&desktop, request(42), Some(gate))
            .await
            .chunks
            .is_some()
    );
}

#[cfg(unix)]
fn files_request(revision: i64) -> clipboard_wire::Request {
    clipboard_wire::Request {
        id: 9,
        revision,
        format: newport_platform::clipboard::FILES_FORMAT.into(),
    }
}

#[cfg(unix)]
#[tokio::test]
async fn file_reads_upload_the_local_paths_of_the_requested_revision() {
    use crate::clipboard_files::{
        Files, Mode,
        tests::{Copied, Disk, Local},
    };
    let copied = Copied::new();
    let (state, desktop) = setup_with(100, copied.paths());
    let disk = Disk::new(false);
    let files = Files::new(Local(disk.clone()), Mode::LongRead);
    let gate = gate();
    let stale = read_files(
        desktop.clone(),
        files.clone(),
        files_request(41),
        Some(gate.clone()),
    )
    .await;
    assert!(stale.chunks.is_none());
    let mut reply = read_files(
        desktop.clone(),
        files.clone(),
        files_request(42),
        Some(gate.clone()),
    )
    .await;
    assert!(reply.data.is_none());
    assert_eq!(state.reads.load(Ordering::Acquire), 1);
    let mut bytes = Vec::new();
    loop {
        let (status, done, chunk) = next_reply_chunk(&desktop, &gate, &mut reply).await;
        bytes.extend(clipboard_wire::decompress_chunk(status, &chunk).unwrap());
        if done {
            break;
        }
    }
    let reply: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let names = disk.names();
    let folder = disk.root().join(&names[0]);
    assert_eq!(
        reply,
        serde_json::json!({"version": 1, "paths": [folder.join("docs"), folder.join("名前 1.txt")]})
    );
    // A newer copy fences the cached reply too.
    state.revision.store(43, Ordering::Release);
    let mut late = read_files(
        desktop.clone(),
        files,
        files_request(42),
        Some(gate.clone()),
    )
    .await;
    assert_eq!(
        next_reply_chunk(&desktop, &gate, &mut late).await,
        (1, true, Vec::new())
    );
    assert_eq!(disk.uploads.load(Ordering::Acquire), 1);
}

#[cfg(unix)]
#[tokio::test]
async fn revoking_clipboard_permission_ends_a_file_read_and_removes_its_partial() {
    use crate::clipboard_files::{
        Files, Mode,
        tests::{Copied, Disk, Local},
    };
    let copied = Copied::new();
    let (_state, desktop) = setup_with(100, copied.paths());
    let disk = Disk::new(true);
    let files = Files::new(Local(disk.clone()), Mode::LongRead);
    let gate = gate();
    let read = tokio::spawn(read_files(
        desktop,
        files.clone(),
        files_request(42),
        Some(gate.clone()),
    ));
    disk.until(|disk| disk.waiting.load(Ordering::Acquire) > 0)
        .await;
    assert_eq!(disk.partials(), 1);
    gate.update(false, false);
    let reply = read.await.unwrap();
    assert!(reply.chunks.is_none());
    // The route revokes the files when it sees the permission change.
    files.revoke();
    disk.until(|disk| disk.names().is_empty()).await;
}

#[tokio::test]
async fn disabled_access_does_not_poll_native_work_or_fall_back_to_ui() {
    let (state, desktop) = setup(100);
    let gate = Arc::new(Gate::new(false, false));
    assert!(
        clipboard_offer_checked(&desktop, None, Some(gate.clone()))
            .await
            .unwrap()
            .is_none()
    );
    assert!(revision(&desktop, Some(gate.clone())).await.is_none());
    assert!(
        read_requested_checked(&desktop, request(42), Some(gate.clone()))
            .await
            .chunks
            .is_none()
    );
    assert_eq!(state.offers.load(Ordering::Acquire), 0);
    assert_eq!(state.reads.load(Ordering::Acquire), 0);
    gate.update(true, false);
    ack(&gate);
    let offer = clipboard_offer_checked(&desktop, None, Some(gate.clone()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(offer.formats, ["text/plain"]);
    assert_eq!(offer.revision, 42);
    assert!(
        read_requested_checked(&desktop, request(41), Some(gate.clone()))
            .await
            .chunks
            .is_none()
    );
    assert_eq!(state.reads.load(Ordering::Acquire), 0);
    assert!(
        read_requested_checked(&desktop, request(42), Some(gate))
            .await
            .chunks
            .is_some()
    );
}

#[tokio::test]
async fn revocation_cancels_a_native_read_without_waiting_for_its_provider() {
    let (state, desktop) = setup(0);
    let gate = gate();
    let app = desktop.clone();
    let access = gate.clone();
    let read =
        tokio::spawn(async move { read_requested_checked(&app, request(42), Some(access)).await });
    started(&state.reads).await;
    gate.update(false, false);
    gate.update(true, false);
    ack(&gate);
    let reply = tokio::time::timeout(Duration::from_secs(1), read)
        .await
        .unwrap()
        .unwrap();
    assert!(reply.chunks.is_none());
    assert_eq!(state.cancelled.load(Ordering::Acquire), 1);
    state.payload.add_permits(1);
    assert!(
        read_requested_checked(&desktop, request(42), Some(gate))
            .await
            .chunks
            .is_some()
    );
}

#[tokio::test]
async fn prepared_chunks_retain_native_fences_and_permission_epochs_until_delivery() {
    let (state, desktop) = setup(100);
    let gate = gate();
    let mut reply = read_requested_checked(&desktop, request(42), Some(gate.clone())).await;
    assert!(reply.data_current());
    assert_eq!(state.released.load(Ordering::Acquire), 0);
    state.metadata.forget_permits(100);
    let (status, done, bytes) = tokio::time::timeout(
        Duration::from_secs(1),
        next_reply_chunk(&desktop, &gate, &mut reply),
    )
    .await
    .unwrap();
    assert!(!done);
    assert_eq!(
        clipboard_wire::decompress_chunk(status, &bytes).unwrap(),
        vec![b'a'; clipboard_wire::CHUNK]
    );
    state.connected.store(false, Ordering::Release);
    assert_eq!(
        next_reply_chunk(&desktop, &gate, &mut reply).await,
        (1, true, Vec::new())
    );
    assert_eq!(state.released.load(Ordering::Acquire), 1);
    state.connected.store(true, Ordering::Release);
    let mut reply = read_requested_checked(&desktop, request(42), Some(gate.clone())).await;
    gate.update(false, false);
    gate.update(true, false);
    ack(&gate);
    assert_eq!(
        next_reply_chunk(&desktop, &gate, &mut reply).await,
        (1, true, Vec::new())
    );
    assert_eq!(state.released.load(Ordering::Acquire), 2);
}

#[tokio::test(start_paused = true)]
async fn native_timeout_and_task_abort_drop_pending_work() {
    let (state, desktop) = setup(0);
    let app = desktop.clone();
    let read = tokio::spawn(async move { read_requested(&app, request(42)).await });
    tokio::task::yield_now().await;
    assert_eq!(state.reads.load(Ordering::Acquire), 1);
    tokio::time::advance(Duration::from_secs(21)).await;
    assert!(read.await.unwrap().chunks.is_none());
    assert_eq!(state.cancelled.load(Ordering::Acquire), 1);
    let read = tokio::spawn(async move { read_requested(&desktop, request(42)).await });
    tokio::task::yield_now().await;
    read.abort();
    assert!(matches!(read.await, Err(error) if error.is_cancelled()));
    assert_eq!(state.cancelled.load(Ordering::Acquire), 2);
}

#[tokio::test]
async fn metadata_revocation_cancels_its_wait_and_prevents_ui_fallback() {
    let (state, desktop) = setup(0);
    state.metadata.forget_permits(100);
    let gate = gate();
    let app = desktop.clone();
    let access = gate.clone();
    let offer =
        tokio::spawn(async move { clipboard_offer_checked(&app, None, Some(access)).await });
    started(&state.offers).await;
    gate.update(false, false);
    assert!(
        tokio::time::timeout(Duration::from_secs(1), offer)
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .is_none()
    );
    assert_eq!(state.cancelled.load(Ordering::Acquire), 1);
}

#[cfg(target_os = "macos")]
#[tokio::test]
#[ignore = "Build clipboard_owner_check and set NEWPORT_CLIPBOARD_HELPER to that local example executable"]
async fn real_native_adapter_keeps_fences_through_backend_chunks_and_suppresses_import_echo() {
    use newport_platform::clipboard::{
        helper::{Board, owner::Owner},
        provider::{Reply, Source},
    };
    struct Fixture {
        executable: std::path::PathBuf,
        name: String,
        owner: Owner,
    }
    impl Fixture {
        async fn consumer(&self, operation: &str) -> serde_json::Value {
            let output = tokio::time::timeout(
                Duration::from_secs(4),
                tokio::process::Command::new(&self.executable)
                    .args(["consumer", &self.name, operation])
                    .kill_on_drop(true)
                    .output(),
            )
            .await
            .unwrap()
            .unwrap();
            assert!(output.status.success(), "native consumer failed");
            serde_json::from_slice(&output.stdout).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.owner.close();
            let _ = std::process::Command::new(&self.executable)
                .args(["consumer", &self.name, "release"])
                .output();
        }
    }
    async fn offer(
        desktop: &Desktop,
        gate: &Arc<Gate>,
        previous: Option<isize>,
        expected: usize,
    ) -> clipboard_wire::Offer {
        let mut last = None;
        let result = tokio::time::timeout(Duration::from_secs(4), async {
            loop {
                if let Some(offer) = clipboard_offer_checked(desktop, previous, Some(gate.clone()))
                    .await
                    .unwrap()
                {
                    let plain = offer.formats.iter().any(|f| f == "text/plain");
                    let png = offer.formats.iter().any(|f| f == "image/png");
                    let matches = match expected {
                        0 => offer.formats.is_empty(),
                        1 => plain && !png,
                        2 => plain && png,
                        _ => unreachable!(),
                    };
                    if matches {
                        break offer;
                    }
                    last = Some(offer);
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        result.unwrap_or_else(|_| {
            panic!("Expected {expected} formats after {previous:?}; last native offer: {last:?}")
        })
    }
    struct Remote;
    impl Source for Remote {
        fn is_current(&self) -> bool {
            true
        }
        fn request(&self, _: &str, reply: Reply) {
            reply.complete(Box::new(LegacyData(b"remote".to_vec())));
        }
    }
    let executable = std::path::PathBuf::from(
        std::env::var_os("NEWPORT_CLIPBOARD_HELPER")
            .expect("explicit local fixture executable required"),
    )
    .canonicalize()
    .unwrap();
    let name = format!("org.newport.test.lazy.{}", uuid::Uuid::new_v4());
    let fixture = Fixture {
        owner: Owner::new(executable.clone(), Board::Test { name: name.clone() }),
        executable,
        name,
    };
    let native = Arc::new(crate::desktop_integration::NativeClipboard::new(Some(
        fixture.owner.clone(),
    )));
    let desktop: Desktop = Arc::new(NativeOnly(native.clone()));
    let gate = gate();
    fixture.consumer("local-rich").await;
    let initial = offer(&desktop, &gate, None, 2).await;
    let (mut text, mut png) = tokio::join!(
        read_requested_checked(&desktop, request(initial.revision), Some(gate.clone())),
        read_requested_checked(
            &desktop,
            clipboard_wire::Request {
                id: 2,
                revision: initial.revision,
                format: "image/png".into()
            },
            Some(gate.clone())
        )
    );
    let (status, done, bytes) = next_reply_chunk(&desktop, &gate, &mut png).await;
    assert!(done);
    assert!(
        clipboard_wire::decompress_chunk(status, &bytes)
            .unwrap()
            .starts_with(&[137, 80, 78, 71])
    );
    assert!(text.data_current());
    fixture.consumer("local-empty").await;
    let empty = offer(&desktop, &gate, Some(initial.revision as isize), 1).await;
    assert_eq!(
        next_reply_chunk(&desktop, &gate, &mut text).await,
        (1, true, Vec::new())
    );
    let mut reply =
        read_requested_checked(&desktop, request(empty.revision), Some(gate.clone())).await;
    assert_eq!(
        next_reply_chunk(&desktop, &gate, &mut reply).await,
        (0, true, Vec::new())
    );
    let imported = fixture
        .owner
        .publish(
            empty.revision as isize,
            vec!["text/plain".into()],
            Arc::new(Remote),
        )
        .await
        .unwrap();
    let remote = tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            if let Some(remote) = native.offer(Some(empty.revision as isize)).await
                && remote.origin
                    == (ClipboardOrigin::Imported {
                        publication: imported.generation,
                    })
            {
                break remote;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(host_offer(&remote).is_none());
    assert!(remote.formats.iter().any(|format| format == "text/plain"));
    assert!(
        read_requested_checked(
            &desktop,
            request(remote.revision as i64),
            Some(gate.clone())
        )
        .await
        .chunks
        .is_none()
    );
    assert!(
        fixture.consumer("public.utf8-plain-text").await["length"]
            .as_u64()
            .is_some()
    );
    native.close();
    assert!(!imported.is_current());
    tokio::time::timeout(Duration::from_secs(3), native.shutdown())
        .await
        .unwrap();
    assert!(
        clipboard_offer_checked(&desktop, None, Some(gate))
            .await
            .unwrap()
            .is_none()
    );
}

#[cfg(target_os = "macos")]
#[tokio::test]
#[ignore = "Requires local clipboard_owner_check and NEWPORT_CLIPBOARD_LINUX_PEER Docker fixture launcher"]
async fn native_origin_roundtrip_preserves_the_real_linux_selection_and_user_clear() {
    use crate::clipboard_export::{
        Receiver,
        source::{Demands, Grant},
    };
    use base64::{Engine, engine::general_purpose::STANDARD};
    use newport_platform::clipboard::helper::{Board, owner::Owner};
    use serde_json::{Value, json};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    struct Peer {
        child: tokio::process::Child,
        input: tokio::process::ChildStdin,
        output: BufReader<tokio::process::ChildStdout>,
    }
    impl Peer {
        async fn receive(&mut self) -> Value {
            let mut line = String::new();
            let count =
                tokio::time::timeout(Duration::from_secs(18), self.output.read_line(&mut line))
                    .await
                    .unwrap()
                    .unwrap();
            assert!(count > 0, "local Linux clipboard fixture exited");
            assert!(count <= 256 * 1024);
            serde_json::from_str(&line).unwrap()
        }
        async fn call(&mut self, command: Value) -> Value {
            self.input
                .write_all(format!("{command}\n").as_bytes())
                .await
                .unwrap();
            self.input.flush().await.unwrap();
            self.receive().await
        }
    }
    struct Native {
        executable: std::path::PathBuf,
        name: String,
        adapter: Arc<crate::desktop_integration::NativeClipboard>,
    }
    impl Native {
        fn consumer(&self, operation: &str) -> tokio::task::JoinHandle<Value> {
            let executable = self.executable.clone();
            let name = self.name.clone();
            let operation = operation.to_owned();
            tokio::spawn(async move {
                let output = tokio::time::timeout(
                    Duration::from_secs(10),
                    tokio::process::Command::new(executable)
                        .args(["consumer", &name, &operation])
                        .kill_on_drop(true)
                        .output(),
                )
                .await
                .unwrap()
                .unwrap();
                assert!(
                    output.status.success(),
                    "native consumer failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
                serde_json::from_slice(&output.stdout).unwrap()
            })
        }
        async fn observe(&self, predicate: impl Fn(&ClipboardOffer) -> bool) -> ClipboardOffer {
            tokio::time::timeout(Duration::from_secs(4), async {
                loop {
                    if let Some(offer) = self.adapter.offer(None).await
                        && predicate(&offer)
                    {
                        return offer;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap()
        }
    }
    impl Drop for Native {
        fn drop(&mut self) {
            self.adapter.close();
            let _ = std::process::Command::new(&self.executable)
                .args(["consumer", &self.name, "release"])
                .output();
        }
    }

    let executable =
        std::path::PathBuf::from(std::env::var_os("NEWPORT_CLIPBOARD_HELPER").unwrap())
            .canonicalize()
            .unwrap();
    let name = format!("org.newport.test.lazy.{}", uuid::Uuid::new_v4());
    let native = Native {
        adapter: Arc::new(crate::desktop_integration::NativeClipboard::new(Some(
            Owner::new(executable.clone(), Board::Test { name: name.clone() }),
        ))),
        executable,
        name,
    };
    let desktop: Desktop = Arc::new(NativeOnly(native.adapter.clone()));
    let permissions = gate();
    let mut metadata = metadata::Metadata::default();
    let mut child =
        tokio::process::Command::new(std::env::var_os("NEWPORT_CLIPBOARD_LINUX_PEER").unwrap())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
    let mut peer = Peer {
        input: child.stdin.take().unwrap(),
        output: BufReader::new(child.stdout.take().unwrap()),
        child,
    };
    let ready = peer.receive().await;
    let mut receiver = Receiver::default();
    receiver.enable(ready["runtime"].as_str().unwrap()).unwrap();
    let grant = Grant::new(
        uuid::Uuid::parse_str(ready["runtime"].as_str().unwrap()).unwrap(),
        uuid::Uuid::parse_str(ready["worker"].as_str().unwrap()).unwrap(),
        1,
    )
    .unwrap();
    let mut demands = Demands::default();
    let png = [
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 4,
        0, 0, 0, 181, 28, 12, 2, 0, 0, 0, 11, 73, 68, 65, 84, 120, 218, 99, 100, 248, 15, 0, 1, 5,
        1, 1, 39, 24, 227, 102, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];
    for (format, operation, bytes, field) in [
        (
            "text/plain",
            "public.utf8-plain-text",
            "Newport 世界\n".as_bytes(),
            "text",
        ),
        ("image/png", "public.png", png.as_slice(), "png"),
    ] {
        let copied = peer
            .call(json!({"op":"copy", "format":format, "bytes":STANDARD.encode(bytes)}))
            .await;
        receiver
            .accept(&STANDARD.decode(copied["event"].as_str().unwrap()).unwrap())
            .unwrap();
        let source = demands.source(receiver.lease().unwrap(), &grant).unwrap();
        let current = native.adapter.offer(None).await.unwrap();
        let publication = native
            .adapter
            .publish(
                current.revision,
                receiver.offer().unwrap().offer.formats.clone(),
                source,
            )
            .await
            .unwrap();
        let origin = ClipboardOrigin::Imported {
            publication: publication.generation(),
        };
        native.observe(|offer| offer.origin == origin).await;
        metadata.start(&desktop, &permissions);
        let (ticket, observed) = metadata.next(&permissions).await.unwrap().unwrap();
        assert_eq!(observed.origin, origin);
        assert!(
            host_offer(&observed).is_none(),
            "native import became a host update"
        );
        metadata.observed(ticket, observed.revision);
        let before = peer.call(json!({"op":"status"})).await;
        assert_eq!(before["broker"], copied["state"]["broker"]);
        assert_eq!(
            before["nativeRequests"], copied["state"]["nativeRequests"],
            "metadata eagerly fetched payload"
        );
        let consumer = native.consumer(operation);
        let demand = tokio::time::timeout(Duration::from_secs(3), demands.receive())
            .await
            .unwrap()
            .unwrap();
        let (read, command) = receiver.read(demand.offer(), demand.format()).unwrap();
        let transfer = tokio::spawn(demand.complete(read));
        let reply = peer.call(json!({"op":"read", "command":command})).await;
        for event in reply["events"].as_array().unwrap() {
            receiver
                .accept(&STANDARD.decode(event.as_str().unwrap()).unwrap())
                .unwrap();
        }
        transfer.await.unwrap();
        assert_eq!(consumer.await.unwrap()[field], true);
        let verified = peer.call(json!({"op":"verify", "format":format})).await;
        for value in verified["values"].as_array().unwrap() {
            assert_eq!(STANDARD.decode(value.as_str().unwrap()).unwrap(), bytes);
        }
        assert_eq!(verified["state"]["broker"], copied["state"]["broker"]);
        assert_eq!(
            verified["state"]["nativeRequests"].as_u64().unwrap(),
            before["nativeRequests"].as_u64().unwrap() + 1
        );
        // Removing the native promise is our own write too. It must not be
        // reflected to Linux as a user clear, erasing the original selection.
        drop(publication);
        native
            .observe(|offer| offer.origin == origin && offer.formats.is_empty())
            .await;
        metadata.start(&desktop, &permissions);
        let (ticket, withdrawn) = metadata.next(&permissions).await.unwrap().unwrap();
        assert_eq!(withdrawn.origin, origin);
        assert!(host_offer(&withdrawn).is_none());
        metadata.observed(ticket, withdrawn.revision);
        assert_eq!(
            peer.call(json!({"op":"status"})).await["broker"],
            copied["state"]["broker"]
        );
    }
    native.consumer("local-clear").await.unwrap();
    native
        .observe(|offer| offer.origin == ClipboardOrigin::Local && offer.formats.is_empty())
        .await;
    metadata.start(&desktop, &permissions);
    let (_, cleared) = metadata.next(&permissions).await.unwrap().unwrap();
    let host = host_offer(&cleared).expect("real local clear must reach the remote clipboard");
    let remote = peer
        .call(json!({"op":"host", "revision":host.revision, "formats":host.formats}))
        .await;
    assert_eq!(remote["broker"]["origin"]["kind"], "host");
    assert!(remote["broker"]["formats"].as_array().unwrap().is_empty());
    assert_eq!(peer.call(json!({"op":"quit"})).await["stopped"], true);
    assert!(
        tokio::time::timeout(Duration::from_secs(4), peer.child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    tokio::time::timeout(Duration::from_secs(3), native.adapter.shutdown())
        .await
        .unwrap();
}

/// Records the notices a paste of files raises.
#[cfg(unix)]
struct Noticing(
    Desktop,
    std::sync::Mutex<Vec<crate::desktop_integration::ClipboardFilesStatus>>,
);
#[cfg(unix)]
impl DesktopIntegration for Noticing {
    fn clipboard_access(&self) -> Option<Arc<dyn ClipboardAccess>> {
        self.0.clipboard_access()
    }
    fn on_main_thread(&self, _: Box<dyn FnOnce() + Send>) -> Result<(), String> {
        panic!("native clipboard dispatched to UI")
    }
    fn clipboard_revision(&self) -> Option<isize> {
        panic!("native clipboard used legacy revision")
    }
    fn clipboard_offer(&self, _: Option<isize>) -> Option<(isize, Vec<String>)> {
        panic!("native clipboard used legacy metadata")
    }
    fn read_clipboard_format(&self, _: isize, _: &str) -> Option<Vec<u8>> {
        panic!("native clipboard used legacy payload read")
    }
    fn open_url(&self, _: &str) -> Result<(), String> {
        panic!("unexpected URL")
    }
    fn clipboard_files_status(
        &self,
        server: &str,
        status: crate::desktop_integration::ClipboardFilesStatus,
    ) {
        assert_eq!(server, "Build server");
        self.1.lock().unwrap().push(status);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn only_a_failed_paste_of_files_raises_a_notice() {
    use crate::clipboard_files::{
        Files, Mode,
        tests::{Copied, Disk, Local},
    };
    use crate::desktop_integration::ClipboardFilesStatus::UploadFailed;
    let copied = Copied::new();
    let (state, native) = setup_with(100, copied.paths());
    let noticing = Arc::new(Noticing(native, Default::default()));
    let desktop: Desktop = noticing.clone();
    let notices = || std::mem::take(&mut *noticing.1.lock().unwrap());
    let gate = gate();

    // Served: no notice, and neither for a superseded copy.
    let disk = Disk::new(false);
    let files = Files::new(Local(disk.clone()), Mode::LongRead).named("Build server");
    let served = read_files(
        desktop.clone(),
        files.clone(),
        files_request(42),
        Some(gate.clone()),
    )
    .await;
    assert!(served.chunks.is_some());
    let stale = read_files(
        desktop.clone(),
        files,
        files_request(41),
        Some(gate.clone()),
    )
    .await;
    assert!(stale.chunks.is_none());
    assert!(notices().is_empty());

    // An SFTP error is reported, with its reason.
    let disk = Disk::new(false);
    disk.fail_writes.store(true, Ordering::Release);
    let files = Files::new(Local(disk.clone()), Mode::LongRead).named("Build server");
    let failed = read_files(
        desktop.clone(),
        files,
        files_request(42),
        Some(gate.clone()),
    )
    .await;
    assert!(failed.chunks.is_none());
    assert_eq!(
        notices(),
        [UploadFailed("No space left on the server".into())]
    );

    // Revoked permission is not.
    let disk = Disk::new(true);
    let files = Files::new(Local(disk.clone()), Mode::LongRead).named("Build server");
    let read = tokio::spawn(read_files(
        desktop.clone(),
        files,
        files_request(42),
        Some(gate.clone()),
    ));
    disk.until(|disk| disk.waiting.load(Ordering::Acquire) > 0)
        .await;
    gate.update(false, false);
    assert!(read.await.unwrap().chunks.is_none());
    assert!(notices().is_empty());
    drop(state);
}
