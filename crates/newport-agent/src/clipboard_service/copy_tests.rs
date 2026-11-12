use super::*;

fn copied_read(reader: Reader, copy: CopyLease, format: &str) -> mpsc::Receiver<io::Result<Chunk>> {
    let format = format.to_owned();
    let (send, receive) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let result = reader
            .read_committed(&copy, &format, ReadCancellation::default())
            .and_then(|mut read| read.next(4096));
        let _ = send.send(result);
    });
    receive
}

#[test]
fn committed_copy_reads_text_and_image_after_focus_moves_without_accepting_late_offers() {
    let (_directory, service, producer, reader, _) = fixture();
    let native = provider(&service, "native");
    route(&service, 1, Some("native"), true, true).unwrap();
    let source = install(&native, 1);
    let host = producer.reader();
    let proposal = host.export_offer().unwrap().unwrap();
    let mut copies = host.copy_session().unwrap();
    let copy = copies.commit(&proposal).unwrap().unwrap();
    assert_eq!(copy.offer, proposal);
    assert!(
        native.poll().unwrap().requests.is_empty(),
        "commit fetched payload"
    );

    let pending = copied_read(host.clone(), copy.clone(), "text/plain");
    let request = native_request(&native);
    let local = fetch(reader.clone(), source.clone(), "text/plain");
    route(&service, 2, Some("native"), true, false).unwrap();
    assert!(host.export_offer().unwrap().is_none());
    assert!(copies.commit(&proposal).unwrap().is_none());
    assert_eq!(copies.current().unwrap(), Some(copy.clone()));
    native
        .reply(&request, "paste after focus → local".as_bytes(), true)
        .unwrap();
    let text = pending
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap();
    assert_eq!(text.bytes, "paste after focus → local".as_bytes());
    assert_eq!(
        local.recv_timeout(Duration::from_secs(1)).unwrap().unwrap(),
        text.bytes
    );

    // A second format is first requested after focus has moved. The copy lease
    // authorizes demand, not an eager snapshot made before the focus change.
    let image = copied_read(host.clone(), copy.clone(), "image/png");
    let request = native_request(&native);
    assert_eq!(request.format, "image/png");
    native.reply(&request, b"\x89PNG\r\n\x1a\n", true).unwrap();
    assert_eq!(
        image
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap()
            .bytes,
        b"\x89PNG\r\n\x1a\n"
    );
    route(&service, 3, Some("native"), true, true).unwrap();
    assert!(
        copies.commit(&proposal).unwrap().is_none(),
        "old focus epoch accepted"
    );
    assert_eq!(copies.current().unwrap(), Some(copy.clone()));
    assert_eq!(
        copied_read(host, copy, "text/plain")
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap()
            .bytes,
        text.bytes
    );
    assert_eq!(reader.offer().unwrap(), source);
    assert!(
        native.poll().unwrap().requests.is_empty(),
        "cached paste fetched again"
    );
}

#[test]
fn committed_copy_revocation_interrupts_pending_reads_and_never_resurrects() {
    for cause in [
        "permission",
        "source",
        "host",
        "provider",
        "integration",
        "connection",
        "release",
    ] {
        let (_directory, service, producer, reader, _) = fixture();
        let native = provider(&service, "native");
        let _other = provider(&service, "other");
        route(&service, 1, Some("native"), true, true).unwrap();
        let source = install(&native, 1);
        let host = producer.reader();
        let proposal = host.export_offer().unwrap().unwrap();
        let mut copies = Some(host.copy_session().unwrap());
        let copy = copies.as_mut().unwrap().commit(&proposal).unwrap().unwrap();
        let pending = copied_read(host.clone(), copy.clone(), "text/plain");
        let request = native_request(&native);
        let local = fetch(reader.clone(), source.clone(), "text/plain");
        match cause {
            "permission" => route(&service, 2, Some("native"), false, false).unwrap(),
            "source" => {
                install(&native, 2);
            }
            "host" => publish(&producer, 10),
            "provider" => route(&service, 2, Some("other"), true, false).unwrap(),
            "integration" => service.authority().revoke("host"),
            "connection" => {
                copies.take();
            }
            "release" => copies.as_mut().unwrap().release(&copy).unwrap(),
            _ => unreachable!(),
        }
        assert!(
            pending
                .recv_timeout(Duration::from_millis(500))
                .unwrap()
                .is_err(),
            "{cause}"
        );
        if matches!(
            cause,
            "permission" | "connection" | "integration" | "release"
        ) {
            assert_eq!(
                reader.offer().unwrap(),
                source,
                "{cause} destroyed local copy"
            );
            native.reply(&request, b"still local", true).unwrap();
            assert_eq!(
                local.recv_timeout(Duration::from_secs(1)).unwrap().unwrap(),
                b"still local"
            );
        } else {
            assert!(
                local.recv_timeout(Duration::from_secs(1)).unwrap().is_err(),
                "{cause}"
            );
        }
        if cause == "permission" {
            route(&service, 3, Some("native"), true, true).unwrap();
        }
        assert!(
            copied_read(host, copy, "text/plain")
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .is_err(),
            "{cause} revived old lease"
        );
        if !matches!(cause, "connection" | "integration") {
            assert!(
                copies.as_mut().unwrap().current().unwrap().is_none(),
                "{cause}"
            );
        }
    }
}

