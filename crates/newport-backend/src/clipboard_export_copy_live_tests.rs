//! Real protocol-v2 peer in a disposable local Docker container. This checks
//! transport/source lifetime; native publication authorization is a separate gate.
use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[cfg(target_os = "macos")]
#[path = "clipboard_export_native_copy_tests.rs"]
mod native;

struct Peer {
    child: tokio::process::Child,
    input: tokio::process::ChildStdin,
    output: BufReader<tokio::process::ChildStdout>,
}
impl Peer {
    async fn start() -> (Self, Value) {
        let mut child =
            tokio::process::Command::new(std::env::var_os("NEWPORT_CLIPBOARD_LINUX_PEER").unwrap())
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::inherit())
                .kill_on_drop(true)
                .spawn()
                .unwrap();
        let mut peer = Self {
            input: child.stdin.take().unwrap(),
            output: BufReader::new(child.stdout.take().unwrap()),
            child,
        };
        let ready = peer.receive().await;
        (peer, ready)
    }
    async fn receive(&mut self) -> Value {
        let mut line = String::new();
        let count = tokio::time::timeout(Duration::from_secs(18), self.output.read_line(&mut line))
            .await
            .unwrap()
            .unwrap();
        assert!(
            count > 0 && count <= 256 * 1024,
            "Linux clipboard peer exited or exceeded its reply bound"
        );
        serde_json::from_str(&line).unwrap()
    }
    async fn call(&mut self, command: Value) -> Value {
        self.input
            .write_all(format!("{command}\n").as_bytes())
            .await
            .unwrap();
        self.input.flush().await.unwrap();
        self.receive().await
    }
    async fn apply(&mut self, receiver: &mut Receiver, command: Value) -> Value {
        let reply = self.call(command).await;
        for event in reply["events"].as_array().unwrap() {
            receiver
                .accept(&STANDARD.decode(event.as_str().unwrap()).unwrap())
                .unwrap();
        }
        reply
    }
    async fn commit(&mut self, receiver: &mut Receiver) -> CommittedCopy {
        let (pending, command) = receiver.commit(&receiver.lease().unwrap()).unwrap();
        self.apply(receiver, json!({"op":"commit", "command":command}))
            .await;
        pending.receive().await.unwrap()
    }
    async fn release(&mut self, receiver: &mut Receiver) {
        let command = receiver.copy_release().expect("missing copy release");
        let Control::Release { ref copy } = command else {
            unreachable!()
        };
        receiver.copy_release_sent(&copy.id);
        self.apply(receiver, json!({"op":"release", "command":command}))
            .await;
        assert!(receiver.committed.is_none());
    }
}

#[tokio::test]
#[ignore = "Requires NEWPORT_CLIPBOARD_LINUX_PEER launcher for the local Docker composed agent"]
async fn real_linux_copies_survive_focus_and_release_without_replacing_native_selection() {
    let (mut peer, ready) = Peer::start().await;
    assert_eq!(peer.call(json!({"op":"copies"})).await["version"], 2);
    let mut receiver = Receiver::default();
    receiver
        .enable_version(ready["runtime"].as_str().unwrap(), wire::COPY_VERSION)
        .unwrap();
    let png = [
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 4,
        0, 0, 0, 181, 28, 12, 2, 0, 0, 0, 11, 73, 68, 65, 84, 120, 218, 99, 100, 248, 15, 0, 1, 5,
        1, 1, 39, 24, 227, 102, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];
    for (format, bytes) in [
        ("text/plain", "Newport 世界\n".as_bytes()),
        ("image/png", png.as_slice()),
    ] {
        let copied = peer
            .apply(
                &mut receiver,
                json!({"op":"copy", "format":format, "bytes":STANDARD.encode(bytes)}),
            )
            .await;
        let focused = receiver.lease().unwrap();
        let copy = peer.commit(&mut receiver).await;
        let before = peer.call(json!({"op":"status"})).await;
        assert_eq!(
            before["nativeRequests"], copied["state"]["nativeRequests"],
            "commit eagerly fetched data"
        );
        peer.apply(&mut receiver, json!({"op":"focus", "focused":false}))
            .await;
        assert!(!focused.is_current());
        assert!(copy.is_current());
        let (read, command) = receiver.read_copy(&copy, format).unwrap();
        peer.apply(&mut receiver, json!({"op":"read", "command":command}))
            .await;
        let held = read.receive().await.unwrap();
        assert_eq!(held.bytes().unwrap(), bytes);
        let verified = peer.call(json!({"op":"verify", "format":format})).await;
        for value in verified["values"].as_array().unwrap() {
            assert_eq!(STANDARD.decode(value.as_str().unwrap()).unwrap(), bytes);
        }
        assert_eq!(verified["state"]["broker"], copied["state"]["broker"]);
        assert_eq!(
            verified["state"]["nativeRequests"].as_u64().unwrap(),
            before["nativeRequests"].as_u64().unwrap() + 1
        );
        receiver.release_copy(&copy).unwrap();
        assert!(held.bytes().is_err());
        peer.release(&mut receiver).await;
        assert_eq!(
            peer.call(json!({"op":"status"})).await["broker"],
            copied["state"]["broker"]
        );
        peer.apply(&mut receiver, json!({"op":"focus", "focused":true}))
            .await;
    }
    // A dropped desktop requester still drains and releases the real runtime
    // acknowledgment, without acquiring bytes or leaving copy authority live.
    let (abandoned, command) = receiver.commit(&receiver.lease().unwrap()).unwrap();
    drop(abandoned);
    receiver.expire();
    peer.apply(&mut receiver, json!({"op":"commit", "command":command}))
        .await;
    peer.release(&mut receiver).await;

    // A proposal sent after real focus loss is rejected by the runtime too.
    let (late, command) = receiver.commit(&receiver.lease().unwrap()).unwrap();
    peer.apply(&mut receiver, json!({"op":"focus", "focused":false}))
        .await;
    peer.apply(&mut receiver, json!({"op":"commit", "command":command}))
        .await;
    assert!(late.receive().await.is_err());
    assert!(receiver.committed.is_none());
    peer.apply(&mut receiver, json!({"op":"focus", "focused":true}))
        .await;

    let old = peer.commit(&mut receiver).await;
    peer.apply(
        &mut receiver,
        json!({"op":"copy", "format":"text/plain", "bytes":STANDARD.encode(b"replacement")}),
    )
    .await;
    assert!(
        !old.is_current(),
        "new native copy retained old read authority"
    );
    assert!(receiver.read_copy(&old, "image/png").is_err());
    assert!(receiver.committed.is_none());
    assert_eq!(peer.call(json!({"op":"quit"})).await["stopped"], true);
    assert!(
        tokio::time::timeout(Duration::from_secs(18), peer.child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
}
