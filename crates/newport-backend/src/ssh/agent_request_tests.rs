//! Exercise Agent::request over framed duplex I/O, including its ACK dispatcher.
//! The unused browser callback retains a real loopback SSH connection. All keys
//! and known_hosts belong to a bounded subprocess, never the user's SSH setup.
use super::*;
use crate::agent::{Agent, control, wire};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const CHILD: &str = "NEWPORT_AGENT_REQUEST_FIXTURE";
const TEST: &str =
    "ssh::agent_request_tests::agent_request_defers_shared_events_and_fences_overflow";

struct NoDesktop;
impl crate::desktop_integration::DesktopIntegration for NoDesktop {
    fn on_main_thread(&self, _: Box<dyn FnOnce() + Send>) -> Result<(), String> {
        panic!("unexpected desktop dispatch")
    }
    fn clipboard_revision(&self) -> Option<isize> {
        panic!("unexpected native clipboard access")
    }
    fn clipboard_offer(&self, _: Option<isize>) -> Option<(isize, Vec<String>)> {
        panic!("unexpected native clipboard access")
    }
    fn read_clipboard_format(&self, _: isize, _: &str) -> Option<Vec<u8>> {
        panic!("unexpected native clipboard access")
    }
    fn open_url(&self, _: &str) -> Result<(), String> {
        panic!("unexpected browser access")
    }
}
struct Peer;
impl russh::server::Handler for Peer {
    type Error = anyhow::Error;
    async fn auth_none(&mut self, _: &str) -> Result<russh::server::Auth> {
        Ok(russh::server::Auth::Accept)
    }
}

async fn callbacks_connection() -> (Arc<Connection>, tokio::task::JoinHandle<()>) {
    let key = russh::keys::PrivateKey::from(
        russh::keys::ssh_key::private::Ed25519Keypair::from_seed(&[61; 32]),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    std::fs::write(
        std::env::var_os("NEWPORT_TEST_KNOWN_HOSTS").unwrap(),
        format!(
            "[127.0.0.1]:{port} {}\n",
            key.public_key().to_openssh().unwrap()
        ),
    )
    .unwrap();
    let config = russh::server::Config {
        keys: vec![key],
        auth_rejection_time: Duration::ZERO,
        ..Default::default()
    };
    let peer = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let session = russh::server::run_stream(Arc::new(config), socket, Peer)
            .await
            .unwrap();
        let _ = session.await;
    });
    let server: Server = serde_json::from_value(serde_json::json!({
        "id":uuid::Uuid::new_v4(), "name":"Agent ACK fixture", "sshHost":"127.0.0.1",
        "sshPort":port, "sshUser":"fixture", "agentSource":"none"
    }))
    .unwrap();
    // Deliberately bypass auth::authenticate: only this local fixture's `none`
    // authentication runs; no agent lookup, keychain or credential reader.
    let mut connection = Connection::handshake(&server).await.unwrap();
    assert!(
        connection
            .handle
            .authenticate_none("fixture")
            .await
            .unwrap()
            .success()
    );
    (Arc::new(connection), peer)
}

async fn receive(stream: &mut tokio::io::DuplexStream) -> (u8, Vec<u8>) {
    let mut header = [0; 5];
    stream.read_exact(&mut header).await.unwrap();
    let size = wire::payload_length(&header, 16384).unwrap();
    let mut payload = vec![0; size];
    stream.read_exact(&mut payload).await.unwrap();
    wire::decode(&payload, 16384).unwrap()
}

async fn exercise(connection: Arc<Connection>, count: usize, unknown: bool) {
    let (stream, mut peer) = tokio::io::duplex(16384);
    let events: Vec<_> = (0..count)
        .map(|index| {
            let kind = if unknown { b'?' } else { b"laLK"[index % 4] };
            (kind, format!("literal-event-{index}").into_bytes())
        })
        .collect();
    let sent = events.clone();
    let accepted = count <= 64 && !unknown;
    let (done, finished) = tokio::sync::oneshot::channel();
    let peer = tokio::spawn(async move {
        let hello =
            control::encode(&control::Hello::new(uuid::Uuid::new_v4().to_string())).unwrap();
        peer.write_all(&wire::encode(b'R', &hello).unwrap())
            .await
            .unwrap();
        assert_eq!(receive(&mut peer).await, (b'H', vec![]));
        // This ordering is controlled by receipt of the actual request; no
        // timing guess or manually copied ACK dispatch loop is involved.
        for (kind, data) in sent {
            peer.write_all(&wire::encode(kind, &data).unwrap())
                .await
                .unwrap();
        }
        peer.write_all(&wire::encode(b'A', b"first").unwrap())
            .await
            .unwrap();
        if accepted {
            assert_eq!(receive(&mut peer).await, (b'H', vec![]));
            peer.write_all(&wire::encode(b'A', b"second").unwrap())
                .await
                .unwrap();
        }
        finished.await.unwrap();
        // Failed requests fence before another write, even if an ACK arrived
        // after the invalid/excessive event. No automatic replay is allowed.
        let mut extra = [0];
        assert!(
            tokio::time::timeout(Duration::from_millis(20), peer.read(&mut extra))
                .await
                .is_err()
        );
    });
    let mut agent = Agent::start(
        stream,
        Arc::new(NoDesktop),
        Arc::new(crate::integration_permissions::Gate::new(false, false)),
        callback::Callbacks::new(connection),
    )
    .await
    .unwrap();
    if accepted {
        assert_eq!(agent.request(b'H', &[]).await.unwrap(), "first");
        // Already deferred GUI events must not be mistaken for the next ACK.
        assert_eq!(agent.request(b'H', &[]).await.unwrap(), "second");
        for event in events {
            assert_eq!(agent.event().await.unwrap(), event);
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(20), agent.event())
                .await
                .is_err()
        );
    } else {
        let error = agent.request(b'H', &[]).await.unwrap_err();
        assert_eq!(
            error,
            if unknown {
                "Unexpected agent event."
            } else {
                "Unexpected or excessive agent events"
            }
        );
        assert!(
            agent
                .request(b'H', &[])
                .await
                .unwrap_err()
                .starts_with("SSH transport interrupted:")
        );
        assert!(
            agent
                .event()
                .await
                .unwrap_err()
                .starts_with("SSH transport interrupted:")
        );
    }
    done.send(()).unwrap();
    peer.await.unwrap();
}

#[test]
fn agent_request_defers_shared_events_and_fences_overflow() {
    if std::env::var_os(CHILD).is_some() {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            tokio::time::timeout(Duration::from_secs(8), async {
                let (connection, peer) = callbacks_connection().await;
                exercise(connection.clone(), 64, false).await;
                exercise(connection.clone(), 65, false).await;
                exercise(connection.clone(), 1, true).await;
                connection.close().await;
                peer.abort();
            })
            .await
            .expect("Agent request fixture stalled");
        });
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", TEST, "--nocapture"])
        .env(CHILD, "1")
        .env("HOME", directory.path())
        .env(
            "NEWPORT_TEST_KNOWN_HOSTS",
            directory.path().join("known_hosts"),
        )
        .env("SSH_AUTH_SOCK", "")
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(12);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "isolated Agent request fixture failed");
            break;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("isolated Agent request fixture timed out");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
