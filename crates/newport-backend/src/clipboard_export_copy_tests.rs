use super::*;
use crate::agent_runtime::protocol::clipboard::{Chunk, Offer, Origin};

#[path = "clipboard_export_copy_live_tests.rs"]
mod live;

fn offer() -> ExportOffer {
    ExportOffer {
        offer: Offer {
            runtime: uuid::Uuid::nil().to_string(),
            revision: 1,
            origin: Origin::Native {
                worker: "display".into(),
                revision: 1,
            },
            formats: vec!["text/plain".into(), "image/png".into()],
        },
        route_revision: 1,
    }
}
fn accept(receiver: &mut Receiver, event: Event) -> Result<(), String> {
    receiver.accept(&event.encode().unwrap())
}
fn setup() -> Receiver {
    let mut receiver = Receiver::default();
    receiver
        .enable_version(&offer().offer.runtime, wire::COPY_VERSION)
        .unwrap();
    accept(
        &mut receiver,
        Event::Offer {
            offer: Some(offer()),
        },
    )
    .unwrap();
    receiver
}
fn acknowledgment(receiver: &mut Receiver, id: u64, offer: ExportOffer) -> CopyLease {
    let copy = CopyLease {
        id: uuid::Uuid::new_v4().to_string(),
        offer,
    };
    accept(
        receiver,
        Event::Committed {
            id,
            copy: Some(copy.clone()),
        },
    )
    .unwrap();
    copy
}
async fn committed(receiver: &mut Receiver) -> CommittedCopy {
    let proposal = receiver.lease().unwrap();
    let (commit, command) = receiver.commit(&proposal).unwrap();
    assert!(
        matches!(command, Control::Commit { id, offer } if id == commit.id() && offer == *proposal.offer())
    );
    acknowledgment(receiver, commit.id(), proposal.offer().clone());
    commit.receive().await.unwrap()
}
fn data(receiver: &mut Receiver, id: u64, offset: usize, bytes: &[u8], eof: bool) {
    accept(
        receiver,
        Event::Data {
            id,
            offset,
            chunk: Some(Chunk {
                bytes: bytes.into(),
                eof,
            }),
        },
    )
    .unwrap();
}

#[tokio::test]
async fn copies_require_explicit_version_and_exact_connection_authority() {
    let mut receiver = setup();
    let mut foreign = setup();
    assert!(receiver.commit(&foreign.lease().unwrap()).is_err());
    let copy = committed(&mut receiver).await;
    assert!(foreign.read_copy(&copy, "text/plain").is_err());
    assert!(foreign.release_copy(&copy).is_err());
    assert!(copy.is_current());
    assert!(receiver.read_copy(&copy, "unoffered").is_err());
    assert!(receiver.enable(&offer().offer.runtime).is_err());
    receiver.disable();
    assert!(!copy.is_current());
    receiver.enable(&offer().offer.runtime).unwrap();
    accept(
        &mut receiver,
        Event::Offer {
            offer: Some(offer()),
        },
    )
    .unwrap();
    assert!(receiver.commit(&receiver.lease().unwrap()).is_err());
    assert!(receiver.enable_version(&offer().offer.runtime, 99).is_err());
}

#[tokio::test]
async fn acknowledged_copy_and_held_bytes_survive_focus_changes() {
    let mut receiver = setup();
    let focused = receiver.lease().unwrap();
    let copy = committed(&mut receiver).await;
    let (read, command) = receiver.read_copy(&copy, "text/plain").unwrap();
    assert!(
        matches!(command, Control::ReadCopy { id, copy: lease, .. } if id == read.id() && lease.id == copy.id())
    );
    data(&mut receiver, read.id(), 0, b"copied", true);
    let held = read.receive().await.unwrap();
    assert_eq!(held.copy_id(), Some(copy.id()));
    let (focused_read, _) = receiver.read(focused.offer(), "text/plain").unwrap();
    let (copy_read, _) = receiver.read_copy(&copy, "image/png").unwrap();
    accept(&mut receiver, Event::Offer { offer: None }).unwrap();
    assert!(!focused.is_current());
    assert!(focused_read.receive().await.is_err());
    assert!(copy.is_current());
    assert_eq!(held.bytes().unwrap(), b"copied");
    data(&mut receiver, copy_read.id(), 0, b"image", true);
    let image = copy_read.receive().await.unwrap();
    let mut refocused = offer();
    refocused.route_revision += 2;
    accept(
        &mut receiver,
        Event::Offer {
            offer: Some(refocused),
        },
    )
    .unwrap();
    assert_eq!(image.bytes().unwrap(), b"image");
    assert!(copy.is_current());
    assert_eq!(held.bytes().unwrap(), b"copied");
}

