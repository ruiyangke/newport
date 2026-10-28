use super::*;
use crate::{clipboard_wire, wire};
use std::{
    io,
    os::unix::fs::PermissionsExt,
    sync::mpsc,
    time::{Duration, Instant},
};

fn fixture() -> (tempfile::TempDir, Service, Producer, Reader, String) {
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let service = Service::start(
        &directory.path().join("clipboard.sock"),
        &uuid::Uuid::new_v4().to_string(),
    )
    .unwrap();
    let authority = service.authority();
    let host = authority.grant("host", "integration").unwrap();
    let worker = "display".to_owned();
    let access = authority.grant(&worker, "remote-apps").unwrap();
    let producer = Producer::new(host).unwrap();
    producer.enabled(true).unwrap();
    (directory, service, producer, Reader::new(access), worker)
}
fn publish(producer: &Producer, revision: i64) {
    producer
        .update(clipboard_wire::Offer {
            revision,
            formats: vec!["image/png".into(), "text/plain".into()],
        })
        .unwrap();
}
fn requested(producer: &Producer) -> clipboard_wire::Request {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let mut output = vec![];
        producer.poll(&mut output).unwrap();
        if !output.is_empty() {
            let (kind, bytes) = wire::read(&mut output.as_slice()).unwrap();
            assert_eq!(kind, b'C');
            return clipboard_wire::Request::decode(&bytes).unwrap();
        }
        assert!(
            Instant::now() < deadline,
            "producer did not receive requested format"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn fetch(reader: Reader, offer: Offer, format: &str) -> mpsc::Receiver<io::Result<Vec<u8>>> {
    let format = format.to_owned();
    let (send, receive) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let result = (|| {
            let mut stream = reader.read(&offer, &format)?;
            let mut bytes = vec![];
            loop {
                let chunk = stream.next(4096)?;
                bytes.extend(chunk.bytes);
                if chunk.eof {
                    return Ok(bytes);
                }
            }
        })();
        let _ = send.send(result);
    });
    receive
}
#[test]
fn runtime_ipc_is_lazy_coalesces_formats_and_streams_bounded_chunks() {
    let (_directory, _service, producer, reader, _) = fixture();
    publish(&producer, -27);
    let offer = reader.offer().unwrap();
    assert_eq!(offer.formats, ["image/png", "text/plain"]);
    let mut output = vec![];
    producer.poll(&mut output).unwrap();
    assert!(output.is_empty(), "metadata fetched a payload");
    let first = fetch(reader.clone(), offer.clone(), "image/png");
    let request = requested(&producer);
    assert_eq!(request.revision, -27);
    assert_eq!(request.format, "image/png");
    let second = fetch(reader.clone(), offer.clone(), "image/png");
    let bytes: Vec<_> = (0..150_000).map(|i| (i % 251) as u8).collect();
    let chunks: Vec<_> = bytes.chunks(clipboard_wire::CHUNK).collect();
    for (index, chunk) in chunks.iter().enumerate() {
        producer
            .reply(&clipboard_wire::reply(
                &request,
                0,
                index + 1 == chunks.len(),
                chunk,
            ))
            .unwrap();
    }
    assert_eq!(
        first.recv_timeout(Duration::from_secs(3)).unwrap().unwrap(),
        bytes
    );
    assert_eq!(
        second
            .recv_timeout(Duration::from_secs(3))
            .unwrap()
            .unwrap(),
        bytes
    );
    assert_eq!(reader.get(offer.revision, "image/png").unwrap(), bytes);
    output.clear();
    producer.poll(&mut output).unwrap();
    assert!(output.is_empty(), "coalesced/cache reads fetched again");
}
#[test]
fn permission_epochs_reject_old_offers_and_late_host_replies() {
    let (_directory, _service, producer, reader, _) = fixture();
    publish(&producer, 1);
    let old_offer = reader.offer().unwrap();
    let waiting = fetch(reader.clone(), old_offer.clone(), "text/plain");
    let old = requested(&producer);
    producer.enabled(false).unwrap();
    assert!(
        waiting
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .is_err()
    );
    assert!(reader.offer().unwrap().formats.is_empty());
    producer.enabled(true).unwrap();
    publish(&producer, 1); // Desktop revision deliberately repeats.
    let offer = reader.offer().unwrap();
    assert_ne!(offer.revision, old_offer.revision);
    assert!(
        fetch(reader.clone(), old_offer, "text/plain")
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .is_err()
    );
    let waiting = fetch(reader.clone(), offer, "text/plain");
    let new = requested(&producer);
    assert_ne!(new.id, old.id);
    producer
        .reply(&clipboard_wire::reply(&old, 0, true, b"stale"))
        .unwrap();
    assert!(waiting.recv_timeout(Duration::from_millis(50)).is_err());
    producer
        .reply(&clipboard_wire::reply(&new, 0, true, b"current"))
        .unwrap();
    assert_eq!(
        waiting
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap(),
        b"current"
    );
}
#[test]
fn worker_revocation_cancels_waiting_reads_without_revoking_other_workers() {
    let (_directory, service, producer, reader, worker) = fixture();
    publish(&producer, 4);
    let offer = reader.offer().unwrap();
    let waiting = fetch(reader.clone(), offer.clone(), "text/plain");
    let request = requested(&producer);
    let started = Instant::now();
    service.authority().revoke(&worker);
    assert!(
        waiting
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .is_err()
    );
    assert!(started.elapsed() < Duration::from_millis(500));
    assert!(reader.offer().is_err());
    let other = Reader::new(service.authority().grant("other", "monitor").unwrap());
    let waiting = fetch(other, offer, "text/plain");
    producer
        .reply(&clipboard_wire::reply(&request, 0, true, b"shared cache"))
        .unwrap();
    assert_eq!(
        waiting
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap(),
        b"shared cache"
    );
}
#[test]
fn producer_role_single_ownership_and_runtime_shutdown_are_enforced() {
    let (directory, service, producer, reader, _) = fixture();
    let authority = service.authority();
    let viewer = authority.grant("viewer", "remote-apps").unwrap();
    assert!(Producer::new(viewer).is_err());
    let replacement = authority.grant("replacement", "integration").unwrap();
    assert!(Producer::new(replacement.clone()).is_err());
    publish(&producer, 7);
    let old_offer = reader.offer().unwrap();
    let waiting = fetch(reader.clone(), old_offer.clone(), "image/png");
    let _ = requested(&producer);
    authority.revoke("host");
    assert!(
        waiting
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .is_err()
    );
    assert!(producer.enabled(true).is_err());
    let replacement = Producer::new(replacement).unwrap();
    replacement.enabled(true).unwrap();
    publish(&replacement, 7);
    assert_ne!(reader.offer().unwrap().revision, old_offer.revision);
    let waiting = fetch(reader.clone(), reader.offer().unwrap(), "image/png");
    let _ = requested(&replacement);
    let started = Instant::now();
    drop(service);
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(
        waiting
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .is_err()
    );
    assert!(!directory.path().join("clipboard.sock").exists());
    assert!(reader.offer().is_err());
}
#[test]
fn credentials_and_offers_cannot_cross_runtime_instances() {
    let (_directory, service, producer, reader, _) = fixture();
    publish(&producer, 9);
    let old = reader.offer().unwrap();
    let (other_directory, other, other_producer, other_reader, _) = fixture();
    publish(&other_producer, 9);
    assert!(other_reader.read(&old, "image/png").is_err());
    let mut forged = service.authority().grant("candidate", "monitor").unwrap();
    forged.path = other_directory.path().join("clipboard.sock");
    assert!(Reader::new(forged).offer().is_err());
    drop(other);
}

