use super::*;
use crate::agent_runtime::protocol::{
    clipboard::{Chunk, ExportOffer, Offer, Origin},
    clipboard_export::{self as wire, Event},
};

fn offer() -> ExportOffer {
    ExportOffer {
        offer: Offer {
            runtime: uuid::Uuid::nil().to_string(),
            revision: 1,
            origin: Origin::Native {
                worker: "native".into(),
                revision: 1,
            },
            formats: vec!["text/plain".into()],
        },
        route_revision: 1,
    }
}
fn encoded(event: Event) -> Result<(u8, Vec<u8>), String> {
    Ok((FRAME, event.encode().unwrap()))
}

#[tokio::test]
async fn shared_launch_and_attachment_check_survive_an_unrelated_pending_ack() {
    let (send, input) = mpsc::channel(8);
    let mut events = Events::new(input);
    let producer = tokio::spawn(async move {
        for kind in *b"laA" {
            send.send(Ok((kind, vec![kind]))).await.unwrap();
        }
    });
    let mut guard = events.request_guard().unwrap();
    loop {
        let (kind, data) = events.receive().await.unwrap();
        if kind == b'A' {
            guard.complete();
            break;
        }
        events.defer(kind, data).unwrap();
    }
    producer.await.unwrap();
    assert_eq!(events.next().await.unwrap(), (b'l', vec![b'l']));
    assert_eq!(events.next().await.unwrap(), (b'a', vec![b'a']));
    assert!(events.deferred.is_empty());
    for index in 0..64 {
        events
            .defer(if index % 2 == 0 { b'l' } else { b'a' }, vec![])
            .unwrap();
    }
    assert!(events.defer(b'a', vec![]).is_err());
    assert!(events.request_guard().is_err());
}

#[tokio::test]
async fn copy_ack_and_revocation_are_applied_before_an_unrelated_request_ack() {
    let (send, input) = mpsc::channel(8);
    let mut events = Events::new(input);
    events
        .exports
        .enable_version(&offer().offer.runtime, wire::COPY_VERSION)
        .unwrap();
    send.send(encoded(Event::Offer {
        offer: Some(offer()),
    }))
    .await
    .unwrap();
    events.receive().await.unwrap();
    let (commit, _) = events
        .exports
        .commit(&events.exports.lease().unwrap())
        .unwrap();
    let copy = crate::agent_runtime::protocol::clipboard::CopyLease {
        id: uuid::Uuid::new_v4().to_string(),
        offer: offer(),
    };
    send.send(encoded(Event::Committed {
        id: commit.id(),
        copy: Some(copy.clone()),
    }))
    .await
    .unwrap();
    let (kind, data) = events.receive().await.unwrap();
    events.defer(kind, data).unwrap();
    let accepted = commit.receive().await.unwrap();
    assert!(accepted.is_current());
    let (read, _) = events.exports.read_copy(&accepted, "text/plain").unwrap();
    for event in [
        Event::Data {
            id: read.id(),
            offset: 0,
            chunk: Some(Chunk {
                bytes: b"copy".into(),
                eof: true,
            }),
        },
        Event::Offer { offer: None },
    ] {
        send.send(encoded(event)).await.unwrap();
        let (kind, data) = events.receive().await.unwrap();
        events.defer(kind, data).unwrap();
    }
    let held = read.receive().await.unwrap();
    assert_eq!(held.bytes().unwrap(), b"copy");
    send.send(encoded(Event::CopyRevoked { id: copy.id }))
        .await
        .unwrap();
    let (kind, data) = events.receive().await.unwrap();
    events.defer(kind, data).unwrap();
    send.send(Ok((b'A', vec![]))).await.unwrap();
    assert_eq!(events.receive().await.unwrap().0, b'A');
    assert_eq!(events.deferred.len(), 1);
    assert!(events.deferred.front().unwrap().1.is_empty());
    assert!(!accepted.is_current());
    assert!(held.bytes().is_err());
}

