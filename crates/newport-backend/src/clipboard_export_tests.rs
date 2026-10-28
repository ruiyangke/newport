use super::*;
use crate::agent_runtime::protocol::clipboard::{Chunk, Offer, Origin};

fn offer(revision: i64, route_revision: u64) -> ExportOffer {
    ExportOffer {
        offer: Offer {
            runtime: uuid::Uuid::nil().to_string(),
            revision,
            origin: Origin::Native {
                worker: "display".into(),
                revision: revision as u64,
            },
            formats: vec!["text/plain".into(), "image/png".into()],
        },
        route_revision,
    }
}
fn accept(receiver: &mut Receiver, event: Event) -> Result<(), String> {
    receiver.accept(&event.encode().unwrap())
}
fn setup() -> (Receiver, ExportOffer) {
    let offer = offer(1, 1);
    let mut receiver = Receiver::default();
    receiver.enable(&offer.offer.runtime).unwrap();
    accept(
        &mut receiver,
        Event::Offer {
            offer: Some(offer.clone()),
        },
    )
    .unwrap();
    (receiver, offer)
}
fn data(
    receiver: &mut Receiver,
    id: u64,
    offset: usize,
    bytes: &[u8],
    eof: bool,
) -> Result<(), String> {
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
}

#[tokio::test]
async fn version_one_rejects_unnegotiated_copy_events_and_revokes_pending_reads() {
    for event in [
        Event::Committed { id: 1, copy: None },
        Event::CopyRevoked {
            id: uuid::Uuid::new_v4().to_string(),
        },
    ] {
        let (mut receiver, offer) = setup();
        let lease = receiver.lease().unwrap();
        let (pending, _) = receiver.read(&offer, "text/plain").unwrap();
        assert!(accept(&mut receiver, event).is_err());
        assert!(!lease.is_current());
        assert!(pending.receive().await.is_err());
        assert!(receiver.enable(&offer.offer.runtime).is_err());
    }
}

#[tokio::test]
async fn demand_is_explicit_and_results_keep_the_source_lease() {
    let (mut receiver, offer) = setup();
    assert!(receiver.pending.is_empty());
    assert_eq!(receiver.budget.load(Ordering::Acquire), 0);
    let (read, command) = receiver.read(&offer, "text/plain").unwrap();
    assert!(matches!(command, Control::Read { id: 1, .. }));
    data(&mut receiver, read.id(), 0, b"hello ", false).unwrap();
    data(&mut receiver, read.id(), 6, b"world", true).unwrap();
    let representation = read.receive().await.unwrap();
    assert_eq!(representation.bytes().unwrap(), b"hello world");
    assert_eq!(representation.format(), "text/plain");
    assert_eq!(representation.offer(), &offer);
    accept(
        &mut receiver,
        Event::Offer {
            offer: Some(offer.clone()),
        },
    )
    .unwrap();
    assert!(
        representation.bytes().is_ok(),
        "duplicate metadata revoked a current result"
    );
    let (waiting, _) = receiver.read(&offer, "image/png").unwrap();
    accept(&mut receiver, Event::Offer { offer: None }).unwrap();
    assert!(waiting.receive().await.is_err());
    assert!(representation.bytes().is_err());
    assert_eq!(
        receiver.budget.load(Ordering::Acquire),
        11,
        "returned data lost its memory charge"
    );
    drop(representation);
    assert_eq!(receiver.budget.load(Ordering::Acquire), 0);
}

