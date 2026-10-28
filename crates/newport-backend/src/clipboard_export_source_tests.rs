use super::*;
use crate::agent_runtime::protocol::clipboard::{Chunk, Offer};
use crate::clipboard_export::Receiver;

#[path = "clipboard_export_source_copy_tests.rs"]
mod copies;

fn setup() -> (Receiver, ExportOffer, Grant, Demands) {
    let runtime = Uuid::new_v4();
    let worker = Uuid::new_v4();
    let offer = ExportOffer {
        offer: Offer {
            runtime: runtime.to_string(),
            revision: 1,
            origin: Origin::Native {
                worker: worker.to_string(),
                revision: 1,
            },
            formats: vec!["text/plain".into(), "image/png".into()],
        },
        route_revision: 1,
    };
    let mut receiver = Receiver::default();
    receiver.enable(&offer.offer.runtime).unwrap();
    advertise(&mut receiver, Some(offer.clone()));
    (
        receiver,
        offer,
        Grant::new(runtime, worker, 1).unwrap(),
        Demands::default(),
    )
}
fn advertise(receiver: &mut Receiver, offer: Option<ExportOffer>) {
    receiver
        .accept(&wire::Event::Offer { offer }.encode().unwrap())
        .unwrap();
}
fn send(receiver: &mut Receiver, id: u64, bytes: &[u8], eof: bool) {
    receiver
        .accept(
            &wire::Event::Data {
                id,
                offset: 0,
                chunk: Some(Chunk {
                    bytes: bytes.to_vec(),
                    eof,
                }),
            }
            .encode()
            .unwrap(),
        )
        .unwrap();
}

#[test]
fn source_requires_exact_runtime_worker_and_route_grants() {
    let (receiver, _, grant, demands) = setup();
    let lease = receiver.lease().unwrap();
    let runtime = Uuid::parse_str(&grant.0.runtime).unwrap();
    let worker = Uuid::parse_str(&grant.0.worker).unwrap();
    assert!(Grant::new(runtime, worker, 0).is_none());
    for bad in [
        Grant::new(Uuid::new_v4(), worker, 1).unwrap(),
        Grant::new(runtime, Uuid::new_v4(), 1).unwrap(),
        Grant::new(runtime, worker, 2).unwrap(),
    ] {
        assert!(demands.source(lease.clone(), &bad).is_none());
    }
    let source = demands.source(lease.clone(), &grant).unwrap();
    assert!(source.is_current());
    assert!(demands.source(lease.clone(), &grant).is_some());
    let mut other = Receiver::default();
    other.enable(&lease.offer().offer.runtime).unwrap();
    advertise(&mut other, Some(lease.offer().clone()));
    assert!(
        demands.source(other.lease().unwrap(), &grant).is_none(),
        "matching runtime/worker UUIDs crossed integration connections"
    );
    grant.revoke();
    assert!(!source.is_current());
    assert!(demands.source(lease.clone(), &grant).is_none());
    let repeated = Grant::new(runtime, worker, 1).unwrap();
    assert!(
        demands.source(lease, &repeated).is_none(),
        "focus reused an old route epoch"
    );
}

#[test]
fn replacing_source_or_connection_never_revives_an_old_source() {
    let (mut receiver, mut offer, grant, demands) = setup();
    let first = demands.source(receiver.lease().unwrap(), &grant).unwrap();
    offer.offer.revision = 2;
    advertise(&mut receiver, Some(offer.clone()));
    assert!(!first.is_current());
    let second = demands.source(receiver.lease().unwrap(), &grant).unwrap();
    assert!(second.is_current());
    offer.route_revision = 2;
    advertise(&mut receiver, Some(offer.clone()));
    let renewed = Grant::new(
        Uuid::parse_str(&grant.0.runtime).unwrap(),
        Uuid::parse_str(&grant.0.worker).unwrap(),
        2,
    )
    .unwrap();
    let third = demands.source(receiver.lease().unwrap(), &renewed).unwrap();
    assert!(!grant.0.current.load(Ordering::Acquire));
    assert!(!first.is_current() && !second.is_current() && third.is_current());
    receiver.disconnect();
    assert!(!third.is_current());
    assert!(receiver.lease().is_none());
}

#[tokio::test]
async fn held_native_data_keeps_memory_slot_focus_and_connection_fences() {
    for revoke in 0..4 {
        let (mut receiver, offer, grant, demands) = setup();
        let source = demands.source(receiver.lease().unwrap(), &grant).unwrap();
        let slots = demands.slots.clone();
        let slot = slots.clone().try_acquire_owned().unwrap();
        let (read, _) = receiver.read(&offer, "text/plain").unwrap();
        send(&mut receiver, read.id(), b"remote", true);
        let data = Data {
            source: source.as_ref().clone(),
            data: read.receive().await.unwrap(),
            _slot: slot,
        };
        assert_eq!(data.bytes(), Some(&b"remote"[..]));
        assert_eq!(slots.available_permits(), wire::MAX_READS - 1);
        let budget = receiver.budget.clone();
        assert_eq!(budget.load(Ordering::Acquire), 6);
        match revoke {
            0 => grant.revoke(),
            1 => drop(grant),
            2 => receiver.disconnect(),
            3 => drop(demands),
            _ => unreachable!(),
        }
        assert!(data.bytes().is_none());
        assert_eq!(budget.load(Ordering::Acquire), 6);
        drop(data);
        assert_eq!(budget.load(Ordering::Acquire), 0);
        assert_eq!(slots.available_permits(), wire::MAX_READS);
    }
}

#[cfg(target_os = "macos")]
mod native {
    use super::*;
    use crate::desktop_integration::{ClipboardAccess, ClipboardPublication, NativeClipboard};
    use newport_platform::clipboard::helper::{Board, owner::Owner};

