//! Fault injection against the disposable server; never touches saved profiles.
use super::*;
use std::sync::Arc;
type Stream = russh::ChannelStream<russh::client::Msg>;

async fn agent(session: &ExecSession) -> Stream {
    // Explicit fixture setup, separate from the production feature launcher.
    crate::agent::install(session).await.unwrap();
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
    let hello: crate::agent::control::Hello =
        crate::agent::control::decode(&receive(&mut stream, b'R').await).unwrap();
    hello.validate().unwrap();
    let permissions = crate::agent::control::Permissions {
        revision: 1,
        clipboard: true,
        browser: true,
    };
    frame(
        &mut stream,
        b'P',
        &crate::agent::control::encode(&permissions).unwrap(),
    )
    .await;
    let applied: crate::agent::control::Applied =
        crate::agent::control::decode(&receive(&mut stream, b'A').await).unwrap();
    assert_eq!(applied.permissions, permissions);
    assert!(applied.error.is_none(), "{:?}", applied.error);
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
#[ignore = "Helper process for backend_crash_recovers_download_staging"]
async fn download_journal_crash_child() {
    let Ok(directory) = std::env::var("NEWPORT_DOWNLOAD_CRASH_DIRECTORY") else {
        return;
    };
    let profile = std::path::Path::new(&directory);
    let server: Server =
        serde_json::from_str(&std::env::var("NEWPORT_DOWNLOAD_CRASH_SERVER").unwrap()).unwrap();
    assert_eq!(server.ssh_host, "127.0.0.1");
    let operations = crate::files::Operations::persistent(profile);
    let sftp = Sftp::connect(&server).await.unwrap();
    crate::files::transfer::download(
        &operations,
        &sftp,
        &std::env::var("NEWPORT_DOWNLOAD_CRASH_REMOTE").unwrap(),
        &profile.join("downloads/existing.bin"),
        |done, _| {
            if done > 0 {
                // Tokio's local write can still be on its blocking pool. Wait
                // for bytes to reach the actual file before killing the process.
                let limit = std::time::Instant::now() + Duration::from_secs(5);
                while !std::fs::read_dir(profile.join("downloads"))
                    .unwrap()
                    .filter_map(Result::ok)
                    .any(|entry| {
                        entry
                            .file_name()
                            .to_string_lossy()
                            .starts_with(".newport-download-")
                            && entry.metadata().unwrap().len() > 4
                    })
                {
                    assert!(std::time::Instant::now() < limit);
                    std::thread::sleep(Duration::from_millis(10));
                }
                std::fs::write(profile.join("ready"), b"ready").unwrap();
                loop {
                    std::thread::park();
                }
            }
        },
    )
    .await
    .unwrap();
    panic!("download must be killed before publication");
}

#[tokio::test]
#[ignore = "Requires disposable OpenSSH fixture; kills only its own child test process"]
async fn backend_crash_recovers_download_staging() {
    let server = server();
    let sftp = Arc::new(Sftp::connect(&server).await.unwrap());
    let profile = tempfile::tempdir().unwrap();
    let folder = profile.path().join("downloads");
    std::fs::create_dir(&folder).unwrap();
    let destination = folder.join("existing.bin");
    std::fs::write(&destination, b"original destination").unwrap();
    let unrelated = folder.join(format!(".newport-download-{}", Uuid::new_v4()));
    std::fs::write(&unrelated, b"keep").unwrap();
    let data = vec![0x5a; 512 * 1024];
    let source = profile
        .path()
        .join(format!("download-crash-{}.bin", Uuid::new_v4()));
    std::fs::write(&source, &data).unwrap();
    let remote = crate::files::transfer::upload(
        &crate::files::Operations::default(),
        sftp.clone(),
        &source,
        "/home/fixture",
        |_, _| {},
    )
    .await
    .unwrap();
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut child = Child(
        std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "remote_tests::recovery::download_journal_crash_child",
                "--nocapture",
            ])
            .env("NEWPORT_DOWNLOAD_CRASH_DIRECTORY", profile.path())
            .env(
                "NEWPORT_DOWNLOAD_CRASH_SERVER",
                serde_json::to_string(&server).unwrap(),
            )
            .env("NEWPORT_DOWNLOAD_CRASH_REMOTE", &remote)
            .spawn()
            .unwrap(),
    );
    timeout(Duration::from_secs(15), async {
        while !profile.path().join("ready").exists() {
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "download helper exited before its first chunk"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    let staging = std::fs::read_dir(&folder)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path != &destination && path != &unrelated)
        .unwrap();
    assert!(std::fs::metadata(&staging).unwrap().len() > 0);
    assert_eq!(
        std::fs::read(&destination).unwrap(),
        b"original destination"
    );
    let operations = crate::files::Operations::persistent(profile.path());
    assert!(!staging.exists());
    assert_eq!(
        std::fs::read(&destination).unwrap(),
        b"original destination"
    );
    assert_eq!(std::fs::read(&unrelated).unwrap(), b"keep");
    let journal = profile
        .path()
        .join("download-recovery.json.servers")
        .join(sftp.server_id().to_string())
        .join("journal.json");
    let records: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&journal).unwrap()).unwrap();
    assert!(records["entries"].as_object().unwrap().is_empty());
    // A separate intentional retry publishes complete data and retires recovery.
    crate::files::transfer::download(&operations, &sftp, &remote, &destination, |_, _| {})
        .await
        .unwrap();
    assert_eq!(std::fs::read(&destination).unwrap(), data);
    assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 2);
    let records: serde_json::Value =
        serde_json::from_slice(&std::fs::read(journal).unwrap()).unwrap();
    assert!(records["entries"].as_object().unwrap().is_empty());
    sftp.session.remove(remote).await.unwrap();
}