#[tokio::test]
async fn concurrent_and_completed_results_share_a_fixed_slot_budget() {
    let (mut receiver, offer) = setup();
    let mut reads = Vec::new();
    for id in 1..=wire::MAX_READS as u64 {
        let (read, _) = receiver.read(&offer, "text/plain").unwrap();
        assert_eq!(read.id(), id);
        reads.push(read);
    }
    assert!(receiver.read(&offer, "image/png").is_err());
    let mut results = Vec::new();
    for read in reads.into_iter().rev() {
        data(&mut receiver, read.id(), 0, b"", true).unwrap();
        results.push(read.receive().await.unwrap());
    }
    assert!(
        receiver.read(&offer, "text/plain").is_err(),
        "held results bypassed the slot bound"
    );
    results.pop();
    let (read, _) = receiver.read(&offer, "text/plain").unwrap();
    assert_eq!(read.id(), 9);
    drop(read);
    receiver.expire();
    assert_eq!(receiver.cancellations(), [9]);
    assert!(
        receiver.read(&offer, "text/plain").is_err(),
        "unacknowledged cancellation freed a remote slot"
    );
    receiver.cancelled(9);
    assert!(receiver.read(&offer, "text/plain").is_ok());
}

#[tokio::test]
async fn aggregate_memory_includes_completed_and_partial_representations() {
    let (mut receiver, offer) = setup();
    let (large, _) = receiver.read(&offer, "image/png").unwrap();
    let (overflow, _) = receiver.read(&offer, "text/plain").unwrap();
    let bytes = vec![7; wire::CHUNK];
    for offset in (0..wire::MAX_BYTES).step_by(wire::CHUNK) {
        data(
            &mut receiver,
            large.id(),
            offset,
            &bytes,
            offset + bytes.len() == wire::MAX_BYTES,
        )
        .unwrap();
    }
    let large = large.receive().await.unwrap();
    assert_eq!(large.bytes().unwrap().len(), wire::MAX_BYTES);
    assert!(receiver.read(&offer, "text/plain").is_err());
    data(&mut receiver, overflow.id(), 0, b"over budget", false).unwrap();
    assert!(
        overflow
            .receive()
            .await
            .unwrap_err()
            .contains("memory limit")
    );
    assert_eq!(receiver.cancellations(), [2]);
    assert_eq!(receiver.budget.load(Ordering::Acquire), wire::MAX_BYTES);
    // Late bytes for the cancelled read cannot allocate another buffer.
    data(&mut receiver, 2, 99, b"late", true).unwrap();
    drop(large);
    assert_eq!(receiver.budget.load(Ordering::Acquire), 0);
    receiver.cancelled(2);
    assert!(receiver.read(&offer, "image/png").is_ok());
}

#[tokio::test(start_paused = true)]
async fn abandonment_timeout_and_explicit_cancel_release_bytes_without_id_reuse() {
    let (mut receiver, offer) = setup();
    let (abandoned, _) = receiver.read(&offer, "text/plain").unwrap();
    let abandoned_id = abandoned.id();
    data(&mut receiver, abandoned_id, 0, b"partial", false).unwrap();
    drop(abandoned);
    receiver.expire();
    assert_eq!(receiver.cancellations(), [abandoned_id]);
    assert_eq!(receiver.budget.load(Ordering::Acquire), 0);
    data(&mut receiver, abandoned_id, 7, b"discarded", true).unwrap();
    receiver.cancelled(abandoned_id);
    let (timed_out, _) = receiver.read(&offer, "text/plain").unwrap();
    tokio::time::advance(TIMEOUT).await;
    receiver.expire();
    assert!(timed_out.receive().await.is_err());
    let (cancelled, _) = receiver.read(&offer, "text/plain").unwrap();
    receiver.cancel(cancelled.id());
    assert!(cancelled.receive().await.is_err());
    receiver.disable();
    receiver.enable(&offer.offer.runtime).unwrap();
    accept(
        &mut receiver,
        Event::Offer {
            offer: Some(offer.clone()),
        },
    )
    .unwrap();
    let (read, _) = receiver.read(&offer, "text/plain").unwrap();
    assert_eq!(read.id(), 4);
}

