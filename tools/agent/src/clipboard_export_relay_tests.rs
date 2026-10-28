use super::*;
use crate::clipboard_service::{NativePublisher, NativeRequest, Producer, Route, Service};
use std::{
    os::unix::fs::PermissionsExt,
    time::{Duration, Instant},
};

struct Fixture {
    relay: Relay,
    native: NativePublisher,
    producer: Producer,
    service: Service,
    _directory: tempfile::TempDir,
}
impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let service = Service::start(
            &directory.path().join("clipboard.sock"),
            &uuid::Uuid::new_v4().to_string(),
        )
        .unwrap();
        let authority = service.authority();
        let producer = Producer::new(authority.grant("host", "integration").unwrap()).unwrap();
        let native =
            NativePublisher::new(authority.grant("native", "remote-apps").unwrap()).unwrap();
        let fixture = Self {
            relay: Relay::new(producer.reader()),
            native,
            producer,
            service,
            _directory: directory,
        };
        fixture.route(1, true, true);
        fixture.publish(1);
        fixture
    }
    fn route(&self, revision: u64, exports: bool, focused: bool) {
        self.service
            .authority()
            .configure_route(Route {
                revision,
                worker_id: Some("native".into()),
                exports,
                focused,
            })
            .unwrap();
    }
    fn publish(&self, revision: u64) {
        let poll = self.native.poll().unwrap();
        assert!(
            self.native
                .publish(
                    &poll.lease.unwrap(),
                    &poll.offer,
                    revision,
                    vec!["text/plain".into(), "image/png".into()]
                )
                .unwrap()
        );
    }
    fn enable(&mut self) -> ExportOffer {
        self.enable_version(protocol::VERSION)
    }
    fn enable_version(&mut self, version: u32) -> ExportOffer {
        assert!(
            self.relay
                .control(Control::Enable { version })
                .unwrap()
                .is_none()
        );
        match self.poll().pop().unwrap() {
            Event::Offer { offer: Some(offer) } => offer,
            value => panic!("unexpected announcement: {value:?}"),
        }
    }
    fn poll(&mut self) -> Vec<Event> {
        let mut output = vec![];
        self.relay.poll(&mut output).unwrap();
        let mut input = output.as_slice();
        let mut events = vec![];
        while !input.is_empty() {
            let (kind, data) = crate::wire::read_limited(&mut input, protocol::MAX_FRAME).unwrap();
            assert_eq!(kind, protocol::FRAME);
            events.push(Event::decode(&data).unwrap());
        }
        events
    }
    fn read(&mut self, id: u64, offer: &ExportOffer) -> Option<Event> {
        self.relay
            .control(Control::Read {
                id,
                offer: offer.clone(),
                format: "text/plain".into(),
            })
            .unwrap()
    }
    fn requested(&self) -> NativeRequest {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let mut poll = self.native.poll().unwrap();
            if let Some(request) = poll.requests.pop() {
                assert!(
                    poll.requests.is_empty(),
                    "one format should share one fetch"
                );
                return request;
            }
            assert!(Instant::now() < deadline, "no native payload request");
            thread::sleep(Duration::from_millis(2));
        }
    }
    fn reaped(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !self.relay.retiring.is_empty() {
            assert!(self.poll().is_empty());
            assert!(Instant::now() < deadline, "cancelled task did not retire");
            thread::sleep(Duration::from_millis(2));
        }
        assert!(self.relay.reads.is_empty());
    }
}

#[path = "clipboard_export_copy_tests.rs"]
mod copies;

#[test]
fn opt_in_is_metadata_only_and_does_not_grant_permission() {
    let mut fixture = Fixture::new();
    assert!(
        fixture.poll().is_empty(),
        "legacy client received an export event"
    );
    fixture.route(2, false, true);
    fixture
        .relay
        .control(Control::Enable {
            version: protocol::VERSION,
        })
        .unwrap();
    assert!(matches!(
        fixture.poll().as_slice(),
        [Event::Offer { offer: None }]
    ));
    fixture.route(3, true, false);
    assert!(fixture.poll().is_empty());
    fixture.route(4, true, true);
    assert!(matches!(
        fixture.poll().as_slice(),
        [Event::Offer { offer: Some(_) }]
    ));
    for _ in 0..4 {
        assert!(fixture.poll().is_empty());
        assert!(fixture.native.poll().unwrap().requests.is_empty());
    }
}

