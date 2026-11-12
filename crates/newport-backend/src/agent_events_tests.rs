use super::*;
use newport_protocol::clipboard::exports::{self as wire, Event as ExportEvent};
use newport_protocol::clipboard::{Chunk, ExportOffer, Offer, Origin};

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
fn encoded(event: ExportEvent) -> Result<ServerMessage, super::super::Failure> {
    Ok(ServerMessage::Export(event))
}
fn acknowledgment() -> ServerMessage {
    ServerMessage::Response(newport_protocol::integration::Response {
        id: 1,
        reply: newport_protocol::integration::Reply::Acknowledged {
            message: String::new(),
        },
    })
}
fn request(id: u64) -> newport_protocol::clipboard::imports::Request {
    newport_protocol::clipboard::imports::Request {
        id,
        revision: 1,
        format: "text/plain".into(),
    }
}
#[tokio::test]
async fn clipboard_requests_and_gui_notices_survive_a_pending_ack() {
    let (send, input) = mpsc::channel(8);
    let mut events = Events::new(input);
    let producer = tokio::spawn(async move {
        for message in [
            ServerMessage::ClipboardRequest(request(1)),
            ServerMessage::GuiNotice,
            acknowledgment(),
        ] {
            send.send(Ok(message)).await.unwrap();
        }
    });
    let mut guard = events.request_guard().unwrap();
    loop {
        let event = events.receive().await.unwrap();
        if matches!(event, Event::Response(_)) {
            guard.complete();
            break;
        }
        events.defer(event).unwrap();
    }
    producer.await.unwrap();
    assert_eq!(
        events.next().await.unwrap(),
        Event::ClipboardRequest(request(1))
    );
    assert_eq!(events.next().await.unwrap(), Event::GuiNotice);
    assert!(events.deferred.is_empty());
}

#[tokio::test]
async fn copy_ack_and_revocation_are_applied_before_an_unrelated_request_ack() {
    let (send, input) = mpsc::channel(8);
    let mut events = Events::new(input);
    events.exports.enable(&offer().offer.runtime).unwrap();
    send.send(encoded(ExportEvent::Offer {
        offer: Some(offer()),
    }))
    .await
    .unwrap();
    events.receive().await.unwrap();
    let (commit, _) = events
        .exports
        .commit(&events.exports.lease().unwrap())
        .unwrap();
    let copy = newport_protocol::clipboard::CopyLease {
        id: uuid::Uuid::new_v4().to_string(),
        offer: offer(),
    };
    send.send(encoded(ExportEvent::Committed {
        id: commit.id(),
        copy: Some(copy.clone()),
    }))
    .await
    .unwrap();
    let event = events.receive().await.unwrap();
    events.defer(event).unwrap();
    let accepted = commit.receive().await.unwrap();
    assert!(accepted.is_current());
    let (read, _) = events.exports.read_copy(&accepted, "text/plain").unwrap();
    for event in [
        ExportEvent::Data {
            id: read.id(),
            offset: 0,
            chunk: Some(Chunk {
                bytes: b"copy".into(),
                eof: true,
            }),
        },
        ExportEvent::Offer { offer: None },
    ] {
        send.send(encoded(event)).await.unwrap();
        let event = events.receive().await.unwrap();
        events.defer(event).unwrap();
    }
    let held = read.receive().await.unwrap();
    assert_eq!(held.bytes().unwrap(), b"copy");
    send.send(encoded(ExportEvent::CopyRevoked { id: copy.id }))
        .await
        .unwrap();
    let event = events.receive().await.unwrap();
    events.defer(event).unwrap();
    send.send(Ok(acknowledgment())).await.unwrap();
    assert!(matches!(
        events.receive().await.unwrap(),
        Event::Response(_)
    ));
    assert_eq!(events.deferred.len(), 1);
    assert_eq!(events.deferred.front(), Some(&Event::ExportsChanged));
    assert!(!accepted.is_current());
    assert!(held.bytes().is_err());
}