// Invoked by the parent crash test only. Blocking at the first written chunk
// makes the crash deterministic without depending on network speed/file size.
#[tokio::test]
#[ignore = "Helper process for backend_crash_recovers_upload_staging"]
async fn upload_journal_crash_child() {
    let Ok(directory) = std::env::var("NEWPORT_UPLOAD_CRASH_DIRECTORY") else {
        return;
    };
    let profile = std::path::Path::new(&directory);
    let server: Server =
        serde_json::from_str(&std::env::var("NEWPORT_UPLOAD_CRASH_SERVER").unwrap()).unwrap();
    assert_eq!(server.ssh_host, "127.0.0.1");
    let operations = crate::files::Operations::persistent(profile);
    let sftp = Arc::new(Sftp::connect(&server).await.unwrap());
    crate::files::transfer::upload(
        &operations,
        sftp,
        &profile.join("payload.bin"),
        &std::env::var("NEWPORT_UPLOAD_CRASH_FOLDER").unwrap(),
        |done, _| {
            if done > 0 {
                std::fs::write(profile.join("ready"), b"ready").unwrap();
                loop {
                    std::thread::park();
                }
            }
        },
    )
    .await
    .unwrap();
    panic!("upload must be killed before publication");
}

#[tokio::test]
#[ignore = "Requires disposable OpenSSH fixture; kills only its own child test process"]
async fn backend_crash_recovers_upload_staging() {
    let server = server();
    let session = ExecSession::connect(&server).await.unwrap();
    let folder = format!("/home/fixture/upload-crash-{}", Uuid::new_v4());
    let unrelated = format!("{folder}/.newport-upload-{}", Uuid::new_v4());
    session
        .execute(&format!("mkdir {folder}; printf keep > {unrelated}"), None)
        .await
        .unwrap();
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("payload.bin"), vec![0x5a; 512 * 1024]).unwrap();
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut child = Child(
        std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "remote_tests::recovery::upload_journal_crash_child",
                "--nocapture",
            ])
            .env("NEWPORT_UPLOAD_CRASH_DIRECTORY", temp.path())
            .env(
                "NEWPORT_UPLOAD_CRASH_SERVER",
                serde_json::to_string(&server).unwrap(),
            )
            .env("NEWPORT_UPLOAD_CRASH_FOLDER", &folder)
            .spawn()
            .unwrap(),
    );
    timeout(Duration::from_secs(15), async {
        while !temp.path().join("ready").exists() {
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "upload helper exited before its first chunk"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    let journal = temp
        .path()
        .join("upload-recovery.json.servers")
        .join(server.id.to_string())
        .join("journal.json");
    let records: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&journal).unwrap()).unwrap();
    let entries = records["entries"].as_object().unwrap();
    assert_eq!(entries.len(), 1);
    let owned = entries.values().next().unwrap()["path"].as_str().unwrap();
    let sftp = Arc::new(Sftp::connect(&server).await.unwrap());
    assert!(sftp.session.stat(owned).await.unwrap().attrs.size.unwrap() > 0);
    assert!(
        sftp.session
            .stat(format!("{folder}/payload.bin"))
            .await
            .is_err()
    );
    let operations = crate::files::Operations::persistent(temp.path());
    operations.retry_upload_cleanup(sftp.clone());
    timeout(Duration::from_secs(8), async {
        loop {
            if let Err(error) = sftp.session.lstat(owned).await {
                assert!(
                    matches!(error, russh_sftp::client::error::Error::Status(ref status)
                    if status.status_code == russh_sftp::protocol::StatusCode::NoSuchFile)
                );
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    operations.shutdown().await;
    assert_eq!(
        sftp.session.stat(&unrelated).await.unwrap().attrs.size,
        Some(4)
    );
    let records: serde_json::Value =
        serde_json::from_slice(&std::fs::read(journal).unwrap()).unwrap();
    assert!(records["entries"].as_object().unwrap().is_empty());
    // Recovery does not replay the upload. A separate deliberate retry succeeds.
    let retry = crate::files::Operations::persistent(temp.path());
    let uploaded = crate::files::transfer::upload(
        &retry,
        sftp,
        &temp.path().join("payload.bin"),
        &folder,
        |_, _| {},
    )
    .await
    .unwrap();
    assert_eq!(uploaded, format!("{folder}/payload.bin"));
    let records: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            temp.path()
                .join("upload-recovery.json.servers")
                .join(server.id.to_string())
                .join("journal.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(records["entries"].as_object().unwrap().is_empty());
    session
        .execute(&format!("rm -r {folder}"), None)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "Requires cargo xtask test remote; replaces the disposable server agent"]
async fn services_require_explicit_install_and_preserve_damaged_installations() {
    let session = ExecSession::connect(&server()).await.unwrap();
    let services = || {
        [
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
        ]
    };
    for (service, after_install) in services().into_iter().zip(services()) {
        session
            .execute("rm -f ~/.local/bin/newport-agent", None)
            .await
            .unwrap();
        let error = crate::agent::launch(&session, service)
            .await
            .err()
            .expect("Missing agent must not be deployed by a feature");
        assert!(error.contains("not installed"), "{error}");
        session
            .execute("test ! -e ~/.local/bin/newport-agent", None)
            .await
            .unwrap();
        crate::agent::install(&session).await.unwrap();
        let git = matches!(after_install, crate::agent::Service::Git);
        let (mut stream, _) = crate::agent::launch(&session, after_install).await.unwrap();
        if git {
            let (client, _) =
                crate::git::client::Client::start_with_identity(stream, Uuid::new_v4().to_string())
                    .await
                    .unwrap();
            drop(client);
        } else {
            let hello: crate::agent::control::Hello =
                crate::agent::control::decode(&receive(&mut stream, b'R').await).unwrap();
            hello.validate().unwrap();
            stop(&mut stream).await;
        }
    }
    // Damage only the fixture entry point, never the retained immutable build.
    session
        .execute("cp -L ~/.local/bin/newport-agent ~/.local/bin/.damaged && chmod 600 ~/.local/bin/.damaged && mv -f ~/.local/bin/.damaged ~/.local/bin/newport-agent", None)
        .await
        .unwrap();
    assert!(
        crate::agent::launch(&session, crate::agent::Service::Git)
            .await
            .is_err()
    );
    let error = crate::agent::install(&session).await.unwrap_err();
    assert!(error.contains("not executable"), "{error}");
    session
        .execute("test ! -x ~/.local/bin/newport-agent", None)
        .await
        .unwrap();
    // Explicit fixture recovery: remove the damaged entry point, and
    // installation relinks the retained build without uploading it again.
    session
        .execute("rm ~/.local/bin/newport-agent", None)
        .await
        .unwrap();
    let mut stream = agent(&session).await;
    stop(&mut stream).await;
    let inode = session
        .execute("stat -Lc %i ~/.local/bin/newport-agent", None)
        .await
        .unwrap();
    let mut stream = agent(&session).await;
    stop(&mut stream).await;
    assert_eq!(
        session
            .execute("stat -Lc %i ~/.local/bin/newport-agent", None)
            .await
            .unwrap(),
        inode,
        "a matching executable must not be uploaded again"
    );
    crate::agent_lifecycle::activate_variant_test(&session).await;
    let mut stream = agent(&session).await;
    stop(&mut stream).await;
    session.close().await;
}

#[tokio::test]
#[ignore = "Requires cargo xtask test remote"]
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
#[ignore = "Requires cargo xtask test remote"]
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
#[ignore = "Requires cargo xtask test remote"]
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
#[ignore = "Requires cargo xtask test remote"]
async fn cancelled_upload_cleans_staging_and_can_be_retried() {
    use crate::files::{Operations, transfer};
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
    transfer::download(
        &crate::files::Operations::default(),
        &sftp,
        &remote,
        &downloaded,
        |_, _| {},
    )
    .await
    .unwrap();
    assert_eq!(std::fs::read(downloaded).unwrap(), data);
    session
        .execute(&format!("rm -r {folder}"), None)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "Requires cargo xtask test remote"]
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
    let profile = tempfile::tempdir().unwrap();
    let operations = crate::files::Operations::persistent(profile.path());
    {
        let download = transfer::download(&operations, &sftp, &remote, &destination, |done, _| {
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
    // Cancellation releases the transfer immediately; its owned disk worker
    // removes staging and retires the record without blocking the executor.
    operations.shutdown().await;
    assert_eq!(std::fs::read(&destination).unwrap(), b"keep existing file");
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
    let records: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            profile
                .path()
                .join("download-recovery.json.servers")
                .join(sftp.server_id().to_string())
                .join("journal.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(records["entries"].as_object().unwrap().is_empty());
    sftp.session.remove(remote).await.unwrap();
}

#[tokio::test]
#[ignore = "Requires cargo xtask test remote"]
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
    // A distinct executable build exercises replacement through the actual
    // activation protocol. This is not an old-release compatibility assertion.
    crate::agent_lifecycle::activate_variant_test(&session).await;
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
#[ignore = "Requires cargo xtask test remote"]
async fn discovery_and_unavailable_capabilities_report_real_server_state() {
    let server = super::installed_server(
        "NEWPORT_TEST_SSH_PORT",
        "Disposable capabilities",
        "fixture",
    )
    .await;
    let ports = crate::ports::discover(&crate::remote_tests::owner(&server))
        .await
        .unwrap();
    assert!(ports.iter().any(|p| p.port == 22));
    assert!(
        ports
            .iter()
            .any(|p| p.port == 8080 && p.address == "127.0.0.1")
    );
    // No Docker in this fixture: the Containers tab says so in the agreed words.
    let error = crate::containers::snapshot(&crate::remote_tests::owner(&server))
        .await
        .unwrap_err();
    assert_eq!(
        error,
        "Docker isn't installed or isn't running on this server."
    );
    // No systemd in this fixture: the Services tab says so instead of an empty list.
    let error = crate::services::snapshot(&crate::remote_tests::owner(&server))
        .await
        .unwrap_err();
    assert!(error.contains("Cannot read system services"), "{error}");
}

#[tokio::test]
#[ignore = "Requires cargo xtask test remote"]
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
#[ignore = "Requires cargo xtask test remote"]
async fn persistent_tunnel_reconnects_after_real_ssh_disconnect_and_stop_cancels_retry() {
    use crate::{
        manager::Manager,
        model::{Config, Status},
        profile::Store,
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
    manager.connect(tunnel.id).unwrap();
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
#[ignore = "Requires cargo xtask test remote"]
async fn changing_local_file_during_upload_does_not_publish_partial_content() {
    use crate::files::{Operations, transfer};
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
#[ignore = "Requires cargo xtask test remote"]
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
    let operations = crate::files::Operations::default();
    let mut download = Box::pin(transfer::download(
        &operations,
        &sftp,
        &remote,
        &local,
        |done, _| {
            if done > 0 {
                progressed.notify_one();
            }
        },
    ));
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