#[tokio::test]
async fn failure_never_returns_a_partial_representation_and_disable_is_immediate() {
    let (mut receiver, offer) = setup();
    let (failed, _) = receiver.read(&offer, "image/png").unwrap();
    data(&mut receiver, failed.id(), 0, b"partial", false).unwrap();
    accept(
        &mut receiver,
        Event::Data {
            id: failed.id(),
            offset: 7,
            chunk: None,
        },
    )
    .unwrap();
    assert!(failed.receive().await.is_err());
    assert_eq!(receiver.budget.load(Ordering::Acquire), 0);
    let (finished, _) = receiver.read(&offer, "text/plain").unwrap();
    data(&mut receiver, finished.id(), 0, b"finished", true).unwrap();
    let finished = finished.receive().await.unwrap();
    let (pending, _) = receiver.read(&offer, "image/png").unwrap();
    let id = pending.id();
    receiver.begin_disable();
    assert!(finished.bytes().is_err());
    assert!(pending.receive().await.is_err());
    // Drain only: old in-flight events precede the disable ACK on this stream.
    data(&mut receiver, id, 0, b"late bytes", true).unwrap();
    accept(
        &mut receiver,
        Event::Offer {
            offer: Some(offer.clone()),
        },
    )
    .unwrap();
    assert!(receiver.offer().is_none());
    assert!(receiver.read(&offer, "text/plain").is_err());
    receiver.disable();
    assert!(data(&mut receiver, id, 0, b"after ack", true).is_err());
}

#[tokio::test]
async fn protocol_errors_and_disconnect_revoke_even_already_delivered_results() {
    for error in 0..5 {
        let (mut receiver, offer) = setup();
        let (read, _) = receiver.read(&offer, "text/plain").unwrap();
        data(&mut receiver, read.id(), 0, b"completed", true).unwrap();
        let data_read = read.receive().await.unwrap();
        let (waiting, _) = receiver.read(&offer, "text/plain").unwrap();
        let result = match error {
            0 => data(&mut receiver, waiting.id(), 5, b"wrong offset", true),
            1 => data(&mut receiver, 1, 0, b"duplicate result", true),
            2 => data(&mut receiver, 99, 0, b"unrequested", true),
            3 => {
                let mut other = offer.clone();
                other.offer.runtime = uuid::Uuid::new_v4().to_string();
                accept(&mut receiver, Event::Offer { offer: Some(other) })
            }
            _ => receiver.accept(&[0xff]),
        };
        assert!(result.is_err());
        assert!(!receiver.enabled());
        assert!(waiting.receive().await.is_err());
        assert!(data_read.bytes().is_err());
    }
    let (mut receiver, offer) = setup();
    let (read, _) = receiver.read(&offer, "text/plain").unwrap();
    drop(receiver);
    assert!(read.receive().await.is_err());
}

#[test]
fn source_and_route_revisions_cannot_regress_or_reappear_after_withdrawal() {
    let (mut receiver, original) = setup();
    accept(
        &mut receiver,
        Event::Offer {
            offer: Some(offer(2, 2)),
        },
    )
    .unwrap();
    assert!(
        accept(
            &mut receiver,
            Event::Offer {
                offer: Some(original)
            }
        )
        .is_err()
    );
    let (mut receiver, original) = setup();
    accept(&mut receiver, Event::Offer { offer: None }).unwrap();
    assert!(
        accept(
            &mut receiver,
            Event::Offer {
                offer: Some(original)
            }
        )
        .is_err()
    );
    let (mut receiver, mut changed) = setup();
    changed.offer.formats.pop();
    assert!(
        accept(
            &mut receiver,
            Event::Offer {
                offer: Some(changed)
            }
        )
        .is_err()
    );
    let (mut receiver, _) = setup();
    accept(&mut receiver, Event::Offer { offer: None }).unwrap();
    accept(
        &mut receiver,
        Event::Offer {
            offer: Some(offer(1, 2)),
        },
    )
    .unwrap();
    assert!(receiver.enable(&uuid::Uuid::new_v4().to_string()).is_err());
}
