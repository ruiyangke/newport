//! Fault injection against the disposable server; never touches saved profiles.
use super::*;
use std::sync::Arc;
type Stream = russh::ChannelStream<russh::client::Msg>;

async fn agent(session: &ExecSession) -> Stream {
    let (mut stream, _) = crate::agent::launch(
        session,
        crate::agent::Service::Integration {
            client: Uuid::new_v4(),
            clipboard: true,
            browser: true,
        },
    )
    .await
    .unwrap();
    assert_eq!(receive(&mut stream, b'R').await, b"newport-agent/6");
    stream
}
async fn offer(stream: &mut Stream, revision: i64, formats: &str) {
    frame(
        stream,
        b'M',
        &rmp_serde::to_vec(&(revision, formats.lines().collect::<Vec<_>>())).unwrap(),
    )
    .await;
    receive(stream, b'A').await;
}
async fn reply(stream: &mut Stream, request: &[u8], done: bool, data: &[u8]) {
    let (id, revision, _): (u64, i64, String) = rmp_serde::from_slice(request).unwrap();
    let bytes =
        rmp_serde::to_vec(&(id, revision, 0u8, done, serde_bytes::Bytes::new(data))).unwrap();
    frame(stream, b'D', &bytes).await;
}
async fn stop(stream: &mut Stream) {
    frame(stream, b'Q', &[]).await;
    timeout(Duration::from_secs(10), stream.read_to_end(&mut Vec::new()))
        .await
        .unwrap()
        .unwrap();
}
const READ: &str = "~/.local/bin/xclip -selection clipboard -o";

#[tokio::test]
#[ignore = "Requires npm run test:remote; replaces the disposable server agent"]
async fn every_service_automatically_installs_and_repairs_the_agent() {
    let session = ExecSession::connect(&server()).await.unwrap();
    for service in [
        crate::agent::Service::Git,
        crate::agent::Service::Integration {
            client: Uuid::new_v4(),
            clipboard: true,
            browser: false,
        },
        crate::agent::Service::Integration {
            client: Uuid::new_v4(),
            clipboard: false,
            browser: true,
        },
    ] {
        session
            .execute("rm -f ~/.local/bin/newport-agent", None)
            .await
            .unwrap();
        let git = matches!(service, crate::agent::Service::Git);
        let (mut stream, _) = crate::agent::launch(&session, service).await.unwrap();
        if git {
            let (client, _) =
                crate::git::client::Client::start_with_identity(stream, Uuid::new_v4().to_string())
                    .await
                    .unwrap();
            drop(client);
        } else {
            assert_eq!(receive(&mut stream, b'R').await, b"newport-agent/6");
            stop(&mut stream).await;
        }
    }
    session
        .execute("chmod 600 ~/.local/bin/newport-agent", None)
        .await
        .unwrap();
    let mut stream = agent(&session).await;
    stop(&mut stream).await;
    let inode = session
        .execute("stat -c %i ~/.local/bin/newport-agent", None)
        .await
        .unwrap();
    let mut stream = agent(&session).await;
    stop(&mut stream).await;
    assert_eq!(
        session
            .execute("stat -c %i ~/.local/bin/newport-agent", None)
            .await
            .unwrap(),
        inode,
        "a matching executable must not be uploaded again"
    );
    session
        .execute(
            "printf '#!/bin/sh\\necho newport-agent/4\\n' > ~/.local/bin/newport-agent",
            None,
        )
        .await
        .unwrap();
    let mut stream = agent(&session).await;
    stop(&mut stream).await;
    session.close().await;
}

