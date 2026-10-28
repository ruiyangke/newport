use super::*;
use crate::{
    agent_runtime::protocol::{
        clipboard::{Chunk, CopyLease, ExportOffer, Offer, Origin},
        clipboard_export::{self as wire, Event},
    },
    clipboard_export::Receiver,
    desktop_integration::ClipboardSource,
};
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

#[path = "clipboard_router_driver_tests.rs"]
mod driver;

#[path = "clipboard_router_display_tests.rs"]
mod display;

#[path = "clipboard_router_route_tests.rs"]
mod route;

struct Native {
    offer: Arc<Mutex<ClipboardOffer>>,
    /// Folders attached to publications, by generation.
    attached: Arc<Mutex<Vec<(u64, std::path::PathBuf)>>>,
    requests: mpsc::UnboundedSender<Request>,
}
struct Request {
    expected: isize,
    formats: Vec<String>,
    source: Arc<dyn ClipboardSource>,
    reply: oneshot::Sender<Box<dyn ClipboardPublication>>,
}
struct Publication {
    source: Arc<dyn ClipboardSource>,
    generation: u64,
    offer: Arc<Mutex<ClipboardOffer>>,
    attached: Arc<Mutex<Vec<(u64, std::path::PathBuf)>>>,
}
impl ClipboardPublication for Publication {
    fn is_current(&self) -> bool {
        self.source.is_current()
    }
    fn generation(&self) -> u64 {
        self.generation
    }
    /// Replaces the offer with file URLs while the board holds this import.
    fn attach_files(&self, folder: &std::path::Path) -> ClipboardFuture<isize> {
        let mut offer = self.offer.lock().unwrap();
        let attached = (offer.origin
            == ClipboardOrigin::Imported {
                publication: self.generation,
            })
        .then(|| {
            offer.revision += 1;
            offer.formats = vec!["public.file-url".into()];
            self.attached
                .lock()
                .unwrap()
                .push((self.generation, folder.to_owned()));
            offer.revision
        });
        Box::pin(async move { attached })
    }
}
impl Drop for Publication {
    fn drop(&mut self) {
        self.source.publication_revoked();
    }
}
struct Claim(Option<Arc<dyn ClipboardSource>>);
impl Claim {
    fn accepted(mut self) {
        self.0 = None;
    }
}
impl Drop for Claim {
    fn drop(&mut self) {
        if let Some(source) = &self.0 {
            source.publication_revoked();
        }
    }
}
impl Native {
    fn new() -> (Arc<Self>, mpsc::UnboundedReceiver<Request>) {
        let (requests, receiver) = mpsc::unbounded_channel();
        (
            Arc::new(Self {
                offer: Arc::new(Mutex::new(ClipboardOffer::local(
                    1,
                    vec!["text/plain".into()],
                ))),
                attached: Arc::default(),
                requests,
            }),
            receiver,
        )
    }
    // Separate native confirmation from reply delivery so focus can change
    // after the OS accepts ownership but before the router's task resumes.
    fn confirm(&self, request: &Request) -> Box<dyn ClipboardPublication> {
        let mut offer = self.offer.lock().unwrap();
        assert_eq!(request.expected, offer.revision);
        assert!(request.source.publication_confirmed());
        offer.revision += 1;
        let generation = offer.revision as u64;
        offer.formats = request.formats.clone();
        offer.origin = ClipboardOrigin::Imported {
            publication: generation,
        };
        Box::new(Publication {
            source: request.source.clone(),
            generation,
            offer: self.offer.clone(),
            attached: self.attached.clone(),
        })
    }
    fn local(&self) -> ClipboardOffer {
        let mut offer = self.offer.lock().unwrap();
        *offer = ClipboardOffer::local(offer.revision + 1, vec!["text/plain".into()]);
        offer.clone()
    }
    fn withdraw(&self) {
        let mut offer = self.offer.lock().unwrap();
        assert!(matches!(offer.origin, ClipboardOrigin::Imported { .. }));
        offer.revision += 1;
        offer.formats.clear();
    }
}
impl ClipboardAccess for Native {
    fn offer(&self, previous: Option<isize>) -> ClipboardFuture<ClipboardOffer> {
        let offer = self.offer.lock().unwrap().clone();
        Box::pin(async move { (previous != Some(offer.revision)).then_some(offer) })
    }
    fn read(&self, revision: isize, format: String) -> ClipboardFuture<Box<dyn ClipboardData>> {
        struct Local;
        impl ClipboardData for Local {
            fn bytes(&self) -> Option<&[u8]> {
                Some(b"local")
            }
        }
        let offer = self.offer.lock().unwrap().clone();
        Box::pin(async move {
            (offer.revision == revision
                && offer.origin == ClipboardOrigin::Local
                && offer.formats.contains(&format))
            .then(|| Box::new(Local) as Box<dyn ClipboardData>)
        })
    }
    fn publish(
        &self,
        expected: isize,
        formats: Vec<String>,
        source: Arc<dyn ClipboardSource>,
    ) -> ClipboardFuture<Box<dyn ClipboardPublication>> {
        if !source.begin_publication() {
            return Box::pin(async { None });
        }
        let claim = Claim(Some(source.clone()));
        let requests = self.requests.clone();
        Box::pin(async move {
            let (reply, receiver) = oneshot::channel();
            requests
                .send(Request {
                    expected,
                    formats,
                    source,
                    reply,
                })
                .ok()?;
            let publication = receiver.await.ok()?;
            claim.accepted();
            Some(publication)
        })
    }
}

