use super::*;
use crate::clipboard_export::source::{Demands, Grant};
use crate::desktop_integration::{
    ClipboardAccess, ClipboardOffer, ClipboardOrigin, NativeClipboard,
};
use newport_platform::clipboard::helper::{Board, owner::Owner};
use std::sync::Arc;

#[path = "clipboard_router_live_tests.rs"]
mod router;

struct Native {
    executable: std::path::PathBuf,
    name: String,
    adapter: Arc<NativeClipboard>,
}
impl Native {
    fn new() -> Self {
        let executable =
            std::path::PathBuf::from(std::env::var_os("NEWPORT_CLIPBOARD_HELPER").unwrap())
                .canonicalize()
                .unwrap();
        let name = format!("org.newport.test.lazy.{}", uuid::Uuid::new_v4());
        let adapter = Arc::new(NativeClipboard::new(Some(Owner::new(
            executable.clone(),
            Board::Test { name: name.clone() },
        ))));
        Self {
            executable,
            name,
            adapter,
        }
    }
    fn consumer(&self, operation: &str) -> tokio::task::JoinHandle<Value> {
        let executable = self.executable.clone();
        let name = self.name.clone();
        let operation = operation.to_owned();
        tokio::spawn(async move {
            let output = tokio::time::timeout(
                Duration::from_secs(8),
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
    async fn observe(&self, predicate: impl Fn(&ClipboardOffer) -> bool) -> ClipboardOffer {
        tokio::time::timeout(Duration::from_secs(4), async {
            loop {
                if let Some(offer) = self.adapter.offer(None).await
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
}
impl Drop for Native {
    fn drop(&mut self) {
        self.adapter.close();
        let _ = std::process::Command::new(&self.executable)
            .args(["consumer", &self.name, "release"])
            .output();
    }
}
fn grant(receiver: &Receiver, ready: &Value) -> Grant {
    Grant::new(
        uuid::Uuid::parse_str(ready["runtime"].as_str().unwrap()).unwrap(),
        uuid::Uuid::parse_str(ready["worker"].as_str().unwrap()).unwrap(),
        receiver.offer().unwrap().route_revision,
    )
    .unwrap()
}

#[tokio::test]
#[ignore = "Requires local clipboard_owner_check and NEWPORT_CLIPBOARD_LINUX_PEER Docker launcher"]
async fn native_copy_publication_survives_focus_and_preserves_a_newer_local_copy() {
    let (mut peer, ready) = Peer::start().await;
    assert_eq!(peer.call(json!({"op":"copies"})).await["version"], 2);
    let mut receiver = Receiver::default();
    receiver
        .enable_version(ready["runtime"].as_str().unwrap(), wire::COPY_VERSION)
        .unwrap();
    let native = Native::new();
    let mut demands = Demands::default();
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
        let copied = peer
            .apply(
                &mut receiver,
                json!({"op":"copy", "format":format, "bytes":STANDARD.encode(bytes)}),
            )
            .await;
        // Capture the native revision with the proposal, before the remote ACK.
        let expected = native.adapter.offer(None).await.unwrap().revision;
        let focus = grant(&receiver, &ready);
        let copy = peer.commit(&mut receiver).await;
        let source = demands.source_copy(copy, &focus).unwrap();
        let publication = native
            .adapter
            .publish(
                expected,
                source.offer().offer.formats.clone(),
                source.clone(),
            )
            .await
            .unwrap();
        let origin = ClipboardOrigin::Imported {
            publication: publication.generation(),
        };
        native.observe(|offer| offer.origin == origin).await;
        assert_eq!(
            peer.call(json!({"op":"status"})).await["nativeRequests"],
            copied["state"]["nativeRequests"]
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(20), demands.receive())
                .await
                .is_err(),
            "publication eagerly fetched data"
        );
        focus.revoke();
        peer.apply(&mut receiver, json!({"op":"focus", "focused":false}))
            .await;
        assert!(publication.is_current());
        let published = native.adapter.offer(None).await.unwrap();
        assert!(
            native
                .adapter
                .publish(
                    published.revision,
                    source.offer().offer.formats.clone(),
                    source.clone()
                )
                .await
                .is_none(),
            "accepted source authorized a second native write after focus loss"
        );
        assert!(
            publication.is_current(),
            "rejected duplicate revoked the accepted copy"
        );
        assert_eq!(
            native.adapter.offer(None).await.unwrap().revision,
            published.revision
        );
        let consumer = native.consumer(operation);
        let demand = tokio::time::timeout(Duration::from_secs(3), demands.receive())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(demand.copy().unwrap().id(), source.copy().unwrap().id());
        let (read, command) = receiver
            .read_copy(demand.copy().unwrap(), demand.format())
            .unwrap();
        let transfer = tokio::spawn(demand.complete(read));
        peer.apply(&mut receiver, json!({"op":"read", "command":command}))
            .await;
        transfer.await.unwrap();
        assert_eq!(consumer.await.unwrap()[field], true);
        assert_eq!(native.consumer(operation).await.unwrap()[field], true);
        assert!(publication.is_current());
        let verified = peer.call(json!({"op":"verify", "format":format})).await;
        for value in verified["values"].as_array().unwrap() {
            assert_eq!(STANDARD.decode(value.as_str().unwrap()).unwrap(), bytes);
        }
        assert_eq!(verified["state"]["broker"], copied["state"]["broker"]);
        assert_eq!(
            verified["state"]["nativeRequests"].as_u64().unwrap(),
            copied["state"]["nativeRequests"].as_u64().unwrap() + 1
        );

        // Keep both the source and publication externally alive. Native local
        // ownership loss must revoke their remote copy anyway.
        native.consumer("local-rich").await.unwrap();
        native
            .observe(|offer| offer.origin == ClipboardOrigin::Local)
            .await;
        assert!(!publication.is_current());
        assert!(!source.copy().unwrap().is_current());
        peer.release(&mut receiver).await;
        drop(publication);
        assert_eq!(
            native.consumer("public.utf8-plain-text").await.unwrap()["text"],
            true
        );
        assert_eq!(native.consumer("public.png").await.unwrap()["png"], true);
        peer.apply(&mut receiver, json!({"op":"focus", "focused":true}))
            .await;
    }

    // A local copy during the commit wait must not be made the new expected
    // revision merely to allow the delayed remote publication to proceed.
    let expected = native.adapter.offer(None).await.unwrap().revision;
    let focus = grant(&receiver, &ready);
    let (pending, command) = receiver.commit(&receiver.lease().unwrap()).unwrap();
    native.consumer("local-rich").await.unwrap();
    native
        .observe(|offer| offer.origin == ClipboardOrigin::Local && offer.revision != expected)
        .await;
    peer.apply(&mut receiver, json!({"op":"commit", "command":command}))
        .await;
    let source = demands
        .source_copy(pending.receive().await.unwrap(), &focus)
        .unwrap();
    assert!(
        native
            .adapter
            .publish(
                expected,
                source.offer().offer.formats.clone(),
                source.clone()
            )
            .await
            .is_none()
    );
    assert!(!source.copy().unwrap().is_current());
    peer.release(&mut receiver).await;
    assert_eq!(
        native.consumer("public.utf8-plain-text").await.unwrap()["text"],
        true
    );
    assert_eq!(native.consumer("public.png").await.unwrap()["png"], true);
    // Replace the board while a native consumer is waiting for remote bytes.
    // Late bytes must be drained without restoring the older remote promise.
    peer.apply(&mut receiver, json!({"op":"copy", "format":"text/plain", "bytes":STANDARD.encode(b"obsolete remote text")})).await;
    let expected = native.adapter.offer(None).await.unwrap().revision;
    let source = demands
        .source_copy(peer.commit(&mut receiver).await, &focus)
        .unwrap();
    let publication = native
        .adapter
        .publish(
            expected,
            source.offer().offer.formats.clone(),
            source.clone(),
        )
        .await
        .unwrap();
    focus.revoke();
    peer.apply(&mut receiver, json!({"op":"focus", "focused":false}))
        .await;
    let consumer = native.consumer("public.utf8-plain-text");
    let demand = tokio::time::timeout(Duration::from_secs(3), demands.receive())
        .await
        .unwrap()
        .unwrap();
    let (read, command) = receiver
        .read_copy(demand.copy().unwrap(), demand.format())
        .unwrap();
    let read_id = read.id();
    let transfer = tokio::spawn(demand.complete(read));
    native.consumer("local-rich").await.unwrap();
    native
        .observe(|offer| offer.origin == ClipboardOrigin::Local)
        .await;
    tokio::time::timeout(Duration::from_secs(1), transfer)
        .await
        .unwrap()
        .unwrap();
    assert!(!publication.is_current());
    receiver.expire();
    assert_eq!(receiver.cancellations(), [read_id]);
    peer.apply(&mut receiver, json!({"op":"read", "command":command}))
        .await;
    peer.release(&mut receiver).await;
    let result = consumer.await.unwrap();
    assert!(
        result["length"].is_null() || result["text"] == true,
        "stale remote bytes reached the native consumer: {result}"
    );
    drop(publication);
    assert_eq!(
        native.consumer("public.utf8-plain-text").await.unwrap()["text"],
        true
    );
    assert_eq!(native.consumer("public.png").await.unwrap()["png"], true);
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
