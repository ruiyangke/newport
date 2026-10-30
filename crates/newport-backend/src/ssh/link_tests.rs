//! A server's link with the production parts, and commands and SFTP over it,
//! against the disposable SSH fixture.
use super::link_parts::{KnownHosts, link_with};
use crate::remote::{Sftp, command};
use newport_ssh::{Error, Link, Login, Open, Reach, TcpDialer};
use russh::keys::ssh_key::{HashAlg, PublicKey};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn fixture() -> Link {
    super::link(
        &super::integration_tests::server(),
        crate::ssh::no_jump_hosts,
    )
}

async fn eventually(what: &str, mut done: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !done() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
}

/// The first login failure, while the link may keep trying.
async fn login_failure(link: &Link) -> Error {
    let mut status = link.status();
    let open = link.open(Open::Session);
    tokio::pin!(open);
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            tokio::select! {
                result = &mut open => panic!("the request ended: {:?}", result.err()),
                changed = status.changed() => {
                    changed.unwrap();
                    let current = status.borrow_and_update();
                    match (&current.reach, &current.login) {
                        (Reach::Failed(error), _) | (_, Some(Login::Failed(error))) => {
                            return error.clone();
                        }
                        _ => {}
                    }
                }
            }
        }
    })
    .await
    .expect("a login to fail")
}

/// Why the link stopped trying to log in. Its request keeps waiting for a
/// Retry meanwhile.
async fn refused(link: &Link) -> Error {
    let mut status = link.status();
    let open = link.open(Open::Session);
    tokio::pin!(open);
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            tokio::select! {
                result = &mut open => panic!("the request ended: {:?}", result.err()),
                changed = status.changed() => {
                    changed.unwrap();
                    if let Reach::Failed(error) = &status.borrow_and_update().reach {
                        return error.clone();
                    }
                }
            }
        }
    })
    .await
    .expect("the login to be refused")
}

#[tokio::test]
#[ignore = "Run with cargo xtask test ssh"]
async fn a_command_returns_its_output_and_exit_status() {
    let link = fixture();
    assert_eq!(
        command::run(&link, "printf ok", None).await.unwrap(),
        command::Output {
            stdout: "ok".into(),
            stderr: String::new(),
            exit_code: 0,
        }
    );
    let failed = command::run(&link, "fail", None).await.unwrap();
    assert_eq!(
        (failed.exit_code, failed.stderr.as_str()),
        (7, "fixture failure")
    );
    let error = command::stdout(&link, "fail", None).await.unwrap_err();
    assert_eq!(
        error,
        command::Error::Failed {
            status: 7,
            details: "fixture failure".into()
        }
    );
    assert_eq!(
        error.to_string(),
        "Remote command failed (status 7): fixture failure"
    );
}

#[tokio::test]
#[ignore = "Run with cargo xtask test ssh"]
async fn a_command_reads_its_stdin() {
    let link = fixture();
    let input: Vec<u8> = (0..=255).cycle().take(5 * 1024 * 1024).collect();
    assert_eq!(
        command::stdout(&link, "stdin", Some(&input)).await.unwrap(),
        input.len().to_string()
    );
    assert_eq!(command::stdout(&link, "stdin", None).await.unwrap(), "0");
}

#[tokio::test]
#[ignore = "Run with cargo xtask test ssh"]
async fn output_past_the_cap_ends_the_command_but_not_the_link() {
    let link = fixture();
    assert_eq!(
        command::run(&link, "large", None).await,
        Err(command::Error::StdoutTooLarge)
    );
    assert_eq!(
        command::stdout(&link, "printf ok", None).await.unwrap(),
        "ok"
    );
}

#[tokio::test]
#[ignore = "Run with cargo xtask test ssh"]
async fn a_command_past_its_deadline_times_out() {
    let link = fixture();
    let started = std::time::Instant::now();
    assert_eq!(
        command::run_with_limit(&link, "stall-until-close", Duration::from_secs(1)).await,
        Err(command::Error::TimedOut)
    );
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(
        command::stdout(&link, "printf ok", None).await.unwrap(),
        "ok"
    );
}

#[tokio::test]
#[ignore = "Run with cargo xtask test ssh"]
async fn a_stream_carries_stdin_and_stdout() {
    let link = fixture();
    let mut stream = command::stream(&link, "stdin").await.unwrap().into_stream();
    stream.write_all(&[7; 100_000]).await.unwrap();
    stream.shutdown().await.unwrap();
    let mut output = String::new();
    tokio::time::timeout(Duration::from_secs(10), stream.read_to_string(&mut output))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(output, "100000");
}

#[tokio::test]
#[ignore = "Run with cargo xtask test ssh"]
async fn an_upload_sends_its_source_and_reports_progress() {
    use crate::agent_payload::TransferEncoding;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let link = fixture();
    let payload = crate::agent_payload::payload("x86_64").unwrap();
    for encoding in [TransferEncoding::Gzip, TransferEncoding::Raw] {
        let progress = AtomicUsize::new(0);
        let output = command::upload(&link, "stdin", payload.stream(encoding).unwrap(), &|sent| {
            progress.store(sent, Ordering::Relaxed)
        })
        .await
        .unwrap();
        assert_eq!(output, payload.transfer_size(encoding).to_string());
        assert_eq!(
            progress.load(Ordering::Relaxed),
            payload.transfer_size(encoding)
        );
    }
}

