//! Loopback SSH fixture checks using file keys; no private product is needed.
use super::super::session_pool::EXTRA_TRANSPORTS;
use super::*;
use std::sync::Arc;

fn server() -> Server {
    let mut server = super::super::integration_tests::server();
    server.ssh_user = "fixture-one-session".into();
    server
}
async fn bounded<F: std::future::Future>(future: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(8), future)
        .await
        .expect("Fixture stalled")
}

#[tokio::test]
#[ignore = "Run against the loopback SSH fixture with file keys"]
async fn session_pool_shares_sftp_and_exec_when_primary_is_full() {
    let session = Arc::new(ExecSession::connect(&server()).await.unwrap());
    let _primary = session.exec_channel("stall-until-close").await.unwrap();
    let mut clients = tokio::task::JoinSet::new();
    for _ in 0..16 {
        let session = session.clone();
        clients.spawn(async move { session.sftp().await.unwrap() });
    }
    let first = bounded(clients.join_next()).await.unwrap().unwrap();
    while let Some(client) = bounded(clients.join_next()).await {
        assert!(Arc::ptr_eq(&first, &client.unwrap()));
    }
    first.realpath(".").await.unwrap();
    for _ in 0..4 {
        assert_eq!(
            bounded(session.execute("printf ok", None)).await.unwrap(),
            "ok"
        );
        first.realpath(".").await.unwrap();
    }
    assert_eq!(session.0.sessions.child_count(), 2);
    let pressure = session.channel_pressure();
    assert_eq!(
        bounded(session.execute("printf ok", None)).await.unwrap(),
        "ok"
    );
    assert!(
        !pressure.has_changed().unwrap(),
        "unused pooled capacity reclaimed idle workers"
    );
    assert!(session.responsive().await);
    session.close().await;
    assert!(session.is_closed());
    assert_eq!(session.0.sessions.child_count(), 0);
    bounded(async {
        while !first.is_closed() {
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert!(first.realpath(".").await.is_err());
}

#[tokio::test]
#[ignore = "Run against the loopback SSH fixture with file keys"]
async fn session_pool_caps_connections_and_reuses_released_capacity_after_cancel() {
    let session = Arc::new(ExecSession::connect(&server()).await.unwrap());
    let mut held = Vec::new();
    for _ in 0..=EXTRA_TRANSPORTS {
        held.push(
            bounded(session.exec_channel("stall-until-close"))
                .await
                .unwrap(),
        );
    }
    assert_eq!(session.0.sessions.child_count(), EXTRA_TRANSPORTS);
    let pending = session.execute("printf ok", None);
    assert!(
        tokio::time::timeout(Duration::from_millis(350), pending)
            .await
            .is_err()
    );
    assert_eq!(session.0.sessions.child_count(), EXTRA_TRANSPORTS);
    assert!(session.responsive().await);
    held.pop().unwrap().close().await.unwrap();
    assert_eq!(
        bounded(session.execute("printf ok", None)).await.unwrap(),
        "ok"
    );
    assert_eq!(session.0.sessions.child_count(), EXTRA_TRANSPORTS);
    session.close().await;
}

#[tokio::test]
#[ignore = "Run against the loopback SSH fixture with file keys"]
async fn session_pool_primary_abort_cancels_waiters_and_secondary_channels() {
    let session = Arc::new(ExecSession::connect(&server()).await.unwrap());
    let mut held = Vec::new();
    for _ in 0..=EXTRA_TRANSPORTS {
        held.push(
            bounded(session.exec_channel("stall-until-close"))
                .await
                .unwrap(),
        );
    }
    let mut pressure = session.channel_pressure();
    let pending = {
        let session = session.clone();
        tokio::spawn(async move { session.execute("printf ok", None).await })
    };
    bounded(pressure.changed()).await.unwrap();
    session.abort_transport();
    assert!(bounded(pending).await.unwrap().is_err());
    assert!(session.is_closed());
    assert_eq!(session.0.sessions.child_count(), 0);
    for mut channel in held {
        bounded(async { while channel.wait().await.is_some() {} }).await;
    }
}

#[tokio::test]
#[ignore = "Run against the loopback SSH fixture with file keys"]
async fn session_pool_zero_sessions_never_opens_additional_connections() {
    let mut server = server();
    server.ssh_user = "fixture-no-sessions".into();
    let session = ExecSession::connect(&server).await.unwrap();
    let error = bounded(session.execute("printf ok", None))
        .await
        .unwrap_err();
    assert!(
        error.starts_with("SSH session capacity unavailable"),
        "{error}"
    );
    assert_eq!(session.0.sessions.child_count(), 0);
    assert!(session.responsive().await);
    session.close().await;
}

#[tokio::test]
#[ignore = "Run against the loopback SSH fixture with file keys"]
async fn session_pool_authentication_failure_preserves_existing_transport() {
    let key = tempfile::NamedTempFile::new().unwrap();
    let mut server = server();
    std::fs::copy(server.identity_file.as_ref().unwrap(), key.path()).unwrap();
    server.identity_file = Some(key.path().to_str().unwrap().into());
    let session = ExecSession::connect(&server).await.unwrap();
    let _primary = session.exec_channel("stall-until-close").await.unwrap();
    let contents = std::fs::read(key.path()).unwrap();
    let path = key.path().to_owned();
    key.close().unwrap();
    let error = bounded(session.execute("printf ok", None))
        .await
        .unwrap_err();
    assert!(error.contains("Cannot read identity"), "{error}");
    assert_eq!(session.0.sessions.child_count(), 0);
    assert!(session.responsive().await);
    std::fs::write(&path, contents).unwrap();
    // A subsequent background request must observe the same failed attempt,
    // even after the credential is repaired. Explicit retry unblocks it.
    let repeated = bounded(session.execute("printf ok", None))
        .await
        .unwrap_err();
    assert_eq!(error, repeated);
    assert_eq!(session.0.sessions.child_count(), 0);
    session.retry_session_admission();
    assert_eq!(
        bounded(session.execute("printf ok", None)).await.unwrap(),
        "ok"
    );
    std::fs::remove_file(path).unwrap();
    assert_eq!(session.0.sessions.child_count(), 1);
    session.close().await;
}

#[tokio::test]
#[ignore = "Run against the loopback SSH fixture with file keys"]
async fn session_pool_bounds_queued_requests_and_cancellation_releases_waiters() {
    let mut server = server();
    server.ssh_user = "fixture-no-sessions".into();
    let session = Arc::new(ExecSession::connect(&server).await.unwrap());
    let mut pending = tokio::task::JoinSet::new();
    for _ in 0..32 {
        let session = session.clone();
        pending.spawn(async move { session.execute("printf ok", None).await });
    }
    bounded(async {
        while session.0.sessions.waiter_count() < 32 {
            tokio::task::yield_now().await;
        }
    })
    .await;
    let error = tokio::time::timeout(
        Duration::from_millis(100),
        session.execute("printf ok", None),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert!(
        error.starts_with("SSH session capacity unavailable"),
        "{error}"
    );
    pending.abort_all();
    while pending.join_next().await.is_some() {}
    assert_eq!(session.0.sessions.waiter_count(), 0);
    assert_eq!(session.0.sessions.child_count(), 0);
    assert!(session.responsive().await);
    session.close().await;
}

#[tokio::test]
#[ignore = "Run against the loopback SSH fixture with file keys"]
async fn session_pool_remote_primary_disconnect_closes_children_and_waiters() {
    let session = Arc::new(ExecSession::connect(&server()).await.unwrap());
    let id = session
        .execute("fixture-connection-id", None)
        .await
        .unwrap();
    let mut held = Vec::new();
    for _ in 0..=EXTRA_TRANSPORTS {
        held.push(
            bounded(session.exec_channel("stall-until-close"))
                .await
                .unwrap(),
        );
    }
    let mut pressure = session.channel_pressure();
    let pending = {
        let session = session.clone();
        tokio::spawn(async move { session.execute("printf ok", None).await })
    };
    bounded(pressure.changed()).await.unwrap();
    let control = ExecSession::connect(&super::super::integration_tests::server())
        .await
        .unwrap();
    control
        .execute(&format!("drop-connection {id}"), None)
        .await
        .unwrap();
    assert!(bounded(pending).await.unwrap().is_err());
    assert!(session.is_closed());
    assert_eq!(session.0.sessions.child_count(), 0);
    for mut channel in held {
        bounded(async { while channel.wait().await.is_some() {} }).await;
    }
    control.close().await;
}

#[tokio::test]
#[ignore = "Run against the loopback SSH fixture with file keys"]
async fn session_pool_secondary_disconnect_during_admission_preserves_primary() {
    let session = ExecSession::connect(&server()).await.unwrap();
    let _primary = session.exec_channel("stall-until-close").await.unwrap();
    let child = bounded(session.execute("fixture-connection-id", None))
        .await
        .unwrap();
    assert_eq!(session.0.sessions.child_count(), 1);
    let control = ExecSession::connect(&super::super::integration_tests::server())
        .await
        .unwrap();
    control
        .execute(&format!("drop-next-session {child}"), None)
        .await
        .unwrap();
    assert_eq!(
        bounded(session.execute("printf ok", None)).await.unwrap(),
        "ok"
    );
    assert!(session.responsive().await);
    let replacement = bounded(session.execute("fixture-connection-id", None))
        .await
        .unwrap();
    assert_ne!(child, replacement);
    assert_eq!(session.0.sessions.child_count(), 1);
    session.close().await;
    control.close().await;
}

#[tokio::test]
#[ignore = "Run against the loopback SSH fixture with file keys, one test thread"]
async fn session_pool_cancelled_authentication_requires_explicit_retry() {
    let mut server = server();
    server.ssh_user = "fixture-paused-auth".into();
    let session = Arc::new(ExecSession::connect(&server).await.unwrap());
    let _primary = session.exec_channel("stall-until-close").await.unwrap();
    let control = ExecSession::connect(&super::super::integration_tests::server())
        .await
        .unwrap();
    let attempt = {
        let session = session.clone();
        tokio::spawn(async move { session.execute("printf ok", None).await })
    };
    bounded(async {
        while control.execute("pool-auth-attempts", None).await.unwrap() != "2" {
            tokio::task::yield_now().await;
        }
    })
    .await;
    attempt.abort();
    assert!(attempt.await.unwrap_err().is_cancelled());
    assert_eq!(session.0.sessions.waiter_count(), 0);
    assert_eq!(session.0.sessions.child_count(), 0);
    control.execute("resume-pool-auth", None).await.unwrap();
    let error = bounded(session.execute("printf ok", None))
        .await
        .unwrap_err();
    assert!(error.contains("setup was cancelled"), "{error}");
    assert_eq!(
        control.execute("pool-auth-attempts", None).await.unwrap(),
        "2"
    );
    assert!(session.responsive().await);
    session.retry_session_admission();
    assert_eq!(
        bounded(session.execute("printf ok", None)).await.unwrap(),
        "ok"
    );
    assert_eq!(
        control.execute("pool-auth-attempts", None).await.unwrap(),
        "3"
    );
    session.close().await;
    control.close().await;
}

#[tokio::test]
#[ignore = "Run against the loopback SSH fixture with file keys, one test thread"]
async fn session_pool_revalidates_host_trust_before_creating_a_child() {
    struct Restore(std::path::PathBuf, Vec<u8>);
    impl Drop for Restore {
        fn drop(&mut self) {
            std::fs::write(&self.0, &self.1).unwrap();
        }
    }
    let server = server();
    let session = ExecSession::connect(&server).await.unwrap();
    let _primary = session.exec_channel("stall-until-close").await.unwrap();
    let path = std::path::PathBuf::from(std::env::var_os("NEWPORT_TEST_KNOWN_HOSTS").unwrap());
    let restore = Restore(path.clone(), std::fs::read(&path).unwrap());
    let other = russh::keys::PrivateKey::from(
        russh::keys::ssh_key::private::Ed25519Keypair::from_seed(&[51; 32]),
    );
    std::fs::write(
        &path,
        format!(
            "[127.0.0.1]:{} {}\n",
            server.ssh_port,
            other.public_key().to_openssh().unwrap()
        ),
    )
    .unwrap();
    let error = bounded(session.execute("printf ok", None))
        .await
        .unwrap_err();
    assert!(error.contains("SSH host key changed"), "{error}");
    assert_eq!(session.0.sessions.child_count(), 0);
    assert!(session.responsive().await);
    drop(restore);
    assert_eq!(
        bounded(session.execute("printf ok", None))
            .await
            .unwrap_err(),
        error
    );
    session.retry_session_admission();
    assert_eq!(
        bounded(session.execute("printf ok", None)).await.unwrap(),
        "ok"
    );
    session.close().await;
}

#[tokio::test]
#[ignore = "Run against the loopback SSH fixture with file keys, one test thread"]
async fn session_pool_refused_first_secondary_channel_does_not_repeat_authentication() {
    let mut server = server();
    server.ssh_user = "fixture-refused-secondary".into();
    let session = ExecSession::connect(&server).await.unwrap();
    let _primary = session.exec_channel("stall-until-close").await.unwrap();
    let control = ExecSession::connect(&super::super::integration_tests::server())
        .await
        .unwrap();
    for _ in 0..4 {
        let error = bounded(session.execute("printf ok", None))
            .await
            .unwrap_err();
        assert!(
            error.starts_with("SSH session capacity unavailable"),
            "{error}"
        );
        assert_eq!(session.0.sessions.child_count(), 0);
    }
    assert_eq!(
        control
            .execute("refused-auth-attempts", None)
            .await
            .unwrap(),
        "2"
    );
    assert!(session.responsive().await);
    session.close().await;
    control.close().await;
}