#[tokio::test]
async fn revocation_fences_held_bytes_and_pending_reads_before_new_source() {
    let mut receiver = setup();
    let copy = committed(&mut receiver).await;
    let (read, _) = receiver.read_copy(&copy, "text/plain").unwrap();
    data(&mut receiver, read.id(), 0, b"old", true);
    let held = read.receive().await.unwrap();
    let (pending, _) = receiver.read_copy(&copy, "image/png").unwrap();
    accept(
        &mut receiver,
        Event::CopyRevoked {
            id: copy.id().into(),
        },
    )
    .unwrap();
    assert!(!copy.is_current());
    assert!(held.bytes().is_err());
    assert!(pending.receive().await.is_err());
    assert!(receiver.read_copy(&copy, "text/plain").is_err());
    let mut next = offer();
    next.offer.revision += 1;
    accept(&mut receiver, Event::Offer { offer: Some(next) }).unwrap();
    assert!(receiver.committed.is_none());
}

#[tokio::test]
async fn local_revocation_releases_authority_and_drains_late_bytes() {
    let mut receiver = setup();
    let copy = committed(&mut receiver).await;
    let (read, _) = receiver.read_copy(&copy, "text/plain").unwrap();
    let id = read.id();
    data(&mut receiver, id, 0, b"partial", false);
    receiver.release_copy(&copy).unwrap();
    assert!(!copy.is_current());
    assert!(read.receive().await.is_err());
    assert_eq!(receiver.cancellations(), [id]);
    assert_eq!(receiver.budget.load(Ordering::Acquire), 0);
    data(&mut receiver, id, 7, b"late", true);
    assert_eq!(receiver.budget.load(Ordering::Acquire), 0);
    assert!(receiver.commit(&receiver.lease().unwrap()).is_err());
    let Some(Control::Release { copy: release }) = receiver.copy_release() else {
        panic!("missing release")
    };
    assert_eq!(release.id, copy.id());
    receiver.copy_release_sent(copy.id());
    assert!(receiver.copy_release().is_none());
    accept(
        &mut receiver,
        Event::CopyRevoked {
            id: copy.id().into(),
        },
    )
    .unwrap();
    assert!(receiver.cancellations().is_empty());
    assert!(receiver.committed.is_none());
    let next = committed(&mut receiver).await;
    receiver.release_copy(&copy).unwrap();
    assert!(next.is_current(), "stale handle revoked a replacement");
}

#[tokio::test]
async fn dropping_owned_copy_or_unreceived_acknowledgment_releases_it() {
    for receive in [false, true] {
        let mut receiver = setup();
        let (commit, _) = receiver.commit(&receiver.lease().unwrap()).unwrap();
        let copy = acknowledgment(&mut receiver, commit.id(), offer());
        if receive {
            drop(commit.receive().await.unwrap());
        } else {
            drop(commit);
        }
        receiver.expire();
        assert!(
            matches!(receiver.copy_release(), Some(Control::Release { copy: released }) if released == copy)
        );
    }
}

#[tokio::test]
async fn abandoned_or_stale_proposal_drains_then_releases_late_acceptance() {
    for stale in [false, true] {
        let mut receiver = setup();
        let (commit, _) = receiver.commit(&receiver.lease().unwrap()).unwrap();
        let id = commit.id();
        if stale {
            accept(&mut receiver, Event::Offer { offer: None }).unwrap();
            assert!(commit.receive().await.is_err());
        } else {
            drop(commit);
        }
        receiver.expire();
        assert!(
            receiver.committing.is_some(),
            "lost correlation before reply"
        );
        let copy = acknowledgment(&mut receiver, id, offer());
        assert!(
            !receiver
                .committed
                .as_ref()
                .unwrap()
                .current
                .load(Ordering::Acquire)
        );
        assert!(
            matches!(receiver.copy_release(), Some(Control::Release { copy: release }) if release == copy)
        );
        assert!(receiver.committing.is_none());
    }
}

#[tokio::test]
async fn rejection_preserves_previous_copy_but_replacement_revokes_it() {
    let mut receiver = setup();
    let old = committed(&mut receiver).await;
    let (read, _) = receiver.read_copy(&old, "text/plain").unwrap();
    data(&mut receiver, read.id(), 0, b"old", true);
    let held = read.receive().await.unwrap();
    let (commit, _) = receiver.commit(&receiver.lease().unwrap()).unwrap();
    accept(
        &mut receiver,
        Event::Committed {
            id: commit.id(),
            copy: None,
        },
    )
    .unwrap();
    assert!(commit.receive().await.is_err());
    assert!(old.is_current());
    assert_eq!(held.bytes().unwrap(), b"old");
    let (pending, _) = receiver.read_copy(&old, "image/png").unwrap();
    let new = committed(&mut receiver).await;
    assert!(pending.receive().await.is_err());
    assert!(held.bytes().is_err());
    assert!(!old.is_current());
    receiver.release_copy(&old).unwrap();
    assert!(new.is_current());
}