struct Remote {
    receiver: Receiver,
    runtime: Uuid,
    worker: Uuid,
}
impl Remote {
    fn new() -> Self {
        Self::at(Uuid::new_v4(), Uuid::new_v4())
    }
    fn at(runtime: Uuid, worker: Uuid) -> Self {
        let mut receiver = Receiver::default();
        receiver
            .enable_version(&runtime.to_string(), wire::COPY_VERSION)
            .unwrap();
        let mut remote = Self {
            receiver,
            runtime,
            worker,
        };
        remote.offer(1, 1);
        remote
    }
    fn event(&mut self, event: Event) {
        self.receiver.accept(&event.encode().unwrap()).unwrap();
    }
    fn offer(&mut self, revision: i64, route_revision: u64) {
        self.offer_formats(revision, route_revision, &["text/plain"]);
    }
    fn offer_formats(&mut self, revision: i64, route_revision: u64, formats: &[&str]) {
        if let Some(wire::Control::Release { copy }) = self.receiver.copy_release() {
            self.receiver.copy_release_sent(&copy.id);
            self.event(Event::CopyRevoked { id: copy.id });
        }
        self.event(Event::Offer {
            offer: Some(ExportOffer {
                offer: Offer {
                    runtime: self.runtime.to_string(),
                    revision,
                    origin: Origin::Native {
                        worker: self.worker.to_string(),
                        revision: revision.try_into().unwrap(),
                    },
                    formats: formats.iter().map(|f| (*f).to_owned()).collect(),
                },
                route_revision,
            }),
        });
    }
    fn lease(&self) -> Lease {
        self.receiver.lease().unwrap()
    }
    fn focus(&self, endpoint: &Endpoint) {
        assert!(
            endpoint.focus(Some(
                Grant::new(
                    self.runtime,
                    self.worker,
                    self.lease().offer().route_revision
                )
                .unwrap()
            ))
        );
    }
    async fn copy(&mut self) -> CommittedCopy {
        let lease = self.lease();
        let (commit, _) = self.receiver.commit(&lease).unwrap();
        self.event(Event::Committed {
            id: commit.id(),
            copy: Some(CopyLease {
                id: Uuid::new_v4().to_string(),
                offer: lease.offer().clone(),
            }),
        });
        commit.receive().await.unwrap()
    }
}
async fn setup() -> (
    Router,
    Arc<Native>,
    mpsc::UnboundedReceiver<Request>,
    Remote,
    Endpoint,
) {
    let (native, requests) = Native::new();
    let router = Router::new(native.clone());
    router.offer(None).await.unwrap();
    let remote = Remote::new();
    let endpoint = router.bind(&remote.lease()).unwrap();
    remote.focus(&endpoint);
    (router, native, requests, remote, endpoint)
}
async fn start(
    router: &Router,
    remote: &mut Remote,
    endpoint: &Endpoint,
    demands: &Demands,
) -> tokio::task::JoinHandle<bool> {
    let proposal = endpoint.propose(remote.lease()).unwrap();
    let published = router.publish(proposal, remote.copy().await, demands);
    tokio::spawn(async move { published.await.is_some() })
}

