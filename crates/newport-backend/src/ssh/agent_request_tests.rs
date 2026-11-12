//! Exercise Agent::request over framed duplex I/O, including its ACK dispatcher.
//! The unused browser callback's link never dials. Everything runs in a
//! bounded subprocess, never with the user's SSH setup.
use crate::agent::{Agent, Event};
use crate::model::Server;
use newport_protocol::{
    clipboard::imports,
    encoding::protobuf::tokio::FrameReader,
    integration::{
        self, Call, ClientMessage, Reply, Request, Response, ServerMessage, wire::Message,
    },
};
use std::{sync::Arc, time::Duration};
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
/// The browser callback goes unused, so its link never dials.
fn callbacks_link() -> newport_ssh::Link {
    let server: Server = serde_json::from_value(serde_json::json!({
        "id":uuid::Uuid::new_v4(), "name":"Agent ACK fixture", "sshHost":"127.0.0.1",
        "sshPort":9, "sshUser":"fixture", "agentSource":"none"
    }))
    .unwrap();
    super::link(&server, crate::ssh::no_jump_hosts)
}

async fn receive(stream: &mut FrameReader<tokio::io::DuplexStream>) -> Call {
    let ClientMessage::Call(call) =
        ClientMessage::decode(stream.next().await.unwrap().unwrap()).unwrap()
    else {
        panic!("expected call")
    };
    assert_eq!(call.request, Request::Heartbeat);
    call
}
async fn acknowledge(stream: &mut FrameReader<tokio::io::DuplexStream>, id: u64, message: &str) {
    stream
        .get_mut()
        .write_all(
            &ServerMessage::Response(Response {
                id,
                reply: Reply::Acknowledged {
                    message: message.into(),
                },
            })
            .encode()
            .unwrap(),
        )
        .await
        .unwrap();
}
async fn exercise(link: newport_ssh::Link, count: usize, wrong_id: bool) {
    let (stream, peer) = tokio::io::duplex(16384);
    let requests: Vec<_> = (0..count)
        .map(|index| imports::Request {
            id: index as u64 + 1,
            revision: 1,
            format: format!("application/fixture-{index}"),
        })
        .collect();
    let sent = requests.clone();
    let accepted = count <= 8 && !wrong_id;
    let (done, finished) = tokio::sync::oneshot::channel();
    let peer = tokio::spawn(async move {
        let mut peer = FrameReader::new(peer, integration::CLIENT_LIMITS);
        let ready = receive(&mut peer).await;
        acknowledge(&mut peer, ready.id, "ready").await;
        let first = receive(&mut peer).await;
        for request in sent {
            peer.get_mut()
                .write_all(&ServerMessage::ClipboardRequest(request).encode().unwrap())
                .await
                .unwrap();
        }
        acknowledge(&mut peer, first.id + u64::from(wrong_id), "first").await;
        if accepted {
            let second = receive(&mut peer).await;
            acknowledge(&mut peer, second.id, "second").await;
        }
        finished.await.unwrap();
        let mut extra = [0];
        assert!(
            tokio::time::timeout(Duration::from_millis(20), peer.get_mut().read(&mut extra))
                .await
                .is_err()
        );
    });
    let mut agent = Agent::start(
        stream,
        Arc::new(NoDesktop),
        Arc::new(crate::integration_permissions::Gate::new(false, false)),
        super::callback::Callbacks::new(link),
    )
    .await
    .unwrap();
    if accepted {
        for message in ["first", "second"] {
            assert_eq!(
                agent.request(Request::Heartbeat).await.unwrap(),
                Reply::Acknowledged {
                    message: message.into()
                }
            );
        }
        for request in requests {
            assert_eq!(
                agent.event().await.unwrap(),
                Event::ClipboardRequest(request)
            );
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(20), agent.event())
                .await
                .is_err()
        );
    } else {
        let error = agent
            .request(Request::Heartbeat)
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(
            error,
            if wrong_id {
                "Invalid integration response ID"
            } else {
                "Unexpected or excessive agent events"
            }
        );
        assert!(
            agent
                .request(Request::Heartbeat)
                .await
                .unwrap_err()
                .to_string()
                .starts_with("SSH transport interrupted:")
        );
        assert!(
            agent
                .event()
                .await
                .unwrap_err()
                .to_string()
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
                let link = callbacks_link();
                exercise(link.clone(), 8, false).await;
                exercise(link.clone(), 9, false).await;
                exercise(link, 1, true).await;
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