#[test]
fn bounded_concurrent_exports_share_one_fetch_with_imports_disabled() {
    let mut fixture = Fixture::new();
    fixture.producer.enabled(false).unwrap();
    let offer = fixture.enable();
    for id in 1..=8 {
        assert!(fixture.read(id, &offer).is_none());
    }
    assert!(matches!(
        fixture.read(9, &offer),
        Some(Event::Data {
            id: 9,
            offset: 0,
            chunk: None
        })
    ));
    let request = fixture.requested();
    // A slow native owner does not hold the event-loop thread.
    let started = Instant::now();
    assert!(fixture.poll().is_empty());
    assert!(started.elapsed() < Duration::from_millis(500));
    let bytes: Vec<_> = (0..150_000).map(|i| (i % 251) as u8).collect();
    let chunks: Vec<_> = bytes.chunks(protocol::CHUNK).collect();
    for (index, chunk) in chunks.iter().enumerate() {
        fixture
            .native
            .reply(&request, chunk, index + 1 == chunks.len())
            .unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut results = BTreeMap::<u64, Vec<u8>>::new();
    let mut completed = std::collections::BTreeSet::new();
    while completed.len() < 8 {
        let events = fixture.poll();
        assert!(events.len() <= 1, "a transfer monopolized the loop");
        for event in events {
            let Event::Data {
                id,
                offset,
                chunk: Some(chunk),
            } = event
            else {
                panic!("export failed: {event:?}")
            };
            assert!(!completed.contains(&id));
            let data = results.entry(id).or_default();
            assert_eq!(offset, data.len());
            data.extend(chunk.bytes);
            if chunk.eof {
                assert_eq!(*data, bytes);
                completed.insert(id);
            }
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(2));
    }
    assert!(fixture.native.poll().unwrap().requests.is_empty());
    fixture.reaped();
}

#[test]
fn focus_permission_and_source_changes_withdraw_reads_and_fence_old_offers() {
    let mut fixture = Fixture::new();
    let mut offer = fixture.enable();
    for id in 1..=3 {
        if id > 1 {
            // A late native reply may populate the local cache after host focus
            // is revoked. Use a new copy to exercise another pending fetch.
            fixture.publish(id);
            let Event::Offer {
                offer: Some(current),
            } = fixture.poll().pop().unwrap()
            else {
                panic!()
            };
            offer = current;
        }
        assert!(fixture.read(id * 2, &offer).is_none());
        let request = fixture.requested();
        match id {
            1 => fixture.route(2, true, false),
            2 => fixture.route(4, false, true),
            _ => fixture.publish(4),
        }
        let events = fixture.poll();
        assert!(
            matches!(events.as_slice(), [Event::Offer { .. }]),
            "old payload escaped: {events:?}"
        );
        fixture.native.reply(&request, b"stale", true).unwrap();
        assert!(matches!(
            fixture.read(id * 2 + 1, &offer),
            Some(Event::Data { chunk: None, .. })
        ));
        fixture.reaped();
        // Re-enabling exports cannot make any older route lease usable again.
        fixture.route(id * 2 + 1, true, true);
        match fixture.poll().pop().unwrap() {
            Event::Offer {
                offer: Some(current),
            } => {
                assert_ne!(current, offer);
                offer = current;
            }
            event => panic!("unexpected event: {event:?}"),
        }
    }
}

#[test]
fn focus_revocation_discards_an_already_buffered_payload() {
    let mut fixture = Fixture::new();
    let offer = fixture.enable();
    fixture.read(1, &offer);
    let request = fixture.requested();
    fixture
        .native
        .reply(&request, b"buffered old data", true)
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while !fixture.relay.reads[&1].finished() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(2));
    }
    fixture.route(2, true, false);
    assert!(matches!(
        fixture.poll().as_slice(),
        [Event::Offer { offer: None }]
    ));
    fixture.reaped();
}

#[test]
fn cancel_disable_and_drop_interrupt_waiters_without_reusing_ids() {
    let mut fixture = Fixture::new();
    let offer = fixture.enable();
    assert!(fixture.read(1, &offer).is_none());
    let _ = fixture.requested();
    fixture.relay.control(Control::Cancel { id: 1 }).unwrap();
    fixture.reaped();
    fixture.read(2, &offer);
    fixture.relay.control(Control::Disable).unwrap();
    fixture.reaped();
    assert!(fixture.poll().is_empty());
    let current = fixture.enable();
    assert_eq!(current, offer);
    assert!(
        fixture
            .relay
            .control(Control::Read {
                id: 2,
                offer: offer.clone(),
                format: "text/plain".into()
            })
            .is_err()
    );
    fixture.read(3, &offer);
    let started = Instant::now();
    drop(fixture.relay);
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(
        fixture.native.poll().is_ok(),
        "relay shutdown revoked the provider"
    );
}

#[test]
fn failed_payload_is_explicit_and_retiring_reads_bound_rapid_replacement() {
    let mut fixture = Fixture::new();
    let offer = fixture.enable();
    fixture.read(1, &offer);
    let request = fixture.requested();
    fixture.native.fail(&request).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let events = fixture.poll();
        if !events.is_empty() {
            assert!(matches!(
                events.as_slice(),
                [Event::Data {
                    id: 1,
                    offset: 0,
                    chunk: None
                }]
            ));
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(2));
    }
    fixture.reaped();
    fixture.publish(2);
    let Event::Offer { offer: Some(offer) } = fixture.poll().pop().unwrap() else {
        panic!()
    };
    for id in 2..100 {
        fixture.read(id, &offer);
        assert!(fixture.relay.reads.len() + fixture.relay.retiring.len() <= protocol::MAX_READS);
        fixture.relay.control(Control::Cancel { id }).unwrap();
    }
    fixture.reaped();
}