#[tokio::test]
async fn imported_copy_can_reach_another_runtime_without_echoing_or_escaping_revocation() {
    for revoke in 0..4 {
        let (router, native, mut requests, mut source, origin) = setup().await;
        let target = Remote::new();
        let destination = router.bind(&target.lease()).unwrap();
        let demands = Demands::default();
        let published = start(&router, &mut source, &origin, &demands).await;
        let request = requests.recv().await.unwrap();
        let publication = native.confirm(&request);
        request.reply.send(publication).ok().unwrap();
        assert!(published.await.unwrap());
        let imported = router.offer(None).await.unwrap();
        assert!(
            !origin.may_forward_import(&imported),
            "never reflect a copy into its own broker"
        );
        assert!(destination.may_forward_import(&imported));
        origin.blur();
        assert!(
            destination.may_forward_import(&imported),
            "copy survives leaving its source window"
        );
        let mut forged = imported.clone();
        forged.revision += 1;
        assert!(!destination.may_forward_import(&forged));
        forged = imported.clone();
        forged.origin = ClipboardOrigin::Imported {
            publication: u64::MAX,
        };
        assert!(!destination.may_forward_import(&forged));
        match revoke {
            0 => origin.revoke(),
            1 => destination.revoke(),
            2 => router.close(),
            _ => {
                native.local();
                router.offer(None).await.unwrap();
            }
        }
        assert!(!destination.may_forward_import(&imported));
    }
}

#[tokio::test]
async fn imported_read_uses_original_copy_after_blur_and_keeps_held_data_fenced() {
    for revoke in 0..4 {
        let (router, native, mut requests, mut remote, endpoint) = setup().await;
        let mut demands = Demands::default();
        let published = start(&router, &mut remote, &endpoint, &demands).await;
        let request = requests.recv().await.unwrap();
        let publication = native.confirm(&request);
        request.reply.send(publication).ok().unwrap();
        assert!(published.await.unwrap());
        let imported = router.offer(None).await.unwrap();
        endpoint.blur();
        remote.event(Event::Offer { offer: None });
        assert!(
            router
                .read(imported.revision - 1, "text/plain".into())
                .await
                .is_none()
        );
        assert!(
            router
                .read(imported.revision, "image/png".into())
                .await
                .is_none()
        );
        let reading = tokio::spawn(router.read(imported.revision, "text/plain".into()));
        let demand = tokio::time::timeout(std::time::Duration::from_secs(1), demands.receive())
            .await
            .unwrap()
            .unwrap();
        let (read, _) = remote
            .receiver
            .read_copy(demand.copy().unwrap(), demand.format())
            .unwrap();
        remote.event(Event::Data {
            id: read.id(),
            offset: 0,
            chunk: Some(Chunk {
                bytes: b"remote copy".to_vec(),
                eof: true,
            }),
        });
        demand.complete(read).await;
        let data = reading.await.unwrap().unwrap();
        assert_eq!(data.bytes(), Some(b"remote copy".as_slice()));
        match revoke {
            0 => endpoint.revoke(),
            1 => router.close(),
            2 => {
                native.local();
                router.offer(None).await.unwrap();
            }
            _ => remote.receiver.disconnect(),
        }
        assert!(
            data.bytes().is_none(),
            "held data escaped source revocation"
        );
        assert!(
            router
                .read(imported.revision, "text/plain".into())
                .await
                .is_none()
        );
    }
}

