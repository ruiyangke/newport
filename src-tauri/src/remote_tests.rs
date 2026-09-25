//! Real OpenSSH/Linux checks. Run with `npm run test:remote`.
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

fn server() -> Server {
    serde_json::from_value(serde_json::json!({
        "id": Uuid::new_v4(), "name": "Disposable Linux", "sshUser": "fixture",
        "sshHost": "127.0.0.1", "sshPort": std::env::var("PORTHOP_TEST_SSH_PORT").unwrap().parse::<u16>().unwrap(),
        "identityFile": std::env::var("PORTHOP_TEST_IDENTITY").unwrap()
    })).unwrap()
}

#[tokio::test]
#[ignore = "Requires scripts/test-remote.py and real OpenSSH"]
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
#[ignore = "Requires scripts/test-remote.py and real OpenSSH"]
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
    assert!(response.contains("200 OK") && response.contains("porthop remote fixture"));
    forwarding.shutdown().await;
    let _released = TcpListener::bind(("127.0.0.1", local)).await.unwrap();
}

#[tokio::test]
#[ignore = "Requires scripts/test-remote.py and real OpenSSH"]
async fn deploy_reinstall_and_agent_protocol() {
    let server = server();
    let session = ExecSession::connect(&server).await.unwrap();
    crate::agent::install(&session).await.unwrap();
    let hash = session
        .execute("sha256sum ~/.local/bin/porthop-agent", None)
        .await
        .unwrap();
    crate::agent::install(&session).await.unwrap();
    crate::agent::reinstall(&session).await.unwrap();
    assert_eq!(
        session
            .execute("sha256sum ~/.local/bin/porthop-agent", None)
            .await
            .unwrap(),
        hash
    );
    let mut stream = session
        .stream(&format!(
            "exec ~/.local/bin/porthop-agent serve {} --clipboard --browser",
            Uuid::new_v4()
        ))
        .await
        .unwrap();
    async fn event(stream: &mut (impl tokio::io::AsyncRead + Unpin), kind: u8) -> Vec<u8> {
        timeout(Duration::from_secs(10), async {
            let mut header = [0; 5];
            stream.read_exact(&mut header).await.unwrap();
            assert_eq!(header[0], kind);
            let size = u32::from_be_bytes(header[1..].try_into().unwrap()) as usize;
            assert!(size < 1024 * 1024);
            let mut data = vec![0; size];
            stream.read_exact(&mut data).await.unwrap();
            data
        })
        .await
        .unwrap()
    }
    assert_eq!(event(&mut stream, b'R').await, b"porthop-agent/5");
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
    stream.write_all(b"S").await.unwrap();
    stream
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .await
        .unwrap();
    stream.write_all(&bytes).await.unwrap();
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
            let request = String::from_utf8(event(&mut stream, b'O').await).unwrap();
            let (id, url) = request.split_once('\n').unwrap();
            assert_eq!(url, "https://example.com/login?code=fixture");
            let ack = format!("{id}\nok");
            stream.write_all(b"B").await.unwrap();
            stream
                .write_all(&(ack.len() as u32).to_be_bytes())
                .await
                .unwrap();
            stream.write_all(ack.as_bytes()).await.unwrap();
        }
    );
    opened.unwrap();
    stream.write_all(&[b'Q', 0, 0, 0, 0]).await.unwrap();
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
#[ignore = "Requires scripts/test-remote.py and real OpenSSH"]
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
#[ignore = "Requires scripts/test-remote.py and real OpenSSH"]
async fn changed_host_key_is_rejected_and_restoration_recovers() {
    struct Restore(std::path::PathBuf, Vec<u8>);
    impl Drop for Restore {
        fn drop(&mut self) {
            std::fs::write(&self.0, &self.1).unwrap();
        }
    }
    let path = std::path::PathBuf::from(std::env::var("PORTHOP_TEST_KNOWN_HOSTS").unwrap());
    let restore = Restore(path.clone(), std::fs::read(&path).unwrap());
    let wrong = std::fs::read_to_string(format!(
        "{}.pub",
        std::env::var("PORTHOP_TEST_IDENTITY").unwrap()
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
