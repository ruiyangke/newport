use super::*;

impl Fixture {
    fn commit(&mut self, id: u64, offer: &ExportOffer) -> Option<CopyLease> {
        // Cross the actual MessagePack boundary in both directions.
        let command = Control::Commit {
            id,
            offer: offer.clone(),
        }
        .encode()
        .unwrap();
        let response = self
            .relay
            .control(Control::decode(&command).unwrap())
            .unwrap()
            .unwrap();
        match Event::decode(&response.encode().unwrap()).unwrap() {
            Event::Committed { id: reply, copy } => {
                assert_eq!(reply, id);
                copy
            }
            other => panic!("unexpected commit response: {other:?}"),
        }
    }
    fn read_copy(&mut self, id: u64, copy: &CopyLease, format: &str) -> Option<Event> {
        let command = Control::ReadCopy {
            id,
            copy: copy.clone(),
            format: format.into(),
        }
        .encode()
        .unwrap();
        self.relay
            .control(Control::decode(&command).unwrap())
            .unwrap()
    }
    fn data(&mut self, id: u64) -> Vec<u8> {
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut bytes = vec![];
        loop {
            for event in self.poll() {
                let Event::Data {
                    id: received,
                    offset,
                    chunk: Some(chunk),
                } = event
                else {
                    panic!("unexpected data event: {event:?}")
                };
                assert_eq!(received, id);
                assert_eq!(offset, bytes.len());
                bytes.extend(chunk.bytes);
                if chunk.eof {
                    return bytes;
                }
            }
            assert!(Instant::now() < deadline, "copy data did not arrive");
            thread::sleep(Duration::from_millis(2));
        }
    }
}

#[test]
fn negotiated_copy_survives_focus_loss_and_late_commits_cannot_replace_it() {
    let mut fixture = Fixture::new();
    fixture.producer.enabled(false).unwrap();
    let offer = fixture.enable_version(protocol::COPY_VERSION);
    let copy = fixture.commit(1, &offer).unwrap();
    assert!(fixture.native.poll().unwrap().requests.is_empty());
    fixture.read_copy(2, &copy, "text/plain");
    let request = fixture.requested();
    fixture.route(2, true, false);
    assert!(matches!(
        fixture.poll().as_slice(),
        [Event::Offer { offer: None }]
    ));
    assert!(fixture.commit(3, &offer).is_none());
    fixture
        .native
        .reply(&request, b"paste into local editor", true)
        .unwrap();
    assert_eq!(fixture.data(2), b"paste into local editor");
    fixture.read_copy(4, &copy, "image/png");
    let request = fixture.requested();
    assert_eq!(request.format, "image/png");
    fixture
        .native
        .reply(&request, b"lazy image bytes", true)
        .unwrap();
    assert_eq!(fixture.data(4), b"lazy image bytes");
    assert!(fixture.native.poll().unwrap().requests.is_empty());
    fixture.route(3, false, false);
    assert!(matches!(fixture.poll().as_slice(), [Event::CopyRevoked { id }] if id == &copy.id));
    assert!(matches!(
        fixture.read_copy(5, &copy, "text/plain"),
        Some(Event::Data { chunk: None, .. })
    ));
    fixture.reaped();
}

#[test]
fn revocation_discards_buffered_committed_data_before_sending_any_chunk() {
    for cause in ["permission", "source", "release", "replacement"] {
        let mut fixture = Fixture::new();
        let offer = fixture.enable_version(protocol::COPY_VERSION);
        let copy = fixture.commit(1, &offer).unwrap();
        fixture.read_copy(2, &copy, "text/plain");
        let request = fixture.requested();
        fixture
            .native
            .reply(&request, b"buffered copy", true)
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !fixture.relay.reads[&2].finished() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(2));
        }
        match cause {
            "permission" => fixture.route(2, false, false),
            "source" => fixture.publish(2),
            "release" => {
                assert!(
                    matches!(fixture.relay.control(Control::Release { copy: copy.clone() }).unwrap(),
                    Some(Event::CopyRevoked { id }) if id == copy.id)
                );
            }
            "replacement" => {
                assert_ne!(fixture.commit(3, &offer).unwrap(), copy);
            }
            _ => unreachable!(),
        }
        let events = fixture.poll();
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, Event::Data { .. })),
            "{cause}: {events:?}"
        );
        if matches!(cause, "permission" | "source") {
            assert!(matches!(events.first(), Some(Event::CopyRevoked { id }) if id == &copy.id));
        }
        assert!(matches!(
            fixture.read_copy(4, &copy, "text/plain"),
            Some(Event::Data { chunk: None, .. })
        ));
        fixture.reaped();
    }
}

#[test]
fn legacy_negotiation_cannot_commit_or_read_copies_and_versions_cannot_change_midstream() {
    let mut fixture = Fixture::new();
    let offer = fixture.enable();
    assert!(
        fixture
            .relay
            .control(Control::Commit {
                id: 1,
                offer: offer.clone()
            })
            .is_err()
    );
    assert!(
        fixture
            .relay
            .control(Control::Enable {
                version: protocol::COPY_VERSION
            })
            .is_err()
    );
    fixture.relay.control(Control::Disable).unwrap();
    let current = fixture.enable_version(protocol::COPY_VERSION);
    let copy = fixture.commit(1, &current).unwrap();
    fixture.relay.control(Control::Disable).unwrap();
    fixture.enable();
    assert!(
        fixture
            .relay
            .control(Control::ReadCopy {
                id: 2,
                copy,
                format: "text/plain".into()
            })
            .is_err()
    );
    assert!(fixture.poll().is_empty());
    assert!(fixture.native.poll().unwrap().requests.is_empty());
}

#[test]
fn copy_and_focused_reads_share_the_same_bound_and_connection_drop_revokes_copy() {
    let mut fixture = Fixture::new();
    let offer = fixture.enable_version(protocol::COPY_VERSION);
    let copy = fixture.commit(1, &offer).unwrap();
    for id in 2..=9 {
        if id % 2 == 0 {
            assert!(fixture.read_copy(id, &copy, "text/plain").is_none());
        } else {
            assert!(fixture.read(id, &offer).is_none());
        }
    }
    assert!(matches!(
        fixture.read_copy(10, &copy, "text/plain"),
        Some(Event::Data { chunk: None, .. })
    ));
    let request = fixture.requested();
    fixture.route(2, true, false);
    assert!(matches!(
        fixture.poll().as_slice(),
        [Event::Offer { offer: None }]
    ));
    assert_eq!(fixture.relay.reads.len(), 4);
    assert!(
        fixture
            .relay
            .reads
            .values()
            .all(|transfer| transfer.committed)
    );
    let reader = fixture.producer.reader();
    let (send, receive) = mpsc::sync_channel(1);
    let task = thread::spawn(move || {
        let result = reader
            .read_committed(&copy, "text/plain", ReadCancellation::default())
            .and_then(|mut read| read.next(4096));
        send.send(result).unwrap();
    });
    drop(fixture.relay);
    assert!(
        receive
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .is_err()
    );
    task.join().unwrap();
    fixture
        .native
        .reply(&request, b"local selection remains", true)
        .unwrap();
    let poll = fixture.native.poll().unwrap();
    assert_eq!(poll.offer.origin, offer.offer.origin);
}