#[tokio::test]
async fn cancelling_imported_read_releases_demand_without_reading_native_promise() {
    let (router, native, mut requests, mut remote, endpoint) = setup().await;
    let mut demands = Demands::default();
    let published = start(&router, &mut remote, &endpoint, &demands).await;
    let request = requests.recv().await.unwrap();
    let publication = native.confirm(&request);
    request.reply.send(publication).ok().unwrap();
    assert!(published.await.unwrap());
    let imported = router.offer(None).await.unwrap();
    let reading = tokio::spawn(router.read(imported.revision, "text/plain".into()));
    let demand = tokio::time::timeout(std::time::Duration::from_secs(1), demands.receive())
        .await
        .unwrap()
        .unwrap();
    assert!(demand.is_current());
    reading.abort();
    assert!(matches!(reading.await, Err(error) if error.is_cancelled()));
    assert!(!demand.is_current());
}

#[tokio::test]
async fn accepted_copy_survives_focus_but_new_local_copy_fences_even_held_bytes() {
    let (router, native, mut requests, mut remote, endpoint) = setup().await;
    let demands = Demands::default();
    let published = start(&router, &mut remote, &endpoint, &demands).await;
    let request = requests.recv().await.unwrap();
    let source = request.source.clone();
    let publication = native.confirm(&request);
    assert!(endpoint.focus(None));
    remote.event(Event::Offer { offer: None });
    assert!(publication.is_current());
    request.reply.send(publication).ok().unwrap();
    assert!(published.await.unwrap());
    assert!(matches!(
        router.offer(None).await.unwrap().origin,
        ClipboardOrigin::Imported { .. }
    ));
    let (read, _) = remote
        .receiver
        .read_copy(
            router
                .0
                .state
                .lock()
                .unwrap()
                .accepted
                .as_ref()
                .unwrap()
                .source
                .copy()
                .unwrap(),
            "text/plain",
        )
        .unwrap();
    remote.event(Event::Data {
        id: read.id(),
        offset: 0,
        chunk: Some(Chunk {
            bytes: b"remote".to_vec(),
            eof: true,
        }),
    });
    let data = read.receive().await.unwrap();
    assert_eq!(data.bytes().unwrap(), b"remote");
    let local = native.local();
    assert_eq!(router.offer(None).await, Some(local));
    assert!(!source.is_current());
    assert!(data.bytes().is_err());
}

#[tokio::test]
async fn delayed_ack_cannot_adopt_new_local_revision_or_replay_on_refocus() {
    let (router, native, mut requests, mut remote, endpoint) = setup().await;
    let demands = Demands::default();
    let proposal = endpoint.propose(remote.lease()).unwrap();
    let old = proposal.expected;
    let local = native.local();
    assert_ne!(local.revision, old);
    assert_eq!(router.offer(None).await, Some(local));
    assert!(!proposal.is_current());
    assert!(
        router
            .publish(proposal, remote.copy().await, &demands)
            .await
            .is_none()
    );
    assert!(requests.try_recv().is_err());
    remote.offer(1, 2);
    remote.focus(&endpoint);
    assert!(endpoint.propose(remote.lease()).is_none());
    remote.offer(2, 2);
    assert!(endpoint.propose(remote.lease()).is_some());
}