#[tokio::test]
#[ignore = "Requires npm run test:remote"]
async fn clipboard_copy_during_transfer_rejects_late_old_response() {
    let session = ExecSession::connect(&server()).await.unwrap();
    let mut stream = agent(&session).await;
    offer(&mut stream, 1, "text/plain").await;
    let (old, ()) = tokio::join!(session.execute(READ, None), async {
        let request = receive(&mut stream, b'C').await;
        reply(&mut stream, &request, false, b"old prefix").await;
        offer(&mut stream, 2, "text/plain").await;
        reply(&mut stream, &request, true, b"old suffix").await;
    });
    assert!(
        old.is_err(),
        "superseded read must fail, not return old bytes"
    );
    let (current, ()) = tokio::join!(session.execute(READ, None), async {
        let request = receive(&mut stream, b'C').await;
        assert_eq!(
            rmp_serde::from_slice::<(u64, i64, String)>(&request)
                .unwrap()
                .1,
            2
        );
        reply(&mut stream, &request, true, b"latest clipboard").await;
    });
    assert_eq!(current.unwrap(), "latest clipboard");
    // Clearing the clipboard also invalidates a successfully cached read.
    offer(&mut stream, 3, "").await;
    assert!(session.execute(READ, None).await.is_err());
    stop(&mut stream).await;
}

#[tokio::test]
#[ignore = "Requires npm run test:remote"]
async fn clipboard_concurrent_readers_share_one_request() {
    let session = ExecSession::connect(&server()).await.unwrap();
    let mut stream = agent(&session).await;
    offer(&mut stream, 10, "text/plain").await;
    let (a, b, ()) = tokio::join!(
        session.execute(READ, None),
        session.execute(READ, None),
        async {
            let request = receive(&mut stream, b'C').await;
            reply(&mut stream, &request, true, b"shared").await;
        }
    );
    assert_eq!(a.unwrap(), "shared");
    assert_eq!(b.unwrap(), "shared");
    frame(&mut stream, b'H', &[]).await;
    // Any duplicate request left in the stream would fail this assertion.
    receive(&mut stream, b'A').await;
    stop(&mut stream).await;
}

#[tokio::test]
#[ignore = "Requires npm run test:remote"]
async fn crashed_agent_restarts_without_serving_old_clipboard() {
    let session = ExecSession::connect(&server()).await.unwrap();
    crate::agent::install(&session).await.unwrap();
    let pid_file = format!("/home/fixture/agent-{}.pid", Uuid::new_v4());
    let mut stream = session
        .stream(&format!(
            "echo $$ > {pid_file}; exec ~/.local/bin/newport-agent serve {} --clipboard",
            Uuid::new_v4()
        ))
        .await
        .unwrap();
    receive(&mut stream, b'R').await;
    offer(&mut stream, 1, "text/plain").await;
    let (read, ()) = tokio::join!(session.execute(READ, None), async {
        let request = receive(&mut stream, b'C').await;
        reply(&mut stream, &request, true, b"before crash").await;
    });
    assert_eq!(read.unwrap(), "before crash");
    session
        .execute(
            &format!("kill -KILL $(cat {pid_file}); rm {pid_file}"),
            None,
        )
        .await
        .unwrap();
    timeout(Duration::from_secs(5), stream.read_to_end(&mut Vec::new()))
        .await
        .unwrap()
        .unwrap();
    let mut restarted = agent(&session).await;
    assert!(session.execute(READ, None).await.is_err());
    offer(&mut restarted, 2, "text/plain").await;
    let (read, ()) = tokio::join!(session.execute(READ, None), async {
        let request = receive(&mut restarted, b'C').await;
        reply(&mut restarted, &request, true, b"after crash").await;
    });
    assert_eq!(read.unwrap(), "after crash");
    stop(&mut restarted).await;
}