#[tokio::test]
async fn abandoned_transport_request_fences_copies_before_the_next_network_poll() {
    let (send, input) = mpsc::channel(8);
    let mut events = Events::new(input);
    events.exports.enable(&offer().offer.runtime).unwrap();
    send.send(encoded(ExportEvent::Offer {
        offer: Some(offer()),
    }))
    .await
    .unwrap();
    events.receive().await.unwrap();
    let (commit, _) = events
        .exports
        .commit(&events.exports.lease().unwrap())
        .unwrap();
    send.send(encoded(ExportEvent::Committed {
        id: commit.id(),
        copy: Some(newport_protocol::clipboard::CopyLease {
            id: uuid::Uuid::new_v4().to_string(),
            offer: offer(),
        }),
    }))
    .await
    .unwrap();
    events.receive().await.unwrap();
    let copy = commit.receive().await.unwrap();
    let (read, _) = events.exports.read_copy(&copy, "text/plain").unwrap();
    send.send(encoded(ExportEvent::Data {
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
    use newport_protocol::integration::wire::Message;
    use tokio::io::AsyncWriteExt;
    async fn write(output: &mut tokio::io::DuplexStream, message: ServerMessage) {
        output.write_all(&message.encode().unwrap()).await.unwrap();
    }
    let (mut send, input) = tokio::io::duplex(1024);
    let (mut events, reader) = Events::start(input);
    let offer = offer();
    events.exports.enable(&offer.offer.runtime).unwrap();
    write(
        &mut send,
        ServerMessage::Export(ExportEvent::Offer {
            offer: Some(offer.clone()),
        }),
    )
    .await;
    let event = events.receive().await.unwrap();
    events.defer(event).unwrap();
    let (read, _) = events.exports.read(&offer, "text/plain").unwrap();
    let producer = tokio::spawn(async move {
        for index in 0..32 {
            write(
                &mut send,
                ServerMessage::Export(ExportEvent::Data {
                    id: 1,
                    offset: index * wire::CHUNK,
                    chunk: Some(Chunk {
                        bytes: vec![index as u8; wire::CHUNK],
                        eof: index == 31,
                    }),
                }),
            )
            .await;
        }
        // Eight native host requests can coexist with one export notification.
        for id in 1..=8 {
            write(&mut send, ServerMessage::ClipboardRequest(request(id))).await;
        }
        write(&mut send, acknowledgment()).await;
        send
    });
    loop {
        let event = events.receive().await.unwrap();
        if matches!(event, Event::Response(_)) {
            break;
        }
        events.defer(event).unwrap();
        assert!(events.deferred.len() <= 9);
        assert_eq!(
            events
                .deferred
                .iter()
                .filter(|event| matches!(event, Event::ExportsChanged))
                .count(),
            1
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
    assert_eq!(events.next().await.unwrap(), Event::ExportsChanged);
    for _ in 0..8 {
        assert!(matches!(
            events.next().await.unwrap(),
            Event::ClipboardRequest(_)
        ));
    }
    drop(send);
    assert!(
        events
            .next()
            .await
            .unwrap_err()
            .to_string()
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
    send.send(encoded(ExportEvent::Offer {
        offer: Some(offer.clone()),
    }))
    .await
    .unwrap();
    events.receive().await.unwrap();
    let (read, _) = events.exports.read(&offer, "text/plain").unwrap();
    for event in [
        ExportEvent::Data {
            id: 1,
            offset: 0,
            chunk: Some(Chunk {
                bytes: b"old".to_vec(),
                eof: true,
            }),
        },
        ExportEvent::Offer { offer: None },
    ] {
        send.send(encoded(event)).await.unwrap();
        let event = events.receive().await.unwrap();
        events.defer(event).unwrap();
    }
    send.send(Ok(acknowledgment())).await.unwrap();
    assert!(matches!(
        events.receive().await.unwrap(),
        Event::Response(_)
    ));
    assert_eq!(events.deferred.len(), 1);
    assert!(events.exports.offer().is_none());
    assert!(read.receive().await.unwrap().bytes().is_err());
}

#[tokio::test]
async fn unnegotiated_events_and_excess_deferred_requests_fail_closed() {
    let (send, input) = mpsc::channel(8);
    let mut events = Events::new(input);
    send.send(encoded(ExportEvent::Offer { offer: None }))
        .await
        .unwrap();
    assert!(
        events
            .receive()
            .await
            .unwrap_err()
            .to_string()
            .contains("Unnegotiated")
    );
    assert!(events.request_guard().is_err());
    let (send, input) = mpsc::channel(8);
    let mut events = Events::new(input);
    let offer = offer();
    events.exports.enable(&offer.offer.runtime).unwrap();
    send.send(encoded(ExportEvent::Offer {
        offer: Some(offer.clone()),
    }))
    .await
    .unwrap();
    events.receive().await.unwrap();
    let (waiting, _) = events.exports.read(&offer, "text/plain").unwrap();
    for _ in 0..100 {
        events.defer(Event::GuiNotice).unwrap();
    }
    for _ in 0..8 {
        events.defer(Event::ClipboardRequest(request(1))).unwrap();
    }
    assert_eq!(events.deferred.len(), 9);
    assert!(events.defer(Event::ClipboardRequest(request(1))).is_err());
    assert!(waiting.receive().await.is_err());
}

#[tokio::test(start_paused = true)]
async fn cancelled_ack_wait_invalidates_results_and_cannot_consume_the_next_requests_ack() {
    let (send, input) = mpsc::channel(8);
    let mut events = Events::new(input);
    let offer = offer();
    events.exports.enable(&offer.offer.runtime).unwrap();
    send.send(encoded(ExportEvent::Offer {
        offer: Some(offer.clone()),
    }))
    .await
    .unwrap();
    events.receive().await.unwrap();
    let (read, _) = events.exports.read(&offer, "text/plain").unwrap();
    send.send(encoded(ExportEvent::Data {
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
    send.send(Ok(acknowledgment())).await.unwrap();
    assert!(events.request_guard().is_err());
    assert!(
        events
            .next()
            .await
            .unwrap_err()
            .to_string()
            .starts_with("SSH transport interrupted:")
    );
    assert!(events.exports.enable(&offer.offer.runtime).is_err());
}