    struct Fixture {
        executable: std::path::PathBuf,
        name: String,
        owner: Owner,
        native: NativeClipboard,
    }
    impl Fixture {
        fn new() -> Self {
            let executable = std::path::PathBuf::from(
                std::env::var_os("NEWPORT_CLIPBOARD_HELPER")
                    .expect("explicit local fixture executable required"),
            )
            .canonicalize()
            .unwrap();
            let name = format!("org.newport.test.lazy.{}", Uuid::new_v4());
            let owner = Owner::new(executable.clone(), Board::Test { name: name.clone() });
            Self {
                executable,
                name,
                native: NativeClipboard::new(Some(owner.clone())),
                owner,
            }
        }
        fn consumer(&self, operation: &str) -> tokio::task::JoinHandle<serde_json::Value> {
            let executable = self.executable.clone();
            let name = self.name.clone();
            let operation = operation.to_owned();
            tokio::spawn(async move {
                let output = tokio::time::timeout(
                    Duration::from_secs(4),
                    tokio::process::Command::new(executable)
                        .args(["consumer", &name, &operation])
                        .kill_on_drop(true)
                        .output(),
                )
                .await
                .unwrap()
                .unwrap();
                assert!(
                    output.status.success(),
                    "native consumer failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
                serde_json::from_slice(&output.stdout).unwrap()
            })
        }
        async fn publish(&self, source: Arc<Source>) -> Box<dyn ClipboardPublication> {
            let revision = self.native.offer(None).await.unwrap().revision;
            self.native
                .publish(revision, source.offer().offer.formats.clone(), source)
                .await
                .unwrap()
        }
        async fn demand(&self, demands: &mut Demands) -> Demand {
            tokio::time::timeout(Duration::from_secs(3), demands.receive())
                .await
                .unwrap()
                .unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.owner.close();
            let _ = std::process::Command::new(&self.executable)
                .args(["consumer", &self.name, "release"])
                .output();
        }
    }

    #[tokio::test]
    #[ignore = "Build clipboard_owner_check and set NEWPORT_CLIPBOARD_HELPER to that local example executable"]
    async fn native_export_is_lazy_and_event_pump_completes_text_and_png() {
        let fixture = Fixture::new();
        let (mut receiver, _, grant, mut demands) = setup();
        let source = demands.source(receiver.lease().unwrap(), &grant).unwrap();
        let publication = fixture.publish(source.clone()).await;
        // Publishing metadata does not prefetch either format.
        assert_eq!(demands.slots.available_permits(), wire::MAX_READS);
        assert!(demands.receiver.try_recv().is_err());
        let png = [
            137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1,
            8, 4, 0, 0, 0, 181, 28, 12, 2, 0, 0, 0, 11, 73, 68, 65, 84, 120, 218, 99, 100, 248, 15,
            0, 1, 5, 1, 1, 39, 24, 227, 102, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
        ];
        for (operation, format, payload, field) in [
            (
                "public.utf8-plain-text",
                "text/plain",
                "Newport 世界\n".as_bytes(),
                "text",
            ),
            ("public.png", "image/png", png.as_slice(), "png"),
        ] {
            let consumer = fixture.consumer(operation);
            let demand = fixture.demand(&mut demands).await;
            assert_eq!(demand.format(), format);
            assert!(demand.is_current());
            let (read, _) = receiver.read(demand.offer(), demand.format()).unwrap();
            let id = read.id();
            let transfer = tokio::spawn(demand.complete(read));
            tokio::task::yield_now().await;
            assert!(
                !transfer.is_finished(),
                "read returned without remote payload"
            );
            // These decoded events are supplied by the integration pump while
            // the separate native process waits inside its provider callback.
            send(&mut receiver, id, payload, true);
            tokio::time::timeout(Duration::from_secs(2), transfer)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(consumer.await.unwrap()[field], true);
        }
        assert!(publication.is_current());
        // Imported native metadata is not echoed into the host-import channel.
        let imported = fixture.native.offer(None).await.unwrap();
        assert_eq!(
            imported.origin,
            crate::desktop_integration::ClipboardOrigin::Imported {
                publication: publication.generation(),
            }
        );
        assert!(imported.formats.iter().any(|f| f == "text/plain"));
        grant.revoke();
        assert!(!publication.is_current());
        tokio::time::timeout(Duration::from_secs(3), fixture.native.shutdown())
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore = "Build clipboard_owner_check and set NEWPORT_CLIPBOARD_HELPER to that local example executable"]
    async fn focus_revocation_cancels_a_waiting_native_paste_and_remote_demand() {
        let fixture = Fixture::new();
        let (mut receiver, _, grant, mut demands) = setup();
        let source = demands.source(receiver.lease().unwrap(), &grant).unwrap();
        let publication = fixture.publish(source).await;
        let consumer = fixture.consumer("public.utf8-plain-text");
        let demand = fixture.demand(&mut demands).await;
        let (read, _) = receiver.read(demand.offer(), demand.format()).unwrap();
        let id = read.id();
        let transfer = tokio::spawn(demand.complete(read));
        tokio::task::yield_now().await;
        grant.revoke();
        tokio::time::timeout(Duration::from_secs(1), transfer)
            .await
            .unwrap()
            .unwrap();
        assert!(!publication.is_current());
        receiver.expire();
        assert_eq!(receiver.cancellations(), [id]);
        // Bytes already in flight cannot complete the abandoned native paste.
        send(&mut receiver, id, b"too late", true);
        assert!(consumer.await.unwrap()["length"].is_null());
        tokio::time::timeout(Duration::from_secs(3), fixture.native.shutdown())
            .await
            .unwrap();
    }
}