#[tokio::test]
#[ignore = "Requires npm run test:remote"]
async fn cancelled_upload_cleans_staging_and_can_be_retried() {
    use crate::files::{transfer, Operations};
    let session = ExecSession::connect(&server()).await.unwrap();
    let folder = format!("/home/fixture/upload-{}", Uuid::new_v4());
    session
        .execute(&format!("mkdir {folder}"), None)
        .await
        .unwrap();
    let sftp = Arc::new(Sftp::connect(&server()).await.unwrap());
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("payload.bin");
    let data = vec![0x5a; 4 * 1024 * 1024];
    std::fs::write(&source, &data).unwrap();
    let operations = Operations::default();
    let progressed = tokio::sync::Notify::new();
    // Dropping the future is the same cancellation mechanism used by Operations.
    {
        let upload = transfer::upload(&operations, sftp.clone(), &source, &folder, |done, _| {
            if done > 0 {
                progressed.notify_one();
            }
        });
        tokio::select! {
            biased;
            _ = progressed.notified() => {},
            result = upload => panic!("upload finished before cancellation: {result:?}"),
        }
    }
    operations.shutdown().await;
    assert_eq!(
        session
            .execute(&format!("find {folder} -mindepth 1 | wc -l"), None)
            .await
            .unwrap()
            .trim(),
        "0"
    );
    let retry = Operations::default();
    let remote = transfer::upload(&retry, sftp.clone(), &source, &folder, |_, _| {})
        .await
        .unwrap();
    let downloaded = temp.path().join("download.bin");
    transfer::download(&sftp, &remote, &downloaded, |_, _| {})
        .await
        .unwrap();
    assert_eq!(std::fs::read(downloaded).unwrap(), data);
    session
        .execute(&format!("rm -r {folder}"), None)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "Requires npm run test:remote"]
async fn cancelled_download_preserves_destination_and_removes_partial_file() {
    use crate::files::transfer;
    let session = ExecSession::connect(&server()).await.unwrap();
    let remote = format!("/home/fixture/download-{}.bin", Uuid::new_v4());
    session
        .execute(&format!("truncate -s 8388608 {remote}"), None)
        .await
        .unwrap();
    let sftp = Sftp::connect(&server()).await.unwrap();
    let temp = tempfile::tempdir().unwrap();
    let destination = temp.path().join("existing.bin");
    std::fs::write(&destination, b"keep existing file").unwrap();
    let progressed = tokio::sync::Notify::new();
    {
        let download = transfer::download(&sftp, &remote, &destination, |done, _| {
            if done > 0 {
                progressed.notify_one();
            }
        });
        tokio::select! {
            biased;
            _ = progressed.notified() => {},
            result = download => panic!("download finished before cancellation: {result:?}"),
        }
    }
    assert_eq!(std::fs::read(&destination).unwrap(), b"keep existing file");
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
    sftp.session.remove(remote).await.unwrap();
}

#[tokio::test]
#[ignore = "Requires npm run test:remote"]
async fn agent_upgrade_and_bad_upload_preserve_working_installation() {
    let session = ExecSession::connect(&server()).await.unwrap();
    crate::agent::install(&session).await.unwrap();
    let hash = session
        .execute("sha256sum ~/.local/bin/newport-agent", None)
        .await
        .unwrap();
    let command = format!(
        "sh -c '{}' newport-install {}",
        include_str!("../agent-install.sh").replace('\'', "'\"'\"'"),
        "0".repeat(64)
    );
    let error = session
        .execute(&command, Some(b"incomplete upload"))
        .await
        .unwrap_err();
    assert!(error.contains("checksum mismatch"), "{error}");
    assert_eq!(
        session
            .execute("sha256sum ~/.local/bin/newport-agent", None)
            .await
            .unwrap(),
        hash
    );
    assert_eq!(
        session
            .execute("find ~/.local/bin -name '.newport-agent.*' | wc -l", None)
            .await
            .unwrap()
            .trim(),
        "0"
    );
    // A managed older-version stub exercises version replacement, without
    // pretending this is compatibility testing against an actual old release.
    session
        .execute(
            "printf '#!/bin/sh\necho newport-agent/4\n' > ~/.local/bin/newport-agent",
            None,
        )
        .await
        .unwrap();
    crate::agent::install(&session).await.unwrap();
    assert_eq!(
        session
            .execute("sha256sum ~/.local/bin/newport-agent", None)
            .await
            .unwrap(),
        hash
    );
    let mut stream = agent(&session).await;
    stop(&mut stream).await;
}

#[tokio::test]
#[ignore = "Requires npm run test:remote"]
async fn discovery_and_unavailable_capabilities_report_real_server_state() {
    let server = server();
    let ports = crate::ports::discover(&server).await.unwrap();
    assert!(ports.iter().any(|p| p.port == 22));
    assert!(ports
        .iter()
        .any(|p| p.port == 8080 && p.address == "127.0.0.1"));
    for section in [
        crate::cockpit::Section::Services,
        crate::cockpit::Section::Containers,
    ] {
        let error = crate::cockpit::collect(&server, section).await.unwrap_err();
        assert!(error.contains("Remote command failed"), "{error}");
    }
}

