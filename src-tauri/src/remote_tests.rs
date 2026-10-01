//! Real OpenSSH/Linux checks. Run with `npm run test:remote`.
use crate::agent::wire;
use crate::{
    model::{Server, Tunnel},
    ssh::{self, sftp::Sftp, ExecSession, Forwarding},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    time::{timeout, Duration},
};
use uuid::Uuid;

mod arboard;
mod boundaries;
mod git;
mod recovery;

fn server() -> Server {
    serde_json::from_value(serde_json::json!({
        "id": Uuid::new_v4(), "name": "Disposable Linux", "sshUser": "fixture",
        "sshHost": "127.0.0.1", "sshPort": std::env::var("NEWPORT_TEST_SSH_PORT").unwrap().parse::<u16>().unwrap(),
        "identityFile": std::env::var("NEWPORT_TEST_IDENTITY").unwrap()
    })).unwrap()
}

#[tokio::test]
#[ignore = "Requires npm run test:remote and real OpenSSH"]
async fn authentication_exec_sftp_and_metrics() {
    let mut server = server();
    let session = ExecSession::connect(&server).await.unwrap();
    let payload = vec![b'x'; 512 * 1024];
    assert_eq!(
        session
            .execute("wc -c", Some(&payload))
            .await
            .unwrap()
            .trim(),
        payload.len().to_string()
    );
    assert_eq!(
        session.execute("printf 'remote 世界'", None).await.unwrap(),
        "remote 世界"
    );
    let failed = ssh::execute(&server, "printf denied >&2; exit 7", None)
        .await
        .unwrap_err();
    assert!(failed.contains("status 7") && failed.contains("denied"));
    session
        .execute("printf 'sftp-data' > ~/roundtrip.txt", None)
        .await
        .unwrap();
    let sftp = Sftp::connect(&server).await.unwrap();
    assert_eq!(
        sftp.session
            .stat("/home/fixture/roundtrip.txt")
            .await
            .unwrap()
            .attrs
            .size,
        Some(9)
    );
    sftp.session
        .remove("/home/fixture/roundtrip.txt")
        .await
        .unwrap();
    assert!(session
        .execute("test -e ~/roundtrip.txt", None)
        .await
        .is_err());
    let metrics = crate::cockpit::collect(&server, crate::cockpit::Section::Overview)
        .await
        .unwrap();
    assert!(metrics["hostname"]
        .as_str()
        .is_some_and(|name| !name.is_empty()));
    assert!(metrics["cpu"].is_number());
    session.close().await;
    server.identity_file = None;
    server.agent_source = Some("system".into());
    assert_eq!(
        ssh::execute(&server, "printf agent-ok", None)
            .await
            .unwrap(),
        "agent-ok"
    );
    server.agent_key_fingerprint = Some("SHA256:missing-key".into());
    assert!(ssh::execute(&server, "true", None).await.is_err());
}

