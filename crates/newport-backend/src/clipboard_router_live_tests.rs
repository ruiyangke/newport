//! The router owns a real native publication while a separate consumer pastes
//! bytes from the composed Linux agent. Uses a disposable named pasteboard.
use super::*;
use crate::clipboard_router::{
    Router,
    driver::{Driver, Transport, Work},
};

struct PeerTransport<'a> {
    peer: &'a mut Peer,
    receiver: &'a mut Receiver,
}
impl Transport for PeerTransport<'_> {
    fn lease(&self) -> Option<crate::clipboard_export::Lease> {
        self.receiver.lease()
    }
    async fn commit(
        &mut self,
        lease: &crate::clipboard_export::Lease,
    ) -> Result<crate::clipboard_export::Commit, String> {
        let (commit, command) = self.receiver.commit(lease)?;
        self.peer
            .apply(self.receiver, json!({"op":"commit", "command":command}))
            .await;
        Ok(commit)
    }
    async fn read(
        &mut self,
        copy: &crate::clipboard_export::CommittedCopy,
        format: &str,
    ) -> Result<crate::clipboard_export::Read, String> {
        let (read, command) = self.receiver.read_copy(copy, format)?;
        self.peer
            .apply(self.receiver, json!({"op":"read", "command":command}))
            .await;
        Ok(read)
    }
}
async fn work(driver: &mut Driver) -> Work {
    tokio::time::timeout(Duration::from_secs(4), driver.next())
        .await
        .unwrap()
}