#[tokio::test]
#[ignore = "Requires npm run test:remote"]
async fn disk_full_reports_retryable_error_and_recovers_after_space_is_freed() {
    let session = ExecSession::connect(&server()).await.unwrap();
    crate::agent::install(&session).await.unwrap();
    let home = format!("/fault-disk/home-{}", Uuid::new_v4());
    let filler = format!("/fault-disk/fill-{}", Uuid::new_v4());
    session
        .execute(&format!("mkdir -m 700 {home}"), None)
        .await
        .unwrap();
    let command = format!(
        "HOME={home} /home/fixture/.local/bin/newport-agent serve {} --clipboard",
        Uuid::new_v4()
    );
    let mut stream = session.stream(&command).await.unwrap();
    receive(&mut stream, b'R').await;
    let output = session
        .execute(&format!("newport-test-fixture fill-disk {filler}"), None)
        .await
        .unwrap();
    assert_eq!(output.trim(), "ENOSPC");
    let mut archive = tar::Builder::new(Vec::new());
    let data = b"recovered clipboard";
    let mut header = tar::Header::new_gnu();
    header.set_size(data.len() as u64);
    header.set_mode(0o600);
    header.set_cksum();
    archive
        .append_data(&mut header, "text/plain", &data[..])
        .unwrap();
    let bytes = archive.into_inner().unwrap();
    frame(&mut stream, b'S', &bytes).await;
    let error = String::from_utf8(receive(&mut stream, b'T').await).unwrap();
    assert!(error.contains("No space left"), "{error}");
    timeout(Duration::from_secs(5), stream.read_to_end(&mut Vec::new()))
        .await
        .unwrap()
        .unwrap();
    session
        .execute(&format!("rm {filler}"), None)
        .await
        .unwrap();
    // Exercise the retry attempt against the same remote HOME after the fault.
    let mut stream = session.stream(&command).await.unwrap();
    receive(&mut stream, b'R').await;
    frame(&mut stream, b'S', &bytes).await;
    receive(&mut stream, b'A').await;
    assert_eq!(
        session
            .execute(
                &format!("HOME={home} /home/fixture/.local/bin/newport-agent clipboard -o"),
                None
            )
            .await
            .unwrap(),
        "recovered clipboard"
    );
    stop(&mut stream).await;
    session
        .execute(&format!("rm -r {home}"), None)
        .await
        .unwrap();
}