#[tokio::test]
#[ignore = "Run with cargo xtask test ssh"]
async fn the_shared_sftp_session_is_reused_and_a_dedicated_one_is_its_own() {
    let sftp = Sftp::new(fixture());
    let mut requests = tokio::task::JoinSet::new();
    for _ in 0..16 {
        let sftp = sftp.clone();
        requests.spawn(async move {
            let shared = sftp.shared().await.unwrap();
            assert!(!shared.realpath(".").await.unwrap().files.is_empty());
            shared
        });
    }
    let mut held = Vec::new();
    while let Some(shared) = requests.join_next().await {
        held.push(shared.unwrap().downgrade());
    }
    assert!(held.iter().all(|session| session.ptr_eq(&held[0])));
    let dedicated = sftp.dedicated().await.unwrap();
    let shared = sftp.shared().await.unwrap();
    assert!(!std::ptr::eq::<crate::remote::sftp::Session>(
        &dedicated, &*shared
    ));
    assert!(shared.downgrade().ptr_eq(&held[0]));
    dedicated.realpath(".").await.unwrap();
    shared.realpath(".").await.unwrap();
}

#[tokio::test]
#[ignore = "Run with cargo xtask test ssh"]
async fn the_shared_sftp_session_closes_when_idle_but_not_while_held() {
    let idle = Duration::from_millis(500);
    let sftp = Sftp::with_idle(fixture(), idle);
    let shared = sftp.shared().await.unwrap();
    let session = shared.downgrade();
    tokio::time::sleep(idle * 3).await;
    shared.realpath(".").await.unwrap();
    assert!(session.upgrade().is_some());
    drop(shared);
    eventually("the idle session to close", || session.upgrade().is_none()).await;
    let reopened = sftp.shared().await.unwrap();
    assert!(!reopened.downgrade().ptr_eq(&session));
    reopened.realpath(".").await.unwrap();
}

#[tokio::test]
#[ignore = "Run with cargo xtask test ssh"]
async fn the_shared_sftp_session_reopens_after_its_connection_is_lost() {
    let link = fixture();
    let sftp = Sftp::new(link.clone());
    let shared = sftp.shared().await.unwrap();
    shared.realpath(".").await.unwrap();
    let connection = command::stdout(&link, "fixture-connection-id", None)
        .await
        .unwrap();
    // The command runs on the connection it drops, so its own result is lost.
    let _ = command::run(&link, &format!("drop-connection {connection}"), None).await;
    eventually("the session to see its connection lost", || {
        shared.is_closed()
    })
    .await;
    let lost = shared.downgrade();
    drop(shared);
    let reopened = sftp.shared().await.unwrap();
    assert!(!reopened.downgrade().ptr_eq(&lost));
    reopened.realpath(".").await.unwrap();
    assert_ne!(
        command::stdout(&link, "fixture-connection-id", None)
            .await
            .unwrap(),
        connection
    );
}

#[tokio::test]
#[ignore = "Run with cargo xtask test ssh"]
async fn the_production_login_uses_the_key_file_and_refuses_a_wrong_one() {
    let server = super::integration_tests::server();
    assert_eq!(
        command::stdout(
            &super::link(&server, crate::ssh::no_jump_hosts),
            "printf ok",
            None
        )
        .await
        .unwrap(),
        "ok"
    );

    let directory = tempfile::tempdir().unwrap();
    let manager =
        crate::manager::Manager::new(crate::profile::Store::for_test(directory.path().into()));
    let owner = manager.sessions.session(&server, 0);
    assert_eq!(
        command::stdout(owner.link(), "printf ok", None)
            .await
            .unwrap(),
        "ok"
    );
    manager.sessions.shutdown().await;

    let mut wrong = server.clone();
    wrong.ssh_user = "nobody".into();
    assert_eq!(
        refused(&super::link(&wrong, crate::ssh::no_jump_hosts)).await,
        Error::AuthRefused
    );

    let mut missing = server;
    missing.identity_file = Some(
        directory
            .path()
            .join("missing")
            .to_string_lossy()
            .into_owned(),
    );
    // A key file that isn't there is this computer's problem, not a refusal.
    let error = refused(&super::link(&missing, crate::ssh::no_jump_hosts)).await;
    assert!(
        matches!(&error, Error::Credentials(reason) if reason.starts_with("Cannot read identity")),
        "{error:?}"
    );
}