#[tokio::test]
async fn focus_loss_before_native_confirmation_cancels_pending_copy() {
    let (router, _, mut requests, mut remote, endpoint) = setup().await;
    let demands = Demands::default();
    let published = start(&router, &mut remote, &endpoint, &demands).await;
    let request = requests.recv().await.unwrap();
    assert!(endpoint.focus(None));
    assert!(!request.source.publication_confirmed());
    drop(request);
    assert!(!published.await.unwrap());
}

#[tokio::test]
async fn cancelled_unpolled_publication_releases_claim_and_only_its_proposal() {
    let (router, _, mut requests, mut remote, endpoint) = setup().await;
    let demands = Demands::default();
    let proposal = endpoint.propose(remote.lease()).unwrap();
    let unpublished = router.publish(proposal, remote.copy().await, &demands);
    drop(unpublished);
    assert!(requests.try_recv().is_err());
    assert!(remote.receiver.copy_release().is_some());
    remote.offer(2, 1);
    let old = endpoint.propose(remote.lease()).unwrap();
    endpoint.focus(None);
    remote.offer(3, 2);
    remote.focus(&endpoint);
    let newer = endpoint.propose(remote.lease()).unwrap();
    drop(old);
    assert!(newer.is_current());
    drop(newer);
}

#[tokio::test]
async fn cancelling_or_retiring_an_inflight_publication_releases_native_and_remote_authority() {
    for confirm in [false, true] {
        for retire in 0..3 {
            let (router, native, mut requests, mut remote, endpoint) = setup().await;
            let demands = Demands::default();
            let published = start(&router, &mut remote, &endpoint, &demands).await;
            let request = requests.recv().await.unwrap();
            let source = request.source.clone();
            let publication = confirm.then(|| native.confirm(&request));
            match retire {
                0 => published.abort(),
                1 => endpoint.revoke(),
                _ => router.close(),
            }
            if retire == 0 {
                assert!(published.await.unwrap_err().is_cancelled());
                assert!(request.reply.is_closed());
            } else {
                if let Some(publication) = publication {
                    assert!(!publication.is_current());
                    request.reply.send(publication).ok().unwrap();
                } else {
                    drop(request);
                }
                assert!(!published.await.unwrap());
            }
            assert!(!source.is_current());
            assert!(remote.receiver.copy_release().is_some());
        }
    }
}

#[tokio::test]
async fn identical_offer_from_another_connection_cannot_authorize_publication() {
    let (router, _, mut requests, remote, endpoint) = setup().await;
    let demands = Demands::default();
    let mut foreign = Remote::at(remote.runtime, remote.worker);
    let proposal = endpoint.propose(remote.lease()).unwrap();
    assert_eq!(foreign.lease().offer(), proposal.lease().offer());
    assert!(
        router
            .publish(proposal, foreign.copy().await, &demands)
            .await
            .is_none()
    );
    assert!(requests.try_recv().is_err());
    assert!(foreign.receiver.copy_release().is_some());
    assert!(router.bind(&remote.lease()).is_none());
    assert!(endpoint.propose(foreign.lease()).is_none());
}

#[tokio::test]
async fn switching_endpoints_preserves_copy_until_permission_or_connection_retires() {
    for disconnect in [false, true] {
        let (router, native, mut requests, mut remote, endpoint) = setup().await;
        let demands = Demands::default();
        let published = start(&router, &mut remote, &endpoint, &demands).await;
        let request = requests.recv().await.unwrap();
        let source = request.source.clone();
        let publication = native.confirm(&request);
        request.reply.send(publication).ok().unwrap();
        assert!(published.await.unwrap());
        router.offer(None).await.unwrap();
        let other = Remote::new();
        let other_endpoint = router.bind(&other.lease()).unwrap();
        other.focus(&other_endpoint);
        assert!(source.is_current());
        assert!(endpoint.focus(None));
        assert!(other_endpoint.propose(other.lease()).is_some());
        if disconnect {
            remote.receiver.disconnect();
        } else {
            endpoint.revoke();
        }
        assert!(!source.is_current());
        let next = Remote::at(remote.runtime, remote.worker);
        let next_endpoint = router.bind(&next.lease()).unwrap();
        next.focus(&next_endpoint);
        drop(endpoint);
        assert!(next_endpoint.propose(next.lease()).is_none());
        native.withdraw();
        router.offer(None).await.unwrap();
        assert!(next_endpoint.propose(next.lease()).is_some());
    }
}

