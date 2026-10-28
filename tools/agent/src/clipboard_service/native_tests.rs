use super::*;

#[path = "copy_tests.rs"]
mod copies;

fn provider(service: &Service, worker: &str) -> NativePublisher {
    NativePublisher::new(service.authority().grant(worker, "display").unwrap()).unwrap()
}
fn route(
    service: &Service,
    revision: u64,
    worker: Option<&str>,
    exports: bool,
    focused: bool,
) -> io::Result<()> {
    service.authority().configure_route(Route {
        revision,
        worker_id: worker.map(str::to_owned),
        exports,
        focused,
    })
}
fn native_request(provider: &NativePublisher) -> NativeRequest {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let poll = provider.poll().unwrap();
        if !poll.requests.is_empty() {
            assert_eq!(poll.requests.len(), 1);
            return poll.requests.into_iter().next().unwrap();
        }
        assert!(
            Instant::now() < deadline,
            "native provider received no demand"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn install(provider: &NativePublisher, revision: u64) -> Offer {
    let poll = provider.poll().unwrap();
    assert!(
        provider
            .publish(
                &poll.lease.unwrap(),
                &poll.offer,
                revision,
                vec!["image/png".into(), "text/plain".into()]
            )
            .unwrap()
    );
    provider.poll().unwrap().offer
}
fn export_read(reader: Reader, offer: ExportOffer) -> mpsc::Receiver<io::Result<Chunk>> {
    let (send, receive) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let result = reader
            .read_export(&offer, "text/plain", ReadCancellation::default())
            .and_then(|mut read| read.next(4096));
        let _ = send.send(result);
    });
    receive
}

#[test]
fn native_assignment_is_explicit_and_route_revisions_cannot_be_reused() {
    let (_directory, service, producer, reader, _) = fixture();
    let native = provider(&service, "native");
    assert!(native.poll().unwrap().lease.is_none());
    assert!(route(&service, 1, Some("missing"), true, true).is_err());
    assert!(route(&service, 1, Some("host"), true, true).is_err());
    assert!(route(&service, 1, None, true, false).is_err());
    route(&service, 1, Some("native"), false, false).unwrap();
    let lease = native.poll().unwrap().lease.unwrap();
    route(&service, 1, Some("native"), false, false).unwrap();
    assert_eq!(native.poll().unwrap().lease.unwrap(), lease);
    assert!(route(&service, 1, Some("native"), true, true).is_err());
    assert!(route(&service, 0, Some("native"), false, false).is_err());
    let offer = install(&native, 1);
    assert_eq!(
        offer.origin,
        Origin::Native {
            worker: "native".into(),
            revision: 1
        }
    );
    assert!(producer.reader().export_offer().unwrap().is_none());
    assert!(reader.export_offer().is_err());
    assert!(
        native.poll().unwrap().requests.is_empty(),
        "publication fetched a payload"
    );
}

#[test]
fn native_local_reads_share_one_fetch_without_host_export_or_import_permission() {
    let (_directory, service, producer, reader, _) = fixture();
    publish(&producer, 9);
    let native = provider(&service, "native");
    route(&service, 1, Some("native"), false, false).unwrap();
    let offer = install(&native, 1);
    publish(&producer, 9); // Repeated host metadata must not replace native copy.
    assert_eq!(reader.offer().unwrap(), offer);
    let first = fetch(reader.clone(), offer.clone(), "image/png");
    let second = fetch(reader.clone(), offer.clone(), "image/png");
    let mut output = vec![];
    producer.poll(&mut output).unwrap();
    assert!(output.is_empty(), "host producer consumed a native request");
    let request = native_request(&native);
    producer.enabled(false).unwrap();
    assert_eq!(
        reader.offer().unwrap(),
        offer,
        "import revocation erased a local native selection"
    );
    let bytes: Vec<_> = (0..150_000).map(|index| (index % 251) as u8).collect();
    let chunks: Vec<_> = bytes.chunks(clipboard_wire::CHUNK).collect();
    for (index, chunk) in chunks.iter().enumerate() {
        native
            .reply(&request, chunk, index + 1 == chunks.len())
            .unwrap();
    }
    assert_eq!(
        first.recv_timeout(Duration::from_secs(2)).unwrap().unwrap(),
        bytes
    );
    assert_eq!(
        second
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap(),
        bytes
    );
    assert_eq!(reader.get(offer.revision, "image/png").unwrap(), bytes);
    assert!(native.poll().unwrap().requests.is_empty());
    assert!(producer.reader().export_offer().unwrap().is_none());
}

