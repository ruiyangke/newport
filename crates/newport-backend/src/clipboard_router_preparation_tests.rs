use super::*;
use newport_platform::clipboard::FILES_FORMAT;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Preparing,
    Committing,
    Native,
    Canceled,
}
#[derive(Default)]
struct Controlled {
    claimed: tokio::sync::Notify,
    release: tokio::sync::Notify,
    committed: tokio::sync::Notify,
    partial: bool,
}
struct Preparing {
    source: Arc<dyn ClipboardSource>,
    generation: u64,
    baseline: isize,
    offer: Arc<Mutex<ClipboardOffer>>,
    phase: Arc<Mutex<Phase>>,
    control: Option<Arc<Controlled>>,
}
impl ClipboardPublication for Preparing {
    fn is_current(&self) -> bool {
        let offer = self.offer.lock().unwrap();
        self.source.is_current()
            && match *self.phase.lock().unwrap() {
                Phase::Preparing => offer.revision == self.baseline,
                Phase::Committing => {
                    offer.revision == self.baseline
                        || offer.origin
                            == (ClipboardOrigin::Imported {
                                publication: self.generation,
                            })
                }
                Phase::Native => {
                    offer.origin
                        == (ClipboardOrigin::Imported {
                            publication: self.generation,
                        })
                }
                Phase::Canceled => false,
            }
    }
    fn generation(&self) -> u64 {
        self.generation
    }
    fn preparing_revision(&self) -> Option<isize> {
        (*self.phase.lock().unwrap() == Phase::Preparing).then_some(self.baseline)
    }
    fn cancel_preparation(&self) -> bool {
        let mut phase = self.phase.lock().unwrap();
        if *phase != Phase::Preparing {
            return false;
        }
        *phase = Phase::Canceled;
        drop(phase);
        self.source.publication_revoked();
        true
    }
    fn attach_files(&self, _: &std::path::Path) -> ClipboardFuture<isize> {
        let (source, offer, phase, control, baseline, generation) = (
            self.source.clone(),
            self.offer.clone(),
            self.phase.clone(),
            self.control.clone(),
            self.baseline,
            self.generation,
        );
        Box::pin(async move {
            {
                let offer = offer.lock().unwrap();
                let mut phase = phase.lock().unwrap();
                if !source.is_current() || *phase != Phase::Preparing || offer.revision != baseline
                {
                    return None;
                }
                assert!(source.publication_confirmed());
                *phase = Phase::Committing;
            }
            let (reply, receipt) = tokio::sync::oneshot::channel();
            // The native operation survives cancellation of its awaiting client.
            tokio::spawn(async move {
                if let Some(control) = &control {
                    control.claimed.notify_one();
                    control.release.notified().await;
                }
                let partial = control.as_ref().is_some_and(|control| control.partial);
                let mut offer = offer.lock().unwrap();
                offer.revision += 1;
                offer.formats = if partial {
                    Vec::new()
                } else {
                    vec![FILES_FORMAT.into()]
                };
                offer.origin = ClipboardOrigin::Imported {
                    publication: generation,
                };
                *phase.lock().unwrap() = Phase::Native;
                let _ = reply.send((!partial).then_some(offer.revision));
                if let Some(control) = control {
                    control.committed.notify_one();
                }
            });
            receipt.await.ok().flatten()
        })
    }
}
impl Drop for Preparing {
    fn drop(&mut self) {
        self.source.publication_revoked();
    }
}
fn prepare(native: &Native, request: &Request, generation: u64) -> Box<dyn ClipboardPublication> {
    prepare_controlled(native, request, generation, None)
}
fn prepare_controlled(
    native: &Native,
    request: &Request,
    generation: u64,
    control: Option<Arc<Controlled>>,
) -> Box<dyn ClipboardPublication> {
    assert_eq!(request.expected, native.offer.lock().unwrap().revision);
    assert!(request.source.preparation_confirmed());
    Box::new(Preparing {
        source: request.source.clone(),
        generation,
        baseline: request.expected,
        offer: native.offer.clone(),
        phase: Arc::new(Mutex::new(Phase::Preparing)),
        control,
    })
}
async fn accept(
    router: &Router,
    native: &Native,
    requests: &mut mpsc::UnboundedReceiver<Request>,
    remote: &mut Remote,
    endpoint: &Endpoint,
    demands: &Demands,
    generation: u64,
) -> Published {
    remote.offer_formats(2, 1, &[FILES_FORMAT]);
    let proposal = endpoint.propose(remote.lease()).unwrap();
    let task = tokio::spawn(router.publish(proposal, remote.copy().await, demands));
    let request = requests.recv().await.unwrap();
    let publication = prepare(native, &request, generation);
    assert!(request.reply.send(publication).is_ok());
    task.await.unwrap().unwrap()
}