#[tokio::test]
async fn busy_offer_is_retried_after_own_native_generation_is_observed() {
    let (router, native, mut requests, mut remote, endpoint) = setup().await;
    let demands = Demands::default();
    let published = start(&router, &mut remote, &endpoint, &demands).await;
    let request = requests.recv().await.unwrap();
    assert!(endpoint.propose(remote.lease()).is_none());
    let publication = native.confirm(&request);
    request.reply.send(publication).ok().unwrap();
    assert!(published.await.unwrap());
    let other = Remote::new();
    let other_endpoint = router.bind(&other.lease()).unwrap();
    other.focus(&other_endpoint);
    assert!(other_endpoint.propose(other.lease()).is_none());
    router.offer(None).await.unwrap();
    assert!(other_endpoint.propose(other.lease()).is_some());
}

#[tokio::test]
async fn replacement_copy_waits_for_revoked_native_promise_to_withdraw() {
    for observed in [false, true] {
        let (router, native, mut requests, mut remote, endpoint) = setup().await;
        let demands = Demands::default();
        let published = start(&router, &mut remote, &endpoint, &demands).await;
        let request = requests.recv().await.unwrap();
        let publication = native.confirm(&request);
        request.reply.send(publication).ok().unwrap();
        assert!(published.await.unwrap());
        if observed {
            router.offer(None).await.unwrap();
        }
        let copy = router
            .0
            .state
            .lock()
            .unwrap()
            .accepted
            .as_ref()
            .unwrap()
            .source
            .copy()
            .unwrap()
            .id()
            .to_owned();
        remote.event(Event::CopyRevoked { id: copy });
        remote.offer(2, 1);
        // The native owner withdraws an invalid promise asynchronously. Its old
        // metadata is no longer a usable baseline, even before withdrawal arrives.
        assert!(endpoint.propose(remote.lease()).is_none());
        router.offer(None).await.unwrap();
        assert!(endpoint.propose(remote.lease()).is_none());
        native.withdraw();
        assert!(endpoint.propose(remote.lease()).is_none());
        router.offer(None).await.unwrap();
        let published = start(&router, &mut remote, &endpoint, &demands).await;
        let request = requests.recv().await.unwrap();
        let publication = native.confirm(&request);
        request.reply.send(publication).ok().unwrap();
        assert!(published.await.unwrap());
    }
}

#[tokio::test]
async fn stale_native_reply_cannot_undo_newer_local_copy_and_close_fences_local_reads() {
    let (router, native, _, _, _) = setup().await;
    let local = native.local();
    assert_eq!(router.offer(None).await, Some(local.clone()));
    // Model an out-of-order reply from another metadata task.
    assert!(!router.observe(&ClipboardOffer::local(local.revision - 1, vec![])));
    let read = router
        .read(local.revision, "text/plain".into())
        .await
        .unwrap();
    assert_eq!(read.bytes(), Some(b"local".as_slice()));
    router.close();
    assert!(read.bytes().is_none());
    assert!(router.offer(None).await.is_none());
    assert!(
        router
            .read(local.revision, "text/plain".into())
            .await
            .is_none()
    );
}