#[test]
fn retired_idle_connections_do_not_accumulate_across_worker_lifetimes() {
    let (_directory, service, producer, _, _) = fixture();
    publish(&producer, 1);
    let authority = service.authority();
    let mut retired = vec![];
    for index in 0..64 {
        let worker = format!("reader-{index}");
        let reader = Reader::new(authority.grant(&worker, "remote-apps").unwrap());
        assert!(!reader.offer().unwrap().formats.is_empty());
        authority.revoke(&worker);
        // Deliberately retain every client socket. Runtime revocation must
        // close its server side without waiting for the idle limit.
        retired.push(reader);
    }
    for reader in retired {
        assert!(reader.offer().is_err());
    }
}

#[test]
fn abandoned_range_reads_release_connections_without_retiring_the_worker() {
    use super::protocol::{self, Request, Response};
    use std::os::unix::net::UnixStream;
    let (_directory, service, producer, reader, _) = fixture();
    publish(&producer, 1);
    let offer = reader.offer().unwrap();
    let access = service
        .authority()
        .grant("abandoned", "remote-apps")
        .unwrap();
    for index in 0..64 {
        let mut stream = UnixStream::connect(&access.path).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        protocol::write(&mut stream, &Request::Authenticate(access.clone())).unwrap();
        assert!(matches!(
            protocol::read::<Response>(&mut stream).unwrap(),
            Response::Ready
        ));
        protocol::write(
            &mut stream,
            &Request::Read {
                runtime: offer.runtime.clone(),
                revision: offer.revision,
                format: "text/plain".into(),
                offset: 0,
                limit: 4096,
                export: None,
            },
        )
        .unwrap();
        if index == 0 {
            let _ = requested(&producer); // The server is waiting on host data.
        }
        std::thread::sleep(Duration::from_millis(30));
        drop(stream);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(reader.offer().unwrap(), offer);
}

#[test]
fn cancelling_one_read_interrupts_its_wait_and_preserves_the_shared_fetch() {
    let (_directory, _service, producer, reader, _) = fixture();
    publish(&producer, 5);
    let offer = reader.offer().unwrap();
    let cancellation = ReadCancellation::default();
    let mut read = reader
        .read_with_cancellation(&offer, "text/plain", cancellation.clone())
        .unwrap();
    let (send, receive) = mpsc::sync_channel(1);
    let task = std::thread::spawn(move || {
        let result = read.next(4096);
        assert!(
            read.next(4096).is_err(),
            "cancelled read must remain failed"
        );
        send.send(result).unwrap();
    });
    let request = requested(&producer);
    let other = fetch(reader.clone(), offer.clone(), "text/plain");
    let started = Instant::now();
    cancellation.cancel();
    assert!(
        receive
            .recv_timeout(Duration::from_millis(500))
            .unwrap()
            .is_err()
    );
    task.join().unwrap();
    assert!(started.elapsed() < Duration::from_millis(500));
    assert!(
        reader
            .read_with_cancellation(&offer, "text/plain", cancellation)
            .is_err()
    );
    producer
        .reply(&clipboard_wire::reply(&request, 0, true, b"still wanted"))
        .unwrap();
    assert_eq!(
        other.recv_timeout(Duration::from_secs(1)).unwrap().unwrap(),
        b"still wanted"
    );
    assert_eq!(
        reader.get(offer.revision, "text/plain").unwrap(),
        b"still wanted"
    );
    let mut output = vec![];
    producer.poll(&mut output).unwrap();
    assert!(
        output.is_empty(),
        "cancellation restarted the coalesced host fetch"
    );
    let cancelled = ReadCancellation::default();
    cancelled.cancel();
    assert!(
        reader
            .read_with_cancellation(&offer, "text/plain", cancelled)
            .is_err()
    );
}

#[test]
fn read_cancellation_interrupts_authentication_before_payload_work() {
    use super::protocol::{self, Request};
    use std::os::unix::net::UnixListener;
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = directory.path().join("clipboard.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let reader = Reader::new(Access {
        path,
        runtime: "runtime".into(),
        worker: "worker".into(),
        token: uuid::Uuid::new_v4().to_string(),
    });
    let offer = Offer {
        runtime: "runtime".into(),
        revision: 1,
        origin: Origin::Host,
        formats: vec!["text/plain".into()],
    };
    let cancellation = ReadCancellation::default();
    let task_cancellation = cancellation.clone();
    let (send, receive) = mpsc::sync_channel(1);
    let task = std::thread::spawn(move || {
        send.send(
            reader
                .read_with_cancellation(&offer, "text/plain", task_cancellation)
                .is_err(),
        )
        .unwrap();
    });
    let (mut stream, _) = listener.accept().unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    assert!(matches!(
        protocol::read::<Request>(&mut stream).unwrap(),
        Request::Authenticate(_)
    ));
    // Withhold Ready. Cancellation must interrupt the authentication wait too.
    cancellation.cancel();
    assert!(receive.recv_timeout(Duration::from_millis(500)).unwrap());
    task.join().unwrap();
}

#[path = "native_tests.rs"]
mod native;
