//! Real-server permission, capability and resource-boundary checks.
use super::*;

#[tokio::test]
#[ignore = "Requires npm run test:remote"]
async fn destination_health_distinguishes_unavailable_service_from_broken_ssh() {
    use crate::model::DestinationStatus;
    let server = server();
    for (remote, expected) in [
        (8080, DestinationStatus::Reachable),
        (1, DestinationStatus::Unavailable),
    ] {
        let reservation = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local = reservation.local_addr().unwrap().port();
        drop(reservation);
        let tunnel: Tunnel = serde_json::from_value(serde_json::json!({
            "id":Uuid::new_v4(), "serverId":server.id, "name":"Health test", "localPort":local,
            "remoteHost":"127.0.0.1", "remotePort":remote
        }))
        .unwrap();
        let forwarding = Forwarding::start(&server, Some(&tunnel)).await.unwrap();
        timeout(Duration::from_secs(10), async {
            loop {
                let states = forwarding.destination_health();
                if states[0].status != DestinationStatus::Checking {
                    assert_eq!(states[0].status, expected);
                    assert!(states[0].checked_at.is_some());
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(
            forwarding.error().is_none(),
            "destination failure must not break SSH"
        );
        forwarding.shutdown().await;
        let _released = TcpListener::bind(("127.0.0.1", local)).await.unwrap();
    }
}

#[tokio::test]
#[ignore = "Requires npm run test:remote"]
async fn browser_only_rejects_clipboard_and_unsafe_urls_but_accepts_matching_ack() {
    let session = ExecSession::connect(&server()).await.unwrap();
    crate::agent::install(&session).await.unwrap();
    let mut stream = session
        .stream(&format!(
            "exec ~/.local/bin/porthop-agent serve {} --browser",
            Uuid::new_v4()
        ))
        .await
        .unwrap();
    receive(&mut stream, b'R').await;
    assert!(session
        .execute("~/.local/bin/xclip -selection clipboard -o", None)
        .await
        .is_err());
    for url in [
        "file:///etc/passwd",
        "javascript:alert(1)",
        "https://user:pass@example.com",
    ] {
        assert!(session
            .execute(&format!("~/.local/bin/xdg-open '{url}'"), None)
            .await
            .is_err());
    }
    let (opened, ()) = tokio::join!(
        session.execute(
            "~/.local/bin/xdg-open 'https://example.com/device?user_code=test'",
            None
        ),
        async {
            let request = String::from_utf8(receive(&mut stream, b'O').await).unwrap();
            let (id, url) = request.split_once('\n').unwrap();
            assert_eq!(url, "https://example.com/device?user_code=test");
            let wrong = id.parse::<u64>().unwrap() + 1;
            frame(
                &mut stream,
                b'B',
                format!("{wrong}\nwrong acknowledgement").as_bytes(),
            )
            .await;
            frame(&mut stream, b'H', &[]).await;
            receive(&mut stream, b'A').await;
            frame(&mut stream, b'B', format!("{id}\nok").as_bytes()).await;
        }
    );
    opened.unwrap();
    frame(&mut stream, b'Q', &[]).await;
    timeout(Duration::from_secs(5), stream.read_to_end(&mut Vec::new()))
        .await
        .unwrap()
        .unwrap();
    assert!(session
        .execute("~/.local/bin/xdg-open https://example.com", None)
        .await
        .is_err());
}

#[tokio::test]
#[ignore = "Requires npm run test:remote"]
async fn callback_leases_are_bounded_and_rollback_releases_port() {
    let session = ExecSession::connect(&server()).await.unwrap();
    let mut callbacks = session.callbacks();
    let mut ports = Vec::new();
    for _ in 0..8 {
        let reservation = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = reservation.local_addr().unwrap().port();
        drop(reservation);
        let url = url::Url::parse(&format!(
            "https://example.com/?redirect_uri=http://127.0.0.1:{port}/callback"
        ))
        .unwrap();
        callbacks
            .prepare(ssh::callback::endpoint(&url).unwrap().unwrap())
            .await
            .unwrap();
        ports.push(port);
    }
    let url = url::Url::parse("https://example.com/?redirect_uri=http://127.0.0.1:1455/callback")
        .unwrap();
    let error = callbacks
        .prepare(ssh::callback::endpoint(&url).unwrap().unwrap())
        .await
        .unwrap_err();
    assert!(error.contains("Too many"), "{error}");
    callbacks.rollback_last();
    let last = ports.pop().unwrap();
    timeout(Duration::from_secs(3), async {
        loop {
            if TcpListener::bind(("127.0.0.1", last)).await.is_ok() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    drop(callbacks);
    for port in ports {
        timeout(Duration::from_secs(3), async {
            loop {
                if TcpListener::bind(("127.0.0.1", port)).await.is_ok() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    assert_eq!(
        session.execute("printf alive", None).await.unwrap(),
        "alive"
    );
}

#[tokio::test]
#[ignore = "Requires npm run test:remote"]
async fn sftp_permission_failure_is_reported_without_harming_session() {
    use crate::files::{transfer, Operations};
    let server = server();
    let session = ExecSession::connect(&server).await.unwrap();
    let folder = format!("/home/fixture/readonly-{}", Uuid::new_v4());
    session
        .execute(&format!("mkdir -m 500 {folder}"), None)
        .await
        .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let local = temp.path().join("upload.txt");
    std::fs::write(&local, b"must not appear").unwrap();
    let sftp = std::sync::Arc::new(Sftp::connect(&server).await.unwrap());
    let operations = Operations::default();
    assert!(
        transfer::upload(&operations, sftp.clone(), &local, &folder, |_, _| {})
            .await
            .is_err()
    );
    assert!(sftp
        .session
        .stat(format!("{folder}/upload.txt"))
        .await
        .is_err());
    session
        .execute(&format!("chmod 700 {folder}"), None)
        .await
        .unwrap();
    let uploaded = transfer::upload(&operations, sftp.clone(), &local, &folder, |_, _| {})
        .await
        .unwrap();
    assert_eq!(
        sftp.session.stat(uploaded).await.unwrap().attrs.size,
        Some(15)
    );
    session
        .execute(&format!("rm -r {folder}"), None)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "Requires npm run test:remote"]
async fn port_range_bind_conflict_rolls_back_without_stealing_existing_listener() {
    let (first, second) = loop {
        let first = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = first.local_addr().unwrap().port();
        if port == u16::MAX {
            continue;
        }
        if let Ok(second) = TcpListener::bind(("127.0.0.1", port + 1)).await {
            break (first, second);
        }
    };
    let port = first.local_addr().unwrap().port();
    let server = server();
    let tunnel: Tunnel = serde_json::from_value(serde_json::json!({
        "id":Uuid::new_v4(), "serverId":server.id, "name":"Range test",
        "localPort":port, "localPortEnd":port + 1,
        "remoteHost":"127.0.0.1", "remotePort":8080, "remotePortEnd":8081
    }))
    .unwrap();
    drop(first);
    assert!(Forwarding::start(&server, Some(&tunnel)).await.is_err());
    let released = TcpListener::bind(("127.0.0.1", port)).await.unwrap();
    let _connection = tokio::net::TcpStream::connect(("127.0.0.1", port + 1))
        .await
        .unwrap();
    timeout(Duration::from_secs(2), second.accept())
        .await
        .unwrap()
        .unwrap();
    drop(released);
    drop(second);
    let forwarding = Forwarding::start(&server, Some(&tunnel)).await.unwrap();
    assert_eq!(forwarding.destination_health().len(), 2);
    forwarding.shutdown().await;
    let _first = TcpListener::bind(("127.0.0.1", port)).await.unwrap();
    let _second = TcpListener::bind(("127.0.0.1", port + 1)).await.unwrap();
}

#[tokio::test]
#[ignore = "Requires npm run test:remote"]
async fn temporary_callback_does_not_reconnect_a_closed_ssh_session() {
    let server = server();
    let control = ExecSession::connect(&server).await.unwrap();
    let reservation = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = reservation.local_addr().unwrap().port();
    drop(reservation);
    // Keep the destination alive on a separate SSH connection throughout.
    let mut httpd = control
        .stream(&format!(
            r#"python3 -u -c '
from http.server import HTTPServer, SimpleHTTPRequestHandler
from functools import partial
s = HTTPServer(("127.0.0.1", {port}), partial(SimpleHTTPRequestHandler, directory="/srv/fixture"))
print("ready", flush=True)
s.serve_forever()
'"#
        ))
        .await
        .unwrap();
    let mut ready = [0; 6];
    timeout(Duration::from_secs(5), httpd.read_exact(&mut ready))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&ready, b"ready\n");
    let session = ExecSession::connect(&server).await.unwrap();
    let mut callbacks = session.callbacks();
    let url = url::Url::parse(&format!(
        "https://example.com/?redirect_uri=http://127.0.0.1:{port}/"
    ))
    .unwrap();
    callbacks
        .prepare(ssh::callback::endpoint(&url).unwrap().unwrap())
        .await
        .unwrap();
    async fn get(port: u16) -> std::io::Result<String> {
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
        stream
            .write_all(b"GET / HTTP/1.0\r\nHost: localhost\r\n\r\n")
            .await?;
        let mut response = String::new();
        stream.read_to_string(&mut response).await?;
        Ok(response)
    }
    assert!(timeout(Duration::from_secs(5), get(port))
        .await
        .unwrap()
        .unwrap()
        .contains("200 OK"));
    session.close().await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let response = timeout(Duration::from_secs(5), get(port)).await.unwrap();
    assert!(
        response.is_err() || response.unwrap().is_empty(),
        "callback must not reconnect"
    );
    let direct = control.execute(&format!("python3 -c 'import urllib.request; print(urllib.request.urlopen(\"http://127.0.0.1:{port}/\").status)'"), None).await.unwrap();
    assert_eq!(
        direct.trim(),
        "200",
        "the remote destination remains healthy"
    );
    drop(callbacks);
    control.close().await;
}

#[tokio::test]
#[ignore = "Requires npm run test:remote"]
async fn file_listing_and_preview_enforce_types_and_size_limits() {
    use crate::files;
    let server = server();
    let session = ExecSession::connect(&server).await.unwrap();
    let folder = format!("/home/fixture/preview-{}", Uuid::new_v4());
    session
        .execute(
            &format!(
                r#"python3 -c '
from pathlib import Path
p = Path("{folder}")
p.mkdir()
(p / "hello 世界.html").write_text("<script>inert 世界</script>")
(p / "binary.dat").write_bytes(bytes([0,255,1]))
with (p / "large.bin").open("wb") as f: f.truncate(17 * 1024 * 1024)
(p / "long.txt").write_text("x" * (1024 * 1024 + 1))
(p / "link.txt").symlink_to(p / "hello 世界.html")
(p / "broken").symlink_to(p / "missing")
'"#
            ),
            None,
        )
        .await
        .unwrap();
    let sftp = Sftp::connect(&server).await.unwrap();
    let list = serde_json::to_value(files::list(&sftp, &folder).await.unwrap()).unwrap();
    assert_eq!(list["entries"].as_array().unwrap().len(), 6);
    let preview = serde_json::to_value(
        files::preview(&sftp, &format!("{folder}/link.txt"))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(preview["kind"], "text");
    assert_eq!(preview["mime"], "text/plain");
    assert_eq!(preview["content"], "<script>inert 世界</script>");
    for (name, expected) in [
        ("binary.dat", "binary"),
        ("large.bin", "16 MiB"),
        ("long.txt", "1 MiB"),
    ] {
        let error = files::preview(&sftp, &format!("{folder}/{name}"))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains(expected), "{name}: {error}");
    }
    assert!(files::preview(&sftp, &folder).await.is_err());
    assert!(files::preview(&sftp, &format!("{folder}/broken"))
        .await
        .is_err());
    session
        .execute(&format!("rm -r {folder}"), None)
        .await
        .unwrap();
}