#[tokio::test]
async fn endpoint_binds_before_any_native_offer_without_conferring_focus() {
    let (native, _) = Native::new();
    let router = Router::new(native);
    let mut receiver = Receiver::default();
    assert!(receiver.connection().is_none());
    let runtime = Uuid::new_v4();
    receiver
        .enable_version(&runtime.to_string(), wire::COPY_VERSION)
        .unwrap();
    let connection = receiver.connection().unwrap();
    let endpoint = router.bind_connection(connection.clone()).unwrap();
    assert!(receiver.lease().is_none());
    assert!(!endpoint.observes_clipboard());
    assert!(router.bind_connection(connection.clone()).is_none());
    let pending = endpoint.begin_focus(Uuid::new_v4()).unwrap();
    assert!(pending.is_current());
    receiver.disconnect();
    assert!(!pending.is_current());
    assert!(!connection.is_current());
    assert!(!pending.acknowledge(Grant::new(runtime, Uuid::new_v4(), 1).unwrap()));
    assert!(router.bind_connection(connection).is_none());
}

#[tokio::test]
async fn focus_acknowledgments_follow_global_order_across_connections() {
    let (router, _, _, mut a, endpoint_a) = setup().await;
    let b = Remote::new();
    let endpoint_b = router.bind(&b.lease()).unwrap();
    let delayed_a = endpoint_a.begin_focus(a.worker).unwrap();
    let latest_b = endpoint_b.begin_focus(b.worker).unwrap();
    assert!(!delayed_a.is_current());
    assert!(latest_b.acknowledge(Grant::new(b.runtime, b.worker, 1).unwrap()));
    assert!(!delayed_a.acknowledge(Grant::new(a.runtime, a.worker, 2).unwrap()));
    assert!(endpoint_a.propose(a.lease()).is_none());
    assert!(endpoint_b.propose(b.lease()).is_some());
    // Even returning to the same endpoint requires the latest request, not
    // merely a runtime route with a higher number from an earlier UI event.
    let old = endpoint_a.begin_focus(a.worker).unwrap();
    let new = endpoint_a.begin_focus(a.worker).unwrap();
    assert!(!old.acknowledge(Grant::new(a.runtime, a.worker, 99).unwrap()));
    a.offer(2, 2);
    assert!(new.acknowledge(Grant::new(a.runtime, a.worker, 2).unwrap()));
    assert!(endpoint_a.propose(a.lease()).is_some());
}

#[tokio::test]
async fn blur_drop_and_revocation_fence_pending_focus_without_affecting_newer_requests() {
    for cancel in 0..4 {
        let (router, _, _, remote, endpoint) = setup().await;
        let pending = endpoint.begin_focus(remote.worker).unwrap();
        match cancel {
            0 => {
                endpoint.blur();
            }
            1 => endpoint.revoke(),
            2 => router.close(),
            _ => {
                drop(pending);
                assert!(!endpoint.observes_clipboard());
                continue;
            }
        }
        assert!(!pending.is_current());
        assert!(!pending.acknowledge(Grant::new(remote.runtime, remote.worker, 2).unwrap()));
    }
    let (router, _, _, _, a) = setup().await;
    let other = Remote::new();
    let b = router.bind(&other.lease()).unwrap();
    let old = a.begin_focus(Uuid::new_v4()).unwrap();
    let latest = b.begin_focus(other.worker).unwrap();
    a.blur();
    drop(old);
    drop(a);
    assert!(latest.is_current());
    assert!(latest.acknowledge(Grant::new(other.runtime, other.worker, 1).unwrap()));
    assert!(b.propose(other.lease()).is_some());
}