#[tokio::test]
async fn copy_acknowledgments_must_match_the_exact_request() {
    for mutation in 0..5 {
        let mut receiver = setup();
        let old = committed(&mut receiver).await;
        let (commit, _) = receiver.commit(&receiver.lease().unwrap()).unwrap();
        let mut id = commit.id();
        let mut accepted = CopyLease {
            id: uuid::Uuid::new_v4().to_string(),
            offer: offer(),
        };
        match mutation {
            0 => id += 1,
            1 => accepted.offer.offer.runtime = uuid::Uuid::new_v4().to_string(),
            2 => accepted.offer.offer.revision += 1,
            3 => accepted.offer.offer.formats.pop().map(|_| ()).unwrap(),
            _ => accepted.offer.route_revision += 1,
        }
        assert!(
            accept(
                &mut receiver,
                Event::Committed {
                    id,
                    copy: Some(accepted)
                }
            )
            .is_err()
        );
        assert!(!old.is_current());
        assert!(commit.receive().await.is_err());
        assert!(receiver.connection_fence().load(Ordering::Acquire));
    }
}

#[tokio::test]
async fn protocol_ordering_failures_disconnect_instead_of_retaining_old_authority() {
    for mutation in 0..3 {
        let mut receiver = setup();
        let copy = committed(&mut receiver).await;
        let event = match mutation {
            0 => Event::Committed { id: 1, copy: None },
            1 => Event::CopyRevoked {
                id: uuid::Uuid::new_v4().to_string(),
            },
            _ => {
                let mut next = offer();
                next.offer.revision += 1;
                Event::Offer { offer: Some(next) }
            }
        };
        assert!(accept(&mut receiver, event).is_err());
        assert!(!copy.is_current());
    }
}

#[tokio::test(start_paused = true)]
async fn missing_commit_or_release_event_retires_the_connection_without_replay() {
    for release in [false, true] {
        let mut receiver = setup();
        let copy = committed(&mut receiver).await;
        let mut pending = None;
        if release {
            copy.revoke();
            receiver.copy_release_sent(copy.id());
        } else {
            pending = Some(receiver.commit(&receiver.lease().unwrap()).unwrap().0);
        }
        tokio::time::advance(TIMEOUT).await;
        receiver.expire();
        assert!(!copy.is_current());
        assert!(receiver.connection_fence().load(Ordering::Acquire));
        assert!(
            receiver
                .enable_version(&offer().offer.runtime, wire::COPY_VERSION)
                .is_err()
        );
        assert!(receiver.copy_release().is_none());
        if let Some(pending) = pending {
            assert!(pending.receive().await.is_err());
        }
    }
}

#[tokio::test]
async fn focused_and_copy_reads_share_slots_and_retained_memory_limits() {
    let mut receiver = setup();
    let copy = committed(&mut receiver).await;
    let mut reads = Vec::new();
    for index in 0..wire::MAX_READS {
        reads.push(if index % 2 == 0 {
            receiver.read(&offer(), "image/png").unwrap().0
        } else {
            receiver.read_copy(&copy, "image/png").unwrap().0
        });
    }
    assert!(receiver.read_copy(&copy, "text/plain").is_err());
    assert!(receiver.read(&offer(), "text/plain").is_err());
    let large = reads.pop().unwrap();
    let chunk = vec![1; wire::CHUNK];
    for offset in (0..wire::MAX_BYTES).step_by(wire::CHUNK) {
        data(
            &mut receiver,
            large.id(),
            offset,
            &chunk,
            offset + chunk.len() == wire::MAX_BYTES,
        );
    }
    let held = large.receive().await.unwrap();
    let over = reads.pop().unwrap();
    data(&mut receiver, over.id(), 0, b"overflow", false);
    assert!(over.receive().await.is_err());
    assert_eq!(receiver.budget.load(Ordering::Acquire), wire::MAX_BYTES);
    assert_eq!(held.bytes().unwrap().len(), wire::MAX_BYTES);
    drop(held);
    assert_eq!(receiver.budget.load(Ordering::Acquire), 0);
    assert!(receiver.read_copy(&copy, "text/plain").is_ok());
}

#[tokio::test]
async fn disable_drains_late_copy_events_without_resurrection_or_id_reuse() {
    let mut receiver = setup();
    let old = committed(&mut receiver).await;
    let (commit, _) = receiver.commit(&receiver.lease().unwrap()).unwrap();
    let id = commit.id();
    receiver.begin_disable();
    assert!(!old.is_current());
    assert!(commit.receive().await.is_err());
    acknowledgment(&mut receiver, id, offer());
    assert!(receiver.committed.is_none());
    receiver.disable();
    receiver
        .enable_version(&offer().offer.runtime, wire::COPY_VERSION)
        .unwrap();
    accept(
        &mut receiver,
        Event::Offer {
            offer: Some(offer()),
        },
    )
    .unwrap();
    let (commit, _) = receiver.commit(&receiver.lease().unwrap()).unwrap();
    assert!(commit.id() > id);
}