#[test]
fn copy_authority_is_exact_and_old_connections_cannot_revoke_replacements() {
    let (_directory, service, producer, reader, _) = fixture();
    let native = provider(&service, "native");
    route(&service, 1, Some("native"), true, true).unwrap();
    install(&native, 1);
    let host = producer.reader();
    let proposal = host.export_offer().unwrap().unwrap();
    assert!(
        reader.copy_session().is_err(),
        "ordinary worker gained copy authority"
    );
    let mut first = host.copy_session().unwrap();
    let old = first.commit(&proposal).unwrap().unwrap();
    let mut second = host.copy_session().unwrap();
    let fresh = second.commit(&proposal).unwrap().unwrap();
    assert_ne!(old.id, fresh.id);
    assert!(first.current().unwrap().is_none());
    first.release(&old).unwrap();
    first.release(&fresh).unwrap(); // Same producer, wrong owning connection.
    drop(first);
    assert_eq!(second.current().unwrap(), Some(fresh.clone()));

    let mut forged = fresh.clone();
    forged.offer.route_revision += 1;
    for rejected in [old, forged] {
        assert!(
            copied_read(host.clone(), rejected, "text/plain")
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .is_err()
        );
    }
    assert!(
        copied_read(reader, fresh.clone(), "text/plain")
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .is_err()
    );
    assert!(
        native.poll().unwrap().requests.is_empty(),
        "rejected reads fetched payload"
    );
    let pending = copied_read(host, fresh.clone(), "text/plain");
    let request = native_request(&native);
    native.reply(&request, b"replacement", true).unwrap();
    assert_eq!(
        pending
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap()
            .bytes,
        b"replacement"
    );
    second.release(&fresh).unwrap();
    assert!(second.current().unwrap().is_none());
}

#[test]
fn committed_copy_cached_ranges_still_require_live_authority() {
    let (_directory, service, producer, _reader, _) = fixture();
    let native = provider(&service, "native");
    route(&service, 1, Some("native"), true, true).unwrap();
    install(&native, 1);
    let host = producer.reader();
    let mut copies = host.copy_session().unwrap();
    let copy = copies
        .commit(&host.export_offer().unwrap().unwrap())
        .unwrap()
        .unwrap();
    let mut stream = host
        .read_committed(&copy, "text/plain", ReadCancellation::default())
        .unwrap();
    let (send, receive) = mpsc::sync_channel(1);
    let task = std::thread::spawn(move || {
        let first = stream.next(4).unwrap();
        send.send((stream, first)).unwrap();
    });
    let request = native_request(&native);
    native.reply(&request, b"long cached copy", true).unwrap();
    let (mut stream, first) = receive.recv_timeout(Duration::from_secs(1)).unwrap();
    task.join().unwrap();
    assert_eq!(first.bytes, b"long");
    assert!(!first.eof);
    route(&service, 2, Some("native"), true, false).unwrap();
    assert_eq!(stream.next(4).unwrap().bytes, b" cac");
    copies.release(&copy).unwrap();
    assert!(
        stream.next(4).is_err(),
        "cached chunks bypassed revoked authority"
    );
}