#[tokio::test]
async fn wrong_revoked_or_nonmonotonic_focus_grants_never_activate() {
    // Keep the router alive; Endpoint deliberately owns only a weak reference.
    let (router, _, _, mut remote, endpoint) = setup().await;
    for kind in 0..4 {
        let grant = Grant::new(
            if kind == 0 {
                Uuid::new_v4()
            } else {
                remote.runtime
            },
            if kind == 3 {
                Uuid::new_v4()
            } else {
                remote.worker
            },
            if kind == 1 { 1 } else { 2 },
        )
        .unwrap();
        if kind == 2 {
            grant.revoke();
        }
        assert!(
            !endpoint
                .begin_focus(remote.worker)
                .unwrap()
                .acknowledge(grant)
        );
        assert!(!endpoint.observes_clipboard());
        assert!(endpoint.propose(remote.lease()).is_none());
    }
    remote.offer(2, 2);
    assert!(
        endpoint
            .begin_focus(remote.worker)
            .unwrap()
            .acknowledge(Grant::new(remote.runtime, remote.worker, 2).unwrap())
    );
    assert!(endpoint.propose(remote.lease()).is_some());
    drop(router);
}

/// A published import on a router that downloads into `root`.
#[cfg(unix)]
async fn published_with_downloads(
    root: std::path::PathBuf,
) -> (Router, Arc<Native>, Remote, Endpoint, Demands, u64) {
    let (native, mut requests) = Native::new();
    let router = Router::with_download_root(native.clone(), root);
    router.offer(None).await.unwrap();
    let mut remote = Remote::new();
    let endpoint = router.bind(&remote.lease()).unwrap();
    remote.focus(&endpoint);
    let demands = Demands::default();
    let published = start(&router, &mut remote, &endpoint, &demands).await;
    let request = requests.recv().await.unwrap();
    let publication = native.confirm(&request);
    let generation = publication.generation();
    request.reply.send(publication).ok().unwrap();
    assert!(published.await.unwrap());
    (router, native, remote, endpoint, demands, generation)
}

#[cfg(unix)]
#[tokio::test]
async fn files_attach_only_to_the_current_import() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("folder");
    // The download can end before the router observed its own publication.
    let (router, native, _remote, _endpoint, _demands, generation) =
        published_with_downloads(dir.path().into()).await;
    assert!(router.attach_files(generation + 1, &folder).await.is_none());
    let revision = router.attach_files(generation, &folder).await.unwrap();
    assert_eq!(
        *native.attached.lock().unwrap(),
        [(generation, folder.clone())]
    );
    // The attached offer is still this import: it survives observation.
    let attached = router.offer(None).await.unwrap();
    assert_eq!(attached.revision, revision);
    assert!(router.attach_files(generation, &folder).await.is_some());

    // A newer Mac copy wins, even before the router observed it.
    native.local();
    assert!(router.attach_files(generation, &folder).await.is_none());
    assert_eq!(native.attached.lock().unwrap().len(), 2);
}

#[cfg(unix)]
#[tokio::test]
async fn files_never_attach_after_revocation_or_close() {
    for revoke in 0..3 {
        let dir = tempfile::tempdir().unwrap();
        let (router, native, mut remote, endpoint, _demands, generation) =
            published_with_downloads(dir.path().into()).await;
        match revoke {
            0 => endpoint.revoke(),
            1 => remote.receiver.disconnect(),
            _ => router.close(),
        }
        assert!(router.attach_files(generation, dir.path()).await.is_none());
        assert!(native.attached.lock().unwrap().is_empty());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn a_download_ticket_follows_its_copy() {
    for revoke in 0..4 {
        let dir = tempfile::tempdir().unwrap();
        let (router, native, mut remote, endpoint, _demands, generation) =
            published_with_downloads(dir.path().into()).await;
        assert!(router.begin_download(generation + 1).is_none());
        let (_, ticket) = router.begin_download(generation).unwrap();
        assert!(ticket.is_current());
        match revoke {
            0 => {
                native.local();
                router.offer(None).await.unwrap();
            }
            1 => endpoint.revoke(),
            2 => remote.receiver.disconnect(),
            _ => router.close(),
        }
        assert!(!ticket.is_current(), "case {revoke}");
        assert!(router.begin_download(generation).is_none());
    }
    // Without a cache nothing downloads.
    let (router, _, _, _, _) = setup().await;
    assert!(router.begin_download(1).is_none());
}