#[test]
fn stale_native_copies_and_replies_cannot_resurrect_after_a_new_host_offer() {
    let (_directory, service, producer, reader, _) = fixture();
    let native = provider(&service, "native");
    route(&service, 1, Some("native"), false, false).unwrap();
    let poll = native.poll().unwrap();
    let lease = poll.lease.unwrap();
    publish(&producer, 8);
    assert!(
        !native
            .publish(&lease, &poll.offer, 1, vec!["text/plain".into()])
            .unwrap()
    );
    assert!(
        !native
            .publish(
                &lease,
                &reader.offer().unwrap(),
                1,
                vec!["text/plain".into()]
            )
            .unwrap()
    );
    let offer = install(&native, 2);
    let waiting = fetch(reader.clone(), offer.clone(), "text/plain");
    let request = native_request(&native);
    publish(&producer, 9);
    assert!(
        waiting
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .is_err()
    );
    native.reply(&request, b"superseded", true).unwrap();
    assert_eq!(reader.offer().unwrap().origin, Origin::Host);
    assert!(
        !native
            .publish(&lease, &reader.offer().unwrap(), 2, offer.formats)
            .unwrap()
    );
    assert!(
        native
            .publish(
                &lease,
                &reader.offer().unwrap(),
                2,
                vec!["text/plain".into()]
            )
            .is_err()
    );
    assert!(
        native
            .publish(
                &lease,
                &reader.offer().unwrap(),
                3,
                vec!["text/plain".into()]
            )
            .unwrap()
    );
}

#[test]
fn export_permission_and_focus_leases_cancel_only_host_reads() {
    let (_directory, service, producer, reader, _) = fixture();
    let native = provider(&service, "native");
    route(&service, 1, Some("native"), false, true).unwrap();
    let offer = install(&native, 1);
    let host = producer.reader();
    assert!(host.export_offer().unwrap().is_none());
    route(&service, 2, Some("native"), true, false).unwrap();
    assert!(host.export_offer().unwrap().is_none());
    route(&service, 3, Some("native"), true, true).unwrap();
    let lease = host.export_offer().unwrap().unwrap();
    let exported = export_read(host.clone(), lease.clone());
    let request = native_request(&native);
    let local = fetch(reader.clone(), offer.clone(), "text/plain");
    route(&service, 4, Some("native"), true, false).unwrap();
    assert!(
        exported
            .recv_timeout(Duration::from_millis(500))
            .unwrap()
            .is_err()
    );
    assert!(host.export_offer().unwrap().is_none());
    assert_eq!(reader.offer().unwrap(), offer);
    native.reply(&request, b"local copy", true).unwrap();
    assert_eq!(
        local.recv_timeout(Duration::from_secs(1)).unwrap().unwrap(),
        b"local copy"
    );
    route(&service, 5, Some("native"), true, true).unwrap();
    assert!(
        export_read(host.clone(), lease)
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .is_err()
    );
    let fresh = host.export_offer().unwrap().unwrap();
    assert_eq!(
        export_read(host.clone(), fresh.clone())
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap()
            .bytes,
        b"local copy"
    );
    route(&service, 6, Some("native"), false, true).unwrap();
    assert!(
        export_read(host, fresh)
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .is_err()
    );
    assert!(
        native.poll().unwrap().requests.is_empty(),
        "cached exports fetched data again"
    );
}