#[tokio::test]
async fn abandoned_transport_request_fences_copies_before_the_next_network_poll() {
    let (send, input) = mpsc::channel(8);
    let mut events = Events::new(input);
    events
        .exports
        .enable_version(&offer().offer.runtime, wire::COPY_VERSION)
        .unwrap();
    send.send(encoded(Event::Offer {
        offer: Some(offer()),
    }))
    .await
    .unwrap();
    events.receive().await.unwrap();
    let (commit, _) = events
        .exports
        .commit(&events.exports.lease().unwrap())
        .unwrap();
    send.send(encoded(Event::Committed {
        id: commit.id(),
        copy: Some(crate::agent_runtime::protocol::clipboard::CopyLease {
            id: uuid::Uuid::new_v4().to_string(),
            offer: offer(),
        }),
    }))
    .await
    .unwrap();
    events.receive().await.unwrap();
    let copy = commit.receive().await.unwrap();
    let (read, _) = events.exports.read_copy(&copy, "text/plain").unwrap();
    send.send(encoded(Event::Data {
        id: read.id(),
        offset: 0,
        chunk: Some(Chunk {
            bytes: b"copy".into(),
            eof: true,
        }),
    }))
    .await
    .unwrap();
    events.receive().await.unwrap();
    let held = read.receive().await.unwrap();
    drop(events.request_guard().unwrap());
    assert!(!copy.is_current());
    assert!(held.bytes().is_err());
    assert!(events.exports.read_copy(&copy, "text/plain").is_err());
    assert!(events.request_guard().is_err());
}

#[tokio::test]
async fn payloads_before_an_ack_do_not_accumulate_in_deferred_events() {
    use tokio::io::AsyncWriteExt;
    async fn write(output: &mut tokio::io::DuplexStream, kind: u8, data: &[u8]) {
        output
            .write_all(&crate::agent::wire::encode(kind, data).unwrap())
            .await
            .unwrap();
    }
    let (mut send, input) = tokio::io::duplex(1024);
    let (mut events, reader) = Events::start(input);
    let offer = offer();
    events.exports.enable(&offer.offer.runtime).unwrap();
    write(
        &mut send,
        FRAME,
        &Event::Offer {
            offer: Some(offer.clone()),
        }
        .encode()
        .unwrap(),
    )
    .await;
    let (kind, data) = events.receive().await.unwrap();
    events.defer(kind, data).unwrap();
    let (read, _) = events.exports.read(&offer, "text/plain").unwrap();
    let producer = tokio::spawn(async move {
        for index in 0..32 {
            write(
                &mut send,
                FRAME,
                &Event::Data {
                    id: 1,
                    offset: index * wire::CHUNK,
                    chunk: Some(Chunk {
                        bytes: vec![index as u8; wire::CHUNK],
                        eof: index == 31,
                    }),
                }
                .encode()
                .unwrap(),
            )
            .await;
        }
        // Eight native host requests can coexist with one export notification.
        for _ in 0..8 {
            write(&mut send, b'C', &[1]).await;
        }
        write(&mut send, b'A', &[]).await;
        send
    });
    loop {
        let (kind, data) = events.receive().await.unwrap();
        if kind == b'A' {
            break;
        }
        events.defer(kind, data).unwrap();
        assert!(events.deferred.len() <= 9);
        assert!(
            events
                .deferred
                .iter()
                .filter(|(kind, _)| *kind == FRAME)
                .all(|(_, data)| data.is_empty())
        );
    }
    let send = producer.await.unwrap();
    let representation = read.receive().await.unwrap();
    for (index, chunk) in representation
        .bytes()
        .unwrap()
        .chunks(wire::CHUNK)
        .enumerate()
    {
        assert!(chunk.iter().all(|byte| *byte == index as u8));
    }
    assert_eq!(events.next().await.unwrap(), (FRAME, vec![]));
    for _ in 0..8 {
        assert_eq!(events.next().await.unwrap().0, b'C');
    }
    drop(send);
    assert!(
        events
            .next()
            .await
            .unwrap_err()
            .starts_with("SSH transport interrupted:")
    );
    assert!(representation.bytes().is_err());
    reader.await.unwrap();
}