#[tokio::test]
#[ignore = "Run with cargo xtask test ssh"]
async fn known_hosts_trusts_a_new_server_and_refuses_a_changed_key() {
    let server = super::integration_tests::server();
    let directory = tempfile::tempdir().unwrap();
    let trusted = directory.path().join("trusted");
    let link = link_with(
        &server,
        Arc::new(TcpDialer),
        KnownHosts::at(trusted.clone()),
    );
    assert_eq!(
        command::stdout(&link, "printf ok", None).await.unwrap(),
        "ok"
    );
    let saved = std::fs::read_to_string(&trusted).unwrap();
    let host = format!("[127.0.0.1]:{}", server.ssh_port);
    let key = saved
        .strip_prefix(&format!("{host} "))
        .unwrap_or_else(|| panic!("{saved}"));
    let fingerprint = PublicKey::from_openssh(key.trim())
        .unwrap()
        .fingerprint(HashAlg::Sha256)
        .to_string();

    let changed = directory.path().join("changed");
    let other = std::fs::read_to_string(std::env::var("NEWPORT_TEST_OTHER_KEY").unwrap()).unwrap();
    std::fs::write(&changed, format!("{host} {other}")).unwrap();
    let link = link_with(
        &server,
        Arc::new(TcpDialer),
        KnownHosts::at(changed.clone()),
    );
    assert_eq!(refused(&link).await, Error::HostKeyChanged { fingerprint });
    assert_eq!(
        std::fs::read_to_string(&changed).unwrap(),
        format!("{host} {other}")
    );
}

#[tokio::test]
#[ignore = "Run with cargo xtask test ssh"]
async fn every_kind_of_credential_logs_in() {
    let _owner = crate::credentials::TEST_OWNER.lock().await;
    async fn ok(server: &crate::model::Server) -> String {
        command::stdout(
            &super::link(server, crate::ssh::no_jump_hosts),
            "printf ok",
            None,
        )
        .await
        .unwrap()
    }
    let mut server = super::integration_tests::server();
    // The fixture's real SSH agent holds this key's decrypted form.
    server.identity_file = Some(format!(
        "{}.encrypted",
        std::env::var("NEWPORT_TEST_IDENTITY").unwrap()
    ));
    assert_eq!(ok(&server).await, "ok");
    server.identity_file = None;
    assert_eq!(ok(&server).await, "ok");
    let mut agent = crate::agent_keys::connect(Some("system")).await.unwrap();
    let keys = agent.request_identities().await.unwrap();
    let fingerprint = keys[0]
        .public_key()
        .fingerprint(HashAlg::Sha256)
        .to_string();
    server.agent_source = Some("system".into());
    server.agent_key_fingerprint = Some(fingerprint);
    assert_eq!(ok(&server).await, "ok");
    server.agent_key_fingerprint = Some(format!("SHA256:{}", "A".repeat(43)));
    assert_eq!(
        login_failure(&super::link(&server, crate::ssh::no_jump_hosts)).await,
        Error::AgentUnavailable
    );
    server.agent_key_fingerprint = None;
    server.agent_source = None;
    server.auth_method = crate::model::AuthMethod::Password;
    // The production credential reader, over an encrypted fixture vault.
    let vault = tempfile::tempdir().unwrap();
    let store = std::sync::Arc::new(crate::profile::Store::for_test(vault.path().into()));
    let config = crate::model::Config {
        servers: vec![server.clone()],
        tunnels: vec![],
    };
    let password = zeroize::Zeroizing::new(std::env::var("NEWPORT_TEST_PASSWORD").unwrap());
    store
        .save_with_password(&config, Some((server.id, &password)))
        .unwrap();
    let _credentials = crate::credentials::register(store).unwrap();
    assert_eq!(ok(&server).await, "ok");
    server.ssh_user = "wrong".into();
    assert_eq!(
        refused(&super::link(&server, crate::ssh::no_jump_hosts)).await,
        Error::AuthRefused
    );
}

#[tokio::test]
#[ignore = "Run with cargo xtask test ssh"]
async fn destination_checks_report_each_status_and_release_their_ports() {
    use crate::model::{DestinationStatus, Status, Tunnel};
    let server = super::integration_tests::server();
    let link = fixture();
    for (remote, expected) in [
        (1, DestinationStatus::Unavailable),
        (2, DestinationStatus::Blocked),
        (3, DestinationStatus::Reachable),
    ] {
        let local = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let tunnel = Tunnel {
            id: uuid::Uuid::new_v4(),
            name: "health".into(),
            server_id: server.id,
            local_port: local,
            local_port_end: None,
            remote_host: "health.fixture".into(),
            remote_port: remote,
            remote_port_end: None,
            auto_connect: false,
            auto_reconnect: false,
            direction: Default::default(),
            project: None,
        };
        let forward = super::forward::Forward::start(link.clone(), &tunnel, true)
            .await
            .unwrap();
        eventually("the destination check", || {
            forward.destination_health()[0].status != DestinationStatus::Checking
        })
        .await;
        let health = forward.destination_health();
        assert_eq!(health[0].status, expected, "port {remote}");
        assert!(health[0].checked_at.is_some());
        // Only a tunnel whose every destination is forbidden fails.
        assert_eq!(
            forward.state().borrow().status == Status::Error,
            expected == DestinationStatus::Blocked,
            "port {remote}"
        );
        forward.shutdown().await;
        std::net::TcpListener::bind(("127.0.0.1", local)).expect("the local port to be free");
    }
}