#[tokio::test]
#[ignore = "Requires npm run test:remote and real OpenSSH"]
async fn forwarding_roundtrip_and_shutdown() {
    let server = server();
    let reservation = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local = reservation.local_addr().unwrap().port();
    drop(reservation);
    let tunnel: Tunnel = serde_json::from_value(serde_json::json!({
        "id":Uuid::new_v4(), "serverId":server.id, "name":"HTTP fixture",
        "localPort":local, "remoteHost":"127.0.0.1", "remotePort":8080
    }))
    .unwrap();
    let forwarding = Forwarding::start(&server, Some(&tunnel)).await.unwrap();
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", local))
        .await
        .unwrap();
    stream
        .write_all(b"GET / HTTP/1.0\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    let mut response = String::new();
    timeout(
        Duration::from_secs(10),
        stream.read_to_string(&mut response),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(response.contains("200 OK") && response.contains("newport remote fixture"));
    forwarding.shutdown().await;
    let _released = TcpListener::bind(("127.0.0.1", local)).await.unwrap();
}

#[tokio::test]
#[ignore = "Requires npm run test:remote and real OpenSSH"]
async fn deploy_reinstall_and_agent_protocol() {
    let server = server();
    let session = ExecSession::connect(&server).await.unwrap();
    crate::agent::install(&session).await.unwrap();
    let hash = session
        .execute("sha256sum ~/.local/bin/newport-agent", None)
        .await
        .unwrap();
    crate::agent::install(&session).await.unwrap();
    crate::agent::reinstall(&session).await.unwrap();
    assert_eq!(
        session
            .execute("sha256sum ~/.local/bin/newport-agent", None)
            .await
            .unwrap(),
        hash
    );
    let mut stream = session
        .stream(&format!(
            "exec ~/.local/bin/newport-agent serve {} --clipboard --browser",
            Uuid::new_v4()
        ))
        .await
        .unwrap();
    async fn event(stream: &mut (impl tokio::io::AsyncRead + Unpin), kind: u8) -> Vec<u8> {
        timeout(Duration::from_secs(10), async {
            let mut header = [0; 5];
            stream.read_exact(&mut header).await.unwrap();
            assert_eq!(header[0], b'M');
            let size = wire::payload_length(&header, 1024 * 1024).unwrap();
            assert!(size < 1024 * 1024);
            let mut data = vec![0; size];
            stream.read_exact(&mut data).await.unwrap();
            let (actual, data) = wire::decode(&data, 1024 * 1024).unwrap();
            assert_eq!(
                actual,
                kind,
                "Unexpected agent payload: {}",
                String::from_utf8_lossy(&data)
            );
            data
        })
        .await
        .unwrap()
    }
    assert_eq!(event(&mut stream, b'R').await, b"newport-agent/6");
    let mut archive = tar::Builder::new(Vec::new());
    for (name, data) in [
        ("text/plain", "container clipboard 世界\n"),
        ("TARGETS", "text/plain\nTARGETS\n"),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o600);
        header.set_cksum();
        archive
            .append_data(&mut header, name, data.as_bytes())
            .unwrap();
    }
    let bytes = archive.into_inner().unwrap();
    frame(&mut stream, b'S', &bytes).await;
    event(&mut stream, b'A').await;
    assert_eq!(
        session
            .execute("~/.local/bin/xclip -selection clipboard -o", None)
            .await
            .unwrap(),
        "container clipboard 世界\n"
    );
    let (opened, ()) = tokio::join!(
        session.execute(
            "~/.local/bin/xdg-open 'https://example.com/login?code=fixture'",
            None
        ),
        async {
            let (id, url) = wire::parse_browser_request(&event(&mut stream, b'O').await).unwrap();
            assert_eq!(url, "https://example.com/login?code=fixture");
            frame(
                &mut stream,
                b'B',
                &wire::browser_reply(id, true, None).unwrap(),
            )
            .await;
        }
    );
    opened.unwrap();
    frame(&mut stream, b'Q', &[]).await;
    timeout(Duration::from_secs(10), stream.read_to_end(&mut Vec::new()))
        .await
        .unwrap()
        .unwrap();
    assert!(session
        .execute("~/.local/bin/xclip -selection clipboard -o", None)
        .await
        .is_err());
    session.close().await;
}

#[tokio::test]
#[ignore = "Requires npm run test:remote and real OpenSSH"]
async fn interactive_shell_resize_and_exit() {
    use crate::terminal::{Event, Input};
    let (input, receiver) = tokio::sync::mpsc::channel(8);
    let (output, mut events) = tokio::sync::mpsc::channel(32);
    let task =
        tokio::spawn(async move { ssh::terminal(&server(), 80, 24, receiver, &output).await });
    timeout(Duration::from_secs(15), async {
        assert!(matches!(events.recv().await, Some(Event::Ready)));
        input.send(Input::Resize(100, 40)).await.unwrap();
        input
            .send(Input::Data(
                b"stty size; printf 'pty-%s\\n' roundtrip; exit 0\n".to_vec(),
            ))
            .await
            .unwrap();
        let mut text = Vec::new();
        loop {
            match events.recv().await.unwrap() {
                Event::Data(bytes) => text.extend(bytes),
                Event::Exit(code) => {
                    assert_eq!(code, Some(0));
                    break;
                }
                Event::Error(error) => panic!("{error}"),
                Event::Ready => panic!("Duplicate ready event"),
            }
        }
        let text = String::from_utf8_lossy(&text);
        assert!(text.contains("40 100"), "PTY size missing: {text}");
        assert!(
            text.contains("pty-roundtrip"),
            "Shell output missing: {text}"
        );
    })
    .await
    .unwrap();
    task.await.unwrap().unwrap();
}

#[tokio::test]
#[ignore = "Requires npm run test:remote and real OpenSSH"]
async fn changed_host_key_is_rejected_and_restoration_recovers() {
    struct Restore(std::path::PathBuf, Vec<u8>);
    impl Drop for Restore {
        fn drop(&mut self) {
            std::fs::write(&self.0, &self.1).unwrap();
        }
    }
    let path = std::path::PathBuf::from(std::env::var("NEWPORT_TEST_KNOWN_HOSTS").unwrap());
    let restore = Restore(path.clone(), std::fs::read(&path).unwrap());
    let wrong = std::fs::read_to_string(format!(
        "{}.pub",
        std::env::var("NEWPORT_TEST_IDENTITY").unwrap()
    ))
    .unwrap();
    std::fs::write(&path, format!("[127.0.0.1]:{} {wrong}", server().ssh_port)).unwrap();
    let error = ssh::execute(&server(), "true", None).await.unwrap_err();
    assert!(
        error.contains("changed"),
        "Expected host-key rejection: {error}"
    );
    drop(restore);
    assert_eq!(
        ssh::execute(&server(), "printf recovered", None)
            .await
            .unwrap(),
        "recovered"
    );
}

#[tokio::test]
#[ignore = "Requires npm run test:remote"]
async fn shell_setup_is_idempotent_and_respects_protected_files() {
    let session = ExecSession::connect(&server()).await.unwrap();
    crate::agent::install(&session).await.unwrap();
    let output = session
        .execute("newport-test-fixture shell-setup", None)
        .await
        .unwrap();
    for shell in ["bash", "zsh", "fish"] {
        assert!(
            output.contains(&format!("{shell}: repeated install")),
            "{output}"
        );
    }
    session.close().await;
}

async fn frame(stream: &mut (impl tokio::io::AsyncWrite + Unpin), kind: u8, data: &[u8]) {
    stream
        .write_all(&wire::encode(kind, data).unwrap())
        .await
        .unwrap();
}
async fn receive(stream: &mut (impl tokio::io::AsyncRead + Unpin), kind: u8) -> Vec<u8> {
    timeout(Duration::from_secs(15), async {
        let mut header = [0; 5];
        stream.read_exact(&mut header).await.unwrap();
        assert_eq!(header[0], b'M');
        let size = wire::payload_length(&header, 1024 * 1024).unwrap();
        assert!(size < 1024 * 1024);
        let mut bytes = vec![0; size];
        stream.read_exact(&mut bytes).await.unwrap();
        let (actual, bytes) = wire::decode(&bytes, 1024 * 1024).unwrap();
        assert_eq!(actual, kind);
        bytes
    })
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "Requires npm run test:remote"]
async fn demand_clipboard_compression_freshness_and_failed_transfer_recovery() {
    use std::io::Write;
    let session = ExecSession::connect(&server()).await.unwrap();
    crate::agent::install(&session).await.unwrap();
    let mut stream = session
        .stream(&format!(
            "exec ~/.local/bin/newport-agent serve {} --clipboard",
            Uuid::new_v4()
        ))
        .await
        .unwrap();
    assert_eq!(receive(&mut stream, b'R').await, b"newport-agent/6");
    // A new offer invalidates the old cache. A failed transfer of that revision
    // must be retryable without another clipboard change.
    for (revision, corrupt) in [(100i64, false), (101, true), (101, false)] {
        frame(
            &mut stream,
            b'M',
            &rmp_serde::to_vec(&(revision, vec!["text/plain"])).unwrap(),
        )
        .await;
        receive(&mut stream, b'A').await;
        let content = format!("revision {revision}: 世界\n").repeat(8192);
        let (read, ()) = tokio::join!(
            session.execute("~/.local/bin/xclip -selection clipboard -o", None),
            async {
                let (id, received_revision, format): (u64, i64, String) =
                    rmp_serde::from_slice(&receive(&mut stream, b'C').await).unwrap();
                assert_eq!(received_revision, revision);
                assert_eq!(format, "text/plain");
                let chunks: Vec<_> = content.as_bytes().chunks(64 * 1024).collect();
                for (index, chunk) in chunks.iter().enumerate() {
                    let mut encoder =
                        flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
                    encoder.write_all(chunk).unwrap();
                    let bytes = if corrupt {
                        b"invalid zlib".to_vec()
                    } else {
                        encoder.finish().unwrap()
                    };
                    let response = rmp_serde::to_vec(&(
                        id,
                        revision,
                        2u8,
                        (corrupt || index + 1 == chunks.len()),
                        serde_bytes::Bytes::new(&bytes),
                    ))
                    .unwrap();
                    frame(&mut stream, b'D', &response).await;
                    if corrupt {
                        break;
                    }
                }
            }
        );
        if corrupt {
            assert!(read.is_err());
            // Failed reads are coalesced for one second to avoid request storms.
            tokio::time::sleep(Duration::from_millis(1100)).await;
        } else {
            assert_eq!(read.unwrap(), content);
            // Cache hit finishes without servicing another demand request.
            assert_eq!(
                timeout(
                    Duration::from_secs(5),
                    session.execute("~/.local/bin/xclip -selection clipboard -o", None)
                )
                .await
                .unwrap()
                .unwrap(),
                content
            );
        }
        frame(&mut stream, b'H', &[]).await;
        receive(&mut stream, b'A').await;
    }
    frame(&mut stream, b'Q', &[]).await;
    timeout(Duration::from_secs(10), stream.read_to_end(&mut Vec::new()))
        .await
        .unwrap()
        .unwrap();
    session.close().await;
}

#[tokio::test]
#[ignore = "Requires npm run test:remote"]
async fn files_transfer_roundtrip_and_existing_file_protection() {
    use crate::files::{transfer, Operations};
    use std::sync::Arc;
    let server = server();
    let sftp = Arc::new(Sftp::connect(&server).await.unwrap());
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join(format!("世界-{}.bin", Uuid::new_v4()));
    let bytes: Vec<u8> = (0..524_288).map(|n| (n % 251) as u8).collect();
    std::fs::write(&source, &bytes).unwrap();
    let operations = Operations::default();
    let remote = transfer::upload(
        &operations,
        sftp.clone(),
        &source,
        "/home/fixture",
        |_, _| {},
    )
    .await
    .unwrap();
    assert!(transfer::upload(
        &operations,
        sftp.clone(),
        &source,
        "/home/fixture",
        |_, _| {}
    )
    .await
    .is_err());
    let destination = temp.path().join("download.bin");
    transfer::download(&sftp, &remote, &destination, |_, _| {})
        .await
        .unwrap();
    assert_eq!(std::fs::read(&destination).unwrap(), bytes);
    sftp.session.remove(remote).await.unwrap();
    operations.shutdown().await;
}

#[tokio::test]
#[ignore = "Requires npm run test:remote"]
async fn browser_callback_http_roundtrip_conflict_and_cleanup() {
    let session = ExecSession::connect(&server()).await.unwrap();
    let reservation = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = reservation.local_addr().unwrap().port();
    let url = url::Url::parse(&format!(
        "https://login.example/authorize?redirect_uri=http://127.0.0.1:{port}/callback"
    ))
    .unwrap();
    let mut callbacks = session.callbacks();
    assert!(callbacks
        .prepare(ssh::callback::endpoint(&url).unwrap().unwrap())
        .await
        .is_err());
    drop(reservation);
    // A real remote HTTP receiver stands in for the CLI's login callback.
    let mut receiver = session
        .stream(&format!("newport-test-fixture callback {port}"))
        .await
        .unwrap();
    let mut ready = [0; 6];
    timeout(Duration::from_secs(10), receiver.read_exact(&mut ready))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&ready, b"ready\n");
    callbacks
        .prepare(ssh::callback::endpoint(&url).unwrap().unwrap())
        .await
        .unwrap();
    let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    socket
        .write_all(b"GET /callback?code=fixture&state=nonce HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    let mut response = String::new();
    timeout(
        Duration::from_secs(10),
        socket.read_to_string(&mut response),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(response.ends_with("logged-in"), "{response}");
    drop(socket);
    drop(callbacks);
    timeout(Duration::from_secs(3), async {
        loop {
            if TcpListener::bind(("127.0.0.1", port)).await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        session
            .execute("printf still-connected", None)
            .await
            .unwrap(),
        "still-connected"
    );
    session.close().await;
}

#[tokio::test]
#[ignore = "Requires npm run test:remote"]
async fn native_x11_and_wayland_clients_fetch_png_on_demand() {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use sha2::{Digest, Sha256};
    let png = STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aB1sAAAAASUVORK5CYII=").unwrap();
    let expected: String = Sha256::digest(&png)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let session = ExecSession::connect(&server()).await.unwrap();
    crate::agent::install(&session).await.unwrap();
    let mut stream = session
        .stream(&format!(
            "exec ~/.local/bin/newport-agent serve {} --clipboard",
            Uuid::new_v4()
        ))
        .await
        .unwrap();
    receive(&mut stream, b'R').await;
    for (revision, client) in [
        (
            200i64,
            "/usr/bin/xclip -selection clipboard -t image/png -o",
        ),
        (201, "/usr/bin/wl-paste --type image/png --no-newline"),
    ] {
        frame(
            &mut stream,
            b'M',
            &rmp_serde::to_vec(&(revision, vec!["image/png"])).unwrap(),
        )
        .await;
        receive(&mut stream, b'A').await;
        let command = format!("eval \"$(~/.local/bin/newport-agent env)\"; {client} | sha256sum");
        let (output, ()) = tokio::join!(session.execute(&command, None), async {
            let (id, received_revision, format): (u64, i64, String) =
                rmp_serde::from_slice(&receive(&mut stream, b'C').await).unwrap();
            assert_eq!(received_revision, revision);
            assert_eq!(format, "image/png");
            let response =
                rmp_serde::to_vec(&(id, revision, 0u8, true, serde_bytes::Bytes::new(&png)))
                    .unwrap();
            frame(&mut stream, b'D', &response).await;
        });
        assert_eq!(
            output.unwrap().split_whitespace().next().unwrap(),
            expected,
            "{client}"
        );
    }
    frame(&mut stream, b'Q', &[]).await;
    timeout(Duration::from_secs(10), stream.read_to_end(&mut Vec::new()))
        .await
        .unwrap()
        .unwrap();
    session.close().await;
}