async fn wait_status(manager: &crate::manager::Manager, id: Uuid, expected: crate::model::Status) {
    timeout(Duration::from_secs(20), async {
        loop {
            let state = manager
                .runtime
                .lock()
                .unwrap()
                .tunnels
                .get(&id)
                .cloned()
                .unwrap_or_default();
            if state.status == expected {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "Expected {expected:?}, got {:?}",
            manager
                .runtime
                .lock()
                .unwrap()
                .tunnels
                .get(&id)
                .map(|state| state.status)
        )
    });
}
async fn http(port: u16) {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    stream
        .write_all(b"GET / HTTP/1.0\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    let mut response = String::new();
    timeout(Duration::from_secs(5), stream.read_to_string(&mut response))
        .await
        .unwrap()
        .unwrap();
    assert!(response.contains("200 OK") && response.contains("newport remote fixture"));
}

#[tokio::test]
#[ignore = "Requires npm run test:remote"]
async fn persistent_tunnel_reconnects_after_real_ssh_disconnect_and_stop_cancels_retry() {
    use crate::{
        config::Store,
        manager::Manager,
        model::{Config, Status},
    };
    let server = server();
    let temp = tempfile::tempdir().unwrap();
    let reservation = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = reservation.local_addr().unwrap().port();
    drop(reservation);
    let tunnel: Tunnel = serde_json::from_value(serde_json::json!({
        "id":Uuid::new_v4(), "serverId":server.id, "name":"Reconnect test", "localPort":port,
        "remoteHost":"127.0.0.1", "remotePort":8080, "autoReconnect":true
    }))
    .unwrap();
    let mut manager = Manager::new(Store::for_test(temp.path().into()));
    manager
        .save(Config {
            servers: vec![server.clone()],
            tunnels: vec![tunnel.clone()],
        })
        .await
        .unwrap();
    manager.connect(tunnel.id).await.unwrap();
    wait_status(&manager, tunnel.id, Status::Connected).await;
    http(port).await;
    // Only this disposable account's SSH worker processes; the root listener lives.
    let disconnect = "pkill -KILL -u $(id -u) -x 'sshd(-session)?'";
    let _ = tokio::join!(
        ssh::execute(&server, disconnect, None),
        wait_status(&manager, tunnel.id, Status::Reconnecting)
    );
    wait_status(&manager, tunnel.id, Status::Connected).await;
    http(port).await;
    let _ = tokio::join!(
        ssh::execute(&server, disconnect, None),
        wait_status(&manager, tunnel.id, Status::Reconnecting)
    );
    manager.disconnect(tunnel.id).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(
        manager.runtime.lock().unwrap().tunnels[&tunnel.id].status,
        Status::Disconnected
    );
    let _released = TcpListener::bind(("127.0.0.1", port)).await.unwrap();
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "Requires npm run test:remote"]
async fn changing_local_file_during_upload_does_not_publish_partial_content() {
    use crate::files::{transfer, Operations};
    use std::{
        io::Write,
        sync::atomic::{AtomicBool, Ordering},
    };
    let session = ExecSession::connect(&server()).await.unwrap();
    let folder = format!("/home/fixture/changing-{}", Uuid::new_v4());
    session
        .execute(&format!("mkdir {folder}"), None)
        .await
        .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let local = temp.path().join("growing.bin");
    std::fs::write(&local, vec![1; 256 * 1024]).unwrap();
    let sftp = Arc::new(Sftp::connect(&server()).await.unwrap());
    let operations = Operations::default();
    let changed = AtomicBool::new(false);
    let error = transfer::upload(&operations, sftp, &local, &folder, |done, _| {
        if done > 0 && !changed.swap(true, Ordering::SeqCst) {
            std::fs::OpenOptions::new()
                .append(true)
                .open(&local)
                .unwrap()
                .write_all(b"new bytes")
                .unwrap();
        }
    })
    .await
    .unwrap_err();
    assert!(changed.load(Ordering::SeqCst));
    assert!(error.to_string().contains("changed"), "{error}");
    operations.shutdown().await;
    assert_eq!(
        session
            .execute(&format!("find {folder} -mindepth 1 | wc -l"), None)
            .await
            .unwrap()
            .trim(),
        "0"
    );
    session
        .execute(&format!("rmdir {folder}"), None)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "Requires npm run test:remote"]
async fn changing_remote_file_during_download_preserves_local_destination() {
    use crate::files::transfer;
    let session = ExecSession::connect(&server()).await.unwrap();
    let remote = format!("/home/fixture/changing-{}.bin", Uuid::new_v4());
    session
        .execute(&format!("truncate -s 8388608 {remote}"), None)
        .await
        .unwrap();
    let sftp = Sftp::connect(&server()).await.unwrap();
    let temp = tempfile::tempdir().unwrap();
    let local = temp.path().join("existing.bin");
    std::fs::write(&local, b"original bytes").unwrap();
    let progressed = tokio::sync::Notify::new();
    let mut download = Box::pin(transfer::download(&sftp, &remote, &local, |done, _| {
        if done > 0 {
            progressed.notify_one();
        }
    }));
    tokio::select! {
        biased;
        _ = progressed.notified() => {},
        result = &mut download => panic!("download finished before mutation: {result:?}"),
    }
    // The transfer future is paused while the remote mutation completes.
    session
        .execute(&format!("truncate -s 0 {remote}"), None)
        .await
        .unwrap();
    let error = download.await.unwrap_err();
    assert!(error.to_string().contains("changed"), "{error}");
    assert_eq!(std::fs::read(&local).unwrap(), b"original bytes");
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
    session
        .execute(&format!("rm {remote}"), None)
        .await
        .unwrap();
}