#[test]
fn provider_rebinding_fences_delayed_messages_even_when_native_counters_repeat() {
    let (_directory, service, _producer, reader, _) = fixture();
    let native = provider(&service, "native");
    let other = provider(&service, "other");
    route(&service, 1, Some("native"), false, false).unwrap();
    let old_lease = native.poll().unwrap().lease.unwrap();
    let offer = install(&native, 1);
    let waiting = fetch(reader.clone(), offer, "text/plain");
    let old = native_request(&native);
    route(&service, 2, Some("other"), false, false).unwrap();
    assert!(
        waiting
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .is_err()
    );
    assert!(native.poll().unwrap().lease.is_none());
    assert!(other.poll().unwrap().lease.is_some());
    assert!(native.reply(&old, b"retired", true).is_err());
    route(&service, 3, Some("native"), false, false).unwrap();
    assert_ne!(native.poll().unwrap().lease.unwrap(), old_lease);
    let offer = install(&native, 1); // A new assignment may start a new display.
    let waiting = fetch(reader.clone(), offer.clone(), "text/plain");
    let request = native_request(&native);
    assert_ne!(request.id, old.id);
    assert!(
        native
            .publish(&old_lease, &offer, 2, vec!["text/plain".into()])
            .is_err()
    );
    assert!(native.reply(&old, b"retired", true).is_err());
    native.reply(&request, b"new display", true).unwrap();
    assert_eq!(
        waiting
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap(),
        b"new display"
    );
    service.authority().revoke("native");
    assert_eq!(reader.offer().unwrap().origin, Origin::Empty);
}

#[test]
fn integration_retirement_preserves_native_local_data_but_revokes_host_exports() {
    let (_directory, service, producer, reader, _) = fixture();
    let native = provider(&service, "native");
    route(&service, 1, Some("native"), true, true).unwrap();
    let offer = install(&native, 1);
    let old = producer.reader().export_offer().unwrap().unwrap();
    let exported = export_read(producer.reader(), old);
    let request = native_request(&native);
    service.authority().revoke("host");
    assert!(
        exported
            .recv_timeout(Duration::from_millis(500))
            .unwrap()
            .is_err()
    );
    assert_eq!(reader.offer().unwrap(), offer);
    let local = fetch(reader, offer, "text/plain");
    native.reply(&request, b"still local", true).unwrap();
    assert_eq!(
        local.recv_timeout(Duration::from_secs(1)).unwrap().unwrap(),
        b"still local"
    );
    let replacement = Producer::new(
        service
            .authority()
            .grant("replacement", "integration")
            .unwrap(),
    )
    .unwrap();
    assert!(replacement.reader().export_offer().unwrap().is_none());
    assert!(route(&service, 1, Some("native"), true, true).is_err());
    route(&service, 2, Some("native"), true, true).unwrap();
    assert!(
        replacement.reader().export_offer().unwrap().is_some(),
        "export need not enable host imports"
    );
}

#[test]
fn maximum_format_offers_and_native_requests_fit_the_bounded_ipc_contract() {
    let (_directory, service, producer, reader, _) = fixture();
    let formats: Vec<_> = (0..256)
        .map(|index| format!("application/{}{:04}", "a".repeat(240), index))
        .collect();
    producer
        .update(clipboard_wire::Offer {
            revision: 9,
            formats: formats.clone(),
        })
        .unwrap();
    let expected = reader.offer().unwrap();
    let native = provider(&service, "native");
    route(&service, 1, Some("native"), false, false).unwrap();
    assert!(
        native
            .publish(
                &native.poll().unwrap().lease.unwrap(),
                &expected,
                1,
                formats.clone()
            )
            .unwrap()
    );
    let offer = reader.offer().unwrap();
    assert_eq!(offer.formats, formats);
    let reads: Vec<_> = formats
        .iter()
        .take(8)
        .map(|format| fetch(reader.clone(), offer.clone(), format))
        .collect();
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut requests = vec![];
    while requests.len() < 8 {
        requests.extend(native.poll().unwrap().requests);
        assert!(
            Instant::now() < deadline,
            "maximum metadata blocked native requests"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    let mut wrong = requests[0].clone();
    wrong.format = formats[255].clone();
    assert!(native.reply(&wrong, b"wrong request", true).is_err());
    for request in requests {
        native.reply(&request, b"bounded", true).unwrap();
    }
    for read in reads {
        assert_eq!(
            read.recv_timeout(Duration::from_secs(1)).unwrap().unwrap(),
            b"bounded"
        );
    }
}