async fn observe(router: &Router, predicate: impl Fn(&ClipboardOffer) -> bool) -> ClipboardOffer {
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            if let Some(offer) = router.offer(None).await
                && predicate(&offer)
            {
                return offer;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "Requires local clipboard_owner_check and NEWPORT_CLIPBOARD_LINUX_PEER Docker launcher"]
async fn router_preserves_lazy_text_and_image_pastes_after_focus_moves() {
    let (mut peer, ready) = Peer::start().await;
    assert_eq!(peer.call(json!({"op":"copies"})).await["version"], 2);
    let mut receiver = Receiver::default();
    receiver
        .enable_version(ready["runtime"].as_str().unwrap(), wire::COPY_VERSION)
        .unwrap();
    let native = Native::new();
    let router = Router::new(native.adapter.clone());
    let demands = Demands::default();
    let mut driver = Driver::new(router.clone(), receiver.connection().unwrap()).unwrap();
    // The peer supplies runtime/worker authority. Display-only identity fields
    // are fixture values; desktop handshake validation has its own peer tests.
    let display = router
        .register_display(crate::graphical_session::Identity {
            runtime: crate::graphical_session::Runtime {
                instance: ready["runtime"].as_str().unwrap().parse().unwrap(),
                build_sha256: "a".repeat(64),
                transport_generation: 1,
            },
            session: uuid::Uuid::new_v4(),
            worker: ready["worker"].as_str().unwrap().parse().unwrap(),
            display_generation: 1,
        })
        .unwrap();
    let presenter = display.presenter().unwrap();
    let png = [
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 4,
        0, 0, 0, 181, 28, 12, 2, 0, 0, 0, 11, 73, 68, 65, 84, 120, 218, 99, 100, 248, 15, 0, 1, 5,
        1, 1, 39, 24, 227, 102, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];
    for (format, bytes, operation, field) in [
        (
            "text/plain",
            "Newport 世界\n".as_bytes(),
            "public.utf8-plain-text",
            "text",
        ),
        ("image/png", png.as_slice(), "public.png", "png"),
    ] {
        assert!(presenter.set_focused(true));
        let copied = peer
            .apply(
                &mut receiver,
                json!({"op":"copy", "format":format, "bytes":STANDARD.encode(bytes)}),
            )
            .await;
        router.offer(None).await.unwrap();
        assert!(
            driver
                .endpoint
                .begin_display_focus(&driver.endpoint.focused_display().unwrap())
                .unwrap()
                .acknowledge(grant(&receiver, &ready))
        );
        let mut transport = PeerTransport {
            peer: &mut peer,
            receiver: &mut receiver,
        };
        driver.propose(&mut transport).await.unwrap();
        let committed = work(&mut driver).await;
        assert!(matches!(committed, Work::Commit(_)));
        driver.complete(committed, &mut transport).await.unwrap();
        let published = work(&mut driver).await;
        assert!(matches!(published, Work::Publication(Ok(Some(_)))));
        driver.complete(published, &mut transport).await.unwrap();
        let imported = observe(&router, |o| {
            matches!(o.origin, ClipboardOrigin::Imported { .. })
        })
        .await;
        assert_eq!(
            peer.call(json!({"op":"status"})).await["nativeRequests"],
            copied["state"]["nativeRequests"]
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(20), driver.next())
                .await
                .is_err(),
            "router eagerly fetched data"
        );
        presenter.set_focused(false);
        peer.apply(&mut receiver, json!({"op":"focus", "focused":false}))
            .await;
        assert_eq!(router.offer(None).await.unwrap(), imported);
        let consumer = native.consumer(operation);
        let demand = work(&mut driver).await;
        assert!(matches!(demand, Work::Demand(_)));
        let mut transport = PeerTransport {
            peer: &mut peer,
            receiver: &mut receiver,
        };
        driver.complete(demand, &mut transport).await.unwrap();
        let transferred = work(&mut driver).await;
        assert!(matches!(transferred, Work::Read(Ok(()))));
        driver.complete(transferred, &mut transport).await.unwrap();
        assert_eq!(consumer.await.unwrap()[field], true);
        assert_eq!(native.consumer(operation).await.unwrap()[field], true);
        let verified = peer.call(json!({"op":"verify", "format":format})).await;
        for value in verified["values"].as_array().unwrap() {
            assert_eq!(STANDARD.decode(value.as_str().unwrap()).unwrap(), bytes);
        }
        assert_eq!(verified["state"]["broker"], copied["state"]["broker"]);
        assert_eq!(
            verified["state"]["nativeRequests"].as_u64().unwrap(),
            copied["state"]["nativeRequests"].as_u64().unwrap() + 1
        );
        native.consumer("local-rich").await.unwrap();
        observe(&router, |o| o.origin == ClipboardOrigin::Local).await;
        peer.release(&mut receiver).await;
        peer.apply(&mut receiver, json!({"op":"focus", "focused":true}))
            .await;
    }

    peer.apply(
        &mut receiver,
        json!({"op":"copy", "format":"text/plain", "bytes":STANDARD.encode(b"late remote copy")}),
    )
    .await;
    presenter.set_focused(true);
    assert!(
        driver
            .endpoint
            .begin_display_focus(&driver.endpoint.focused_display().unwrap())
            .unwrap()
            .acknowledge(grant(&receiver, &ready))
    );
    let expected = router.offer(None).await.unwrap().revision;
    let proposal = driver.endpoint.propose(receiver.lease().unwrap()).unwrap();
    let (commit, command) = receiver.commit(proposal.lease()).unwrap();
    native.consumer("local-rich").await.unwrap();
    observe(&router, |o| {
        o.origin == ClipboardOrigin::Local && o.revision != expected
    })
    .await;
    peer.apply(&mut receiver, json!({"op":"commit", "command":command}))
        .await;
    assert!(
        router
            .publish(proposal, commit.receive().await.unwrap(), &demands)
            .await
            .is_none()
    );
    peer.release(&mut receiver).await;
    assert_eq!(
        native.consumer("public.utf8-plain-text").await.unwrap()["text"],
        true
    );
    assert_eq!(native.consumer("public.png").await.unwrap()["png"], true);
    drop(driver);
    router.close();
    native.adapter.shutdown().await;
    assert_eq!(peer.call(json!({"op":"quit"})).await["stopped"], true);
    assert!(
        tokio::time::timeout(Duration::from_secs(18), peer.child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
}