#[tokio::test]
async fn withdrawal_before_ack_invalidates_a_completed_result_and_coalesces_notification() {
    let (send, input) = mpsc::channel(8);
    let mut events = Events::new(input);
    let offer = offer();
    events.exports.enable(&offer.offer.runtime).unwrap();
    send.send(encoded(Event::Offer {
        offer: Some(offer.clone()),
    }))
    .await
    .unwrap();
    events.receive().await.unwrap();
    let (read, _) = events.exports.read(&offer, "text/plain").unwrap();
    for event in [
        Event::Data {
            id: 1,
            offset: 0,
            chunk: Some(Chunk {
                bytes: b"old".to_vec(),
                eof: true,
            }),
        },
        Event::Offer { offer: None },
    ] {
        send.send(encoded(event)).await.unwrap();
        let (kind, data) = events.receive().await.unwrap();
        events.defer(kind, data).unwrap();
    }
    send.send(Ok((b'A', vec![]))).await.unwrap();
    assert_eq!(events.receive().await.unwrap().0, b'A');
    assert_eq!(events.deferred.len(), 1);
    assert!(events.exports.offer().is_none());
    assert!(read.receive().await.unwrap().bytes().is_err());
}

#[tokio::test]
async fn unnegotiated_events_and_excess_deferred_requests_fail_closed() {
    let (send, input) = mpsc::channel(8);
    let mut events = Events::new(input);
    send.send(encoded(Event::Offer { offer: None }))
        .await
        .unwrap();
    assert!(events.receive().await.unwrap_err().contains("Unnegotiated"));
    assert!(events.request_guard().is_err());
    let (send, input) = mpsc::channel(8);
    let mut events = Events::new(input);
    let offer = offer();
    events.exports.enable(&offer.offer.runtime).unwrap();
    send.send(encoded(Event::Offer {
        offer: Some(offer.clone()),
    }))
    .await
    .unwrap();
    events.receive().await.unwrap();
    let (waiting, _) = events.exports.read(&offer, "text/plain").unwrap();
    for _ in 0..100 {
        events.defer(b'G', vec![]).unwrap();
    }
    for _ in 0..64 {
        events.defer(b'L', vec![0]).unwrap();
    }
    for _ in 0..8 {
        events.defer(b'C', vec![0]).unwrap();
    }
    assert_eq!(events.deferred.len(), 73);
    assert!(events.defer(b'C', vec![0]).is_err());
    assert!(waiting.receive().await.is_err());
}

#[tokio::test(start_paused = true)]
async fn cancelled_ack_wait_invalidates_results_and_cannot_consume_the_next_requests_ack() {
    let (send, input) = mpsc::channel(8);
    let mut events = Events::new(input);
    let offer = offer();
    events.exports.enable(&offer.offer.runtime).unwrap();
    send.send(encoded(Event::Offer {
        offer: Some(offer.clone()),
    }))
    .await
    .unwrap();
    events.receive().await.unwrap();
    let (read, _) = events.exports.read(&offer, "text/plain").unwrap();
    send.send(encoded(Event::Data {
        id: read.id(),
        offset: 0,
        chunk: Some(Chunk {
            bytes: b"received".into(),
            eof: true,
        }),
    }))
    .await
    .unwrap();
    events.receive().await.unwrap();
    let representation = read.receive().await.unwrap();
    let mut successful = events.request_guard().unwrap();
    successful.complete();
    drop(successful);
    assert!(representation.bytes().is_ok());
    let guard = events.request_guard().unwrap();
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(1), async move {
            let _guard = guard;
            std::future::pending::<()>().await;
        })
        .await
        .is_err()
    );
    // Invalidate before another network poll, including already delivered data.
    assert!(representation.bytes().is_err());
    assert!(events.exports.offer().is_none());
    send.send(Ok((b'A', vec![]))).await.unwrap();
    assert!(events.request_guard().is_err());
    assert!(
        events
            .next()
            .await
            .unwrap_err()
            .starts_with("SSH transport interrupted:")
    );
    assert!(events.exports.enable(&offer.offer.runtime).is_err());
}