#[tokio::test]
async fn file_preparation_keeps_real_local_origin_until_attachment() {
    let (router, native, mut requests, mut remote, endpoint) = setup().await;
    let demands = Demands::default();
    let before = router.offer(None).await.unwrap();
    let published = accept(
        &router,
        &native,
        &mut requests,
        &mut remote,
        &endpoint,
        &demands,
        100,
    )
    .await;
    endpoint.blur();
    assert!(published.source.is_current());
    assert_eq!(router.offer(None).await.unwrap(), before);
    assert!(router.0.state.lock().unwrap().accepted.is_none());
    assert_eq!(
        router
            .attach_files(100, std::path::Path::new("prepared-cache"))
            .await,
        Some(before.revision + 1)
    );
    let after = router.offer(None).await.unwrap();
    assert_eq!(after.origin, ClipboardOrigin::Imported { publication: 100 });
    assert!(!endpoint.may_forward_import(&after));
    assert!(router.0.state.lock().unwrap().preparation.is_none());
    assert!(router.0.state.lock().unwrap().accepted.is_some());
}

#[tokio::test]
async fn canceled_or_stale_preparation_never_replaces_the_native_copy() {
    for reason in 0..4 {
        let (router, native, mut requests, mut remote, endpoint) = setup().await;
        let demands = Demands::default();
        let published = accept(
            &router,
            &native,
            &mut requests,
            &mut remote,
            &endpoint,
            &demands,
            101,
        )
        .await;
        let expected = match reason {
            0 => native.local(),
            _ => {
                match reason {
                    1 => endpoint.revoke(),
                    2 => remote.receiver.disconnect(),
                    _ => router.finish_preparation(101),
                }
                native.offer.lock().unwrap().clone()
            }
        };
        assert_eq!(router.offer(None).await.unwrap(), expected);
        assert!(
            router
                .attach_files(101, std::path::Path::new("prepared-cache"))
                .await
                .is_none()
        );
        assert_eq!(*native.offer.lock().unwrap(), expected);
        assert!(!published.source.is_current());
        assert!(router.0.state.lock().unwrap().preparation.is_none());
    }
}

#[tokio::test]
async fn preparing_another_servers_files_preserves_the_existing_import_and_echo_identity() {
    let (router, native, mut requests, mut first, origin) = setup().await;
    let first_demands = Demands::default();
    let proposal = origin.propose(first.lease()).unwrap();
    let task = tokio::spawn(router.publish(proposal, first.copy().await, &first_demands));
    let request = requests.recv().await.unwrap();
    let publication = native.confirm(&request);
    assert!(request.reply.send(publication).is_ok());
    let published = task.await.unwrap().unwrap();
    let original = router.offer(None).await.unwrap();
    let mut second = Remote::new();
    let endpoint = router.bind(&second.lease()).unwrap();
    second.focus(&endpoint);
    let second_demands = Demands::default();
    let prepared = accept(
        &router,
        &native,
        &mut requests,
        &mut second,
        &endpoint,
        &second_demands,
        102,
    )
    .await;
    assert!(published.source.is_current());
    assert_eq!(router.offer(None).await.unwrap(), original);
    assert!(!origin.may_forward_import(&original));
    assert!(endpoint.may_forward_import(&original));
    router.finish_preparation(102);
    assert!(!prepared.source.is_current());
    assert!(published.source.is_current());
    assert_eq!(router.offer(None).await.unwrap(), original);
    assert_eq!(
        router
            .0
            .state
            .lock()
            .unwrap()
            .accepted
            .as_ref()
            .unwrap()
            .publication
            .generation(),
        published.generation
    );
}

#[tokio::test]
async fn cancellation_after_claim_preserves_the_publication_until_real_native_promotion() {
    for partial in [false, true] {
        let (router, native, mut requests, mut remote, endpoint) = setup().await;
        let demands = Demands::default();
        remote.offer_formats(2, 1, &[FILES_FORMAT]);
        let proposal = endpoint.propose(remote.lease()).unwrap();
        let task = tokio::spawn(router.publish(proposal, remote.copy().await, &demands));
        let request = requests.recv().await.unwrap();
        let control = Arc::new(Controlled {
            partial,
            ..Default::default()
        });
        let publication = prepare_controlled(&native, &request, 105, Some(control.clone()));
        assert!(request.reply.send(publication).is_ok());
        let published = task.await.unwrap().unwrap();
        let before = router.offer(None).await.unwrap();
        let downloading = router.clone();
        let attachment = tokio::spawn(async move {
            downloading
                .attach_files(105, std::path::Path::new("prepared-cache"))
                .await
        });
        control.claimed.notified().await;
        if partial {
            control.release.notify_one();
            assert!(
                attachment.await.unwrap().is_none(),
                "partial receipt does not claim complete files"
            );
            router.finish_preparation(105);
        } else {
            attachment.abort();
            assert!(attachment.await.unwrap_err().is_cancelled());
            router.finish_preparation(105);
            assert_eq!(router.offer(None).await.unwrap(), before);
            assert!(
                published.source.is_current(),
                "drop after claim must preserve the copy lifetime"
            );
            assert!(router.0.state.lock().unwrap().preparation.is_some());
            control.release.notify_one();
        }
        control.committed.notified().await;
        let after = router.offer(None).await.unwrap();
        assert_eq!(after.origin, ClipboardOrigin::Imported { publication: 105 });
        assert!(!endpoint.may_forward_import(&after));
        assert!(router.0.state.lock().unwrap().preparation.is_none());
        assert!(router.0.state.lock().unwrap().accepted.is_some());
        assert!(
            published.source.is_current(),
            "partial committed copy must keep imported provenance"
        );
        assert_eq!(after.formats.contains(&FILES_FORMAT.into()), !partial);
    }
}
