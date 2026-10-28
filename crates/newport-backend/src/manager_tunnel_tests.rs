use super::*;
use crate::commands;
use tokio::sync::oneshot;

fn fixture() -> (tempfile::TempDir, Manager, Tunnel) {
    let directory = tempfile::tempdir().unwrap();
    let mut manager = Manager::new(Store::for_test(directory.path().into()));
    let server: Server = serde_json::from_value(serde_json::json!({
        "id":Uuid::new_v4(), "name":"Cleanup fixture", "sshHost":"127.0.0.1",
        "sshPort":9, "sshUser":"fixture"
    }))
    .unwrap();
    let tunnel =
        newport_domain::forwarding::discovered_tunnel(&[], server.id, "127.0.0.1", 3000).unwrap();
    manager.config.servers.push(server);
    manager.config.tunnels.push(tunnel.clone());
    (directory, manager, tunnel)
}

/// Hold shutdown at a known point, like a remote cancellation awaiting its reply.
fn hold_running(
    manager: &mut Manager,
    tunnel: &Tunnel,
) -> (oneshot::Sender<()>, TunnelPublication) {
    manager.publish_forwarding();
    let publication = TunnelPublication {
        runtime: manager.runtime.clone(),
        id: tunnel.id,
        live: Arc::new(AtomicBool::new(true)),
    };
    publication.state(ConnectionState::new(Status::Connected));
    let (release, released) = oneshot::channel();
    let (cancel, _) = watch::channel(false);
    manager.forwarding.state.lock().unwrap().running.insert(
        tunnel.id,
        Running {
            cancel,
            task: tokio::spawn(async move {
                let _ = released.await;
            }),
            publication: publication.clone(),
            local_ports: tunnel
                .pairs()
                .unwrap()
                .into_iter()
                .map(|(port, _)| port)
                .collect(),
            previous: Vec::new(),
        },
    );
    (release, publication)
}

async fn retired(publication: &TunnelPublication) {
    // Profile commands persist the encrypted vault before cleanup admission.
    // Responsiveness is checked separately after this prerequisite completes.
    tokio::time::timeout(Duration::from_secs(15), async {
        while publication.live.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("cleanup must be admitted");
}

#[tokio::test]
async fn new_discovery_and_batch_commits_keep_stable_ids_and_do_not_duplicate_saved_destinations() {
    let (_directory, manager, existing) = fixture();
    let store = manager.store.clone();
    let state = share(manager);
    tokio::time::timeout(
        Duration::from_secs(15),
        commands::forward_discovered_port(&state, existing.server_id, "127.0.0.1".into(), 41011),
    )
    .await
    .expect("a replayed allocation must settle, not generate new IDs forever")
    .unwrap();
    let first = store.load().unwrap();
    let created = first
        .tunnels
        .iter()
        .find(|t| t.remote_port == 41011)
        .unwrap()
        .clone();
    commands::forward_discovered_port(&state, existing.server_id, "127.0.0.1".into(), 41011)
        .await
        .unwrap();
    assert_eq!(store.load().unwrap(), first);
    let requested = newport_domain::forwarding::discovered_tunnel(
        &first.tunnels,
        existing.server_id,
        "127.0.0.1",
        41012,
    )
    .unwrap();
    // Only guards against a hang: the encrypted profile save alone takes
    // seconds in debug builds, longer while the whole suite runs.
    let result = tokio::time::timeout(
        Duration::from_secs(60),
        commands::forward_tunnel_batch(
            &state,
            existing.server_id,
            vec![requested.clone()],
            vec![created.clone()],
            None,
        ),
    )
    .await
    .expect("batch commit must retain its proposed identities")
    .unwrap();
    assert_eq!(
        result.iter().map(|t| t.id).collect::<Vec<_>>(),
        vec![created.id, requested.id]
    );
    let saved = store.load().unwrap();
    assert_eq!(saved.tunnels.len(), 3);
    assert_eq!(saved, state.snapshot().config);
    commands::forward_tunnel_batch(
        &state,
        existing.server_id,
        vec![requested],
        vec![created],
        None,
    )
    .await
    .unwrap();
    assert_eq!(store.load().unwrap(), saved);
    shutdown(&state).await;
}

#[tokio::test]
async fn disconnect_does_not_hold_manager_and_outlives_cancelled_waiter() {
    let (_directory, mut manager, tunnel) = fixture();
    let (release, publication) = hold_running(&mut manager, &tunnel);
    let state = crate::manager::share(manager);
    let waiter = tokio::spawn({
        let state = state.clone();
        async move { commands::set_tunnel_connected(&state, tunnel.id, false).await }
    });
    retired(&publication).await;
    let snapshot = tokio::time::timeout(Duration::from_millis(200), commands::snapshot(&state))
        .await
        .expect("snapshot must not wait for SSH cleanup")
        .unwrap();
    assert_eq!(
        snapshot.runtime.tunnels[&tunnel.id].status,
        Status::Disconnected
    );
    assert!(!waiter.is_finished());
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    let cleanup = state.forwarding.cleanup(tunnel.id).unwrap();
    assert!(!cleanup.task.is_finished());
    publication.state(ConnectionState::error("late reply"));
    publication.health(Vec::new());
    let snapshot = commands::snapshot(&state).await.unwrap();
    assert_eq!(
        snapshot.runtime.tunnels[&tunnel.id].status,
        Status::Disconnected
    );
    assert!(!snapshot.runtime.tunnel_health.contains_key(&tunnel.id));
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), cleanup.wait())
        .await
        .unwrap();
}

#[tokio::test]
async fn forwarding_admission_ignores_profile_writer_and_unrelated_server_cleanup() {
    let (_directory, mut manager, a) = fixture();
    let (release, a_publication) = hold_running(&mut manager, &a);
    let ssh = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let mut server = manager.config.servers[0].clone();
    server.id = Uuid::new_v4();
    server.ssh_port = ssh.local_addr().unwrap().port();
    let mut b = a.clone();
    b.id = Uuid::new_v4();
    b.server_id = server.id;
    let port = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    b.local_port = port.local_addr().unwrap().port();
    drop(port);
    manager.config.servers.push(server);
    manager.config.tunnels.push(b.clone());
    let state = share(manager);
    let writer = state.lock().await;
    let stopping = tokio::spawn({
        let state = state.clone();
        async move { commands::set_tunnel_connected(&state, a.id, false).await }
    });
    retired(&a_publication).await;
    tokio::time::timeout(
        Duration::from_millis(200),
        commands::set_tunnel_connected(&state, b.id, true),
    )
    .await
    .expect("unrelated forwarding must not wait for profile storage or cleanup")
    .unwrap();
    let _peer = tokio::time::timeout(Duration::from_secs(2), ssh.accept())
        .await
        .unwrap()
        .unwrap();
    assert!(!stopping.is_finished());
    assert_eq!(
        state.snapshot().runtime.tunnels[&a.id].status,
        Status::Disconnected
    );
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), stopping)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    tokio::time::timeout(
        Duration::from_secs(2),
        commands::set_tunnel_connected(&state, b.id, false),
    )
    .await
    .unwrap()
    .unwrap();
    drop(writer);
    shutdown(&state).await;
}

#[tokio::test]
async fn stop_during_profile_edit_is_not_undone_when_the_edit_publishes() {
    let (_directory, mut manager, tunnel) = fixture();
    let (release, publication) = hold_running(&mut manager, &tunnel);
    let state = share(manager);
    let mut writer = state.lock().await;
    writer.config.tunnels[0].name = "Edited while storage is busy".into();
    let stopping = tokio::spawn({
        let state = state.clone();
        async move { commands::set_tunnel_connected(&state, tunnel.id, false).await }
    });
    retired(&publication).await;
    drop(writer);
    assert!(state.forwarding.active(tunnel.server_id).is_empty());
    assert_eq!(
        state.snapshot().runtime.tunnels[&tunnel.id].status,
        Status::Disconnected
    );
    release.send(()).unwrap();
    stopping.await.unwrap().unwrap();
    shutdown(&state).await;
}

#[tokio::test]
async fn saved_discovery_and_batch_starts_bypass_storage_but_validate_stale_scopes() {
    let (_directory, mut manager, tunnel) = fixture();
    let (release, _) = hold_running(&mut manager, &tunnel);
    let expected = manager.connection_scope(tunnel.server_id).unwrap();
    let state = share(manager);
    let writer = state.lock().await;
    assert_eq!(
        state
            .forward_open_endpoint(tunnel.id, None, Some(&tunnel), Some(&expected))
            .unwrap()
            .port(),
        tunnel.local_port,
    );
    tokio::time::timeout(
        Duration::from_millis(200),
        commands::forward_discovered_port(
            &state,
            tunnel.server_id,
            tunnel.remote_host.clone(),
            tunnel.remote_port,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let batch = tokio::time::timeout(
        Duration::from_millis(200),
        commands::forward_tunnel_batch(
            &state,
            tunnel.server_id,
            vec![],
            vec![tunnel.clone()],
            Some(expected.clone()),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(batch, vec![tunnel.clone()]);
    let mut stale_tunnel = tunnel.clone();
    stale_tunnel.remote_port += 1;
    assert!(
        commands::forward_tunnel_batch(
            &state,
            tunnel.server_id,
            vec![],
            vec![stale_tunnel],
            Some(expected.clone())
        )
        .await
        .is_err()
    );
    let mut stale_instance = expected.clone();
    stale_instance[9] = serde_json::json!(Uuid::new_v4());
    assert!(
        commands::forward_tunnel_batch(
            &state,
            tunnel.server_id,
            vec![],
            vec![tunnel.clone()],
            Some(stale_instance)
        )
        .await
        .is_err()
    );
    drop(writer);
    state
        .lock()
        .await
        .advance_connection_revision(tunnel.server_id);
    assert!(
        commands::forward_tunnel_batch(
            &state,
            tunnel.server_id,
            vec![],
            vec![tunnel.clone()],
            Some(expected)
        )
        .await
        .is_err()
    );
    release.send(()).unwrap();
    shutdown(&state).await;
}

#[tokio::test]
async fn published_credential_revision_retires_only_its_servers_running_forwards() {
    let (_directory, mut manager, a) = fixture();
    let (release_a, before_a) = hold_running(&mut manager, &a);
    let mut server = manager.config.servers[0].clone();
    server.id = Uuid::new_v4();
    let mut b = a.clone();
    b.id = Uuid::new_v4();
    b.server_id = server.id;
    b.local_port += 1;
    manager.config.servers.push(server);
    manager.config.tunnels.push(b.clone());
    let (release_b, before_b) = hold_running(&mut manager, &b);
    let state = share(manager);
    state.lock().await.advance_connection_revision(a.server_id);
    assert!(!before_a.live.load(Ordering::Acquire));
    assert!(before_b.live.load(Ordering::Acquire));
    assert_eq!(
        state.snapshot().runtime.tunnels[&a.id].status,
        Status::Connecting
    );
    assert_eq!(
        state.snapshot().runtime.tunnels[&b.id].status,
        Status::Connected
    );
    before_a.state(ConnectionState::error("old credentials replied"));
    assert_eq!(
        state.snapshot().runtime.tunnels[&a.id].status,
        Status::Connecting
    );
    let cleanup = state.disconnect_tunnel(a.id).unwrap();
    release_a.send(()).unwrap();
    cleanup.wait().await;
    release_b.send(()).unwrap();
    shutdown(&state).await;
}

#[tokio::test]
async fn concurrent_starts_cannot_reserve_the_same_local_port() {
    let (_directory, mut manager, mut a) = fixture();
    let ssh = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    manager.config.servers[0].ssh_port = ssh.local_addr().unwrap().port();
    let port = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    a.local_port = port.local_addr().unwrap().port();
    drop(port);
    let mut b = a.clone();
    b.id = Uuid::new_v4();
    manager.config.tunnels = vec![a.clone(), b.clone()];
    let state = share(manager);
    let (one, two) = tokio::join!(
        commands::set_tunnel_connected(&state, a.id, true),
        commands::set_tunnel_connected(&state, b.id, true)
    );
    assert!(one.is_ok() ^ two.is_ok());
    assert!(one.err().or(two.err()).unwrap().contains("overlap"));
    shutdown(&state).await;
}

#[tokio::test]
async fn replacement_preserves_cleanup_chain_and_local_port_reservations() {
    let (_directory, mut manager, mut tunnel) = fixture();
    let ssh = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    manager.config.servers[0].ssh_port = ssh.local_addr().unwrap().port();
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    tunnel.local_port = listener.local_addr().unwrap().port();
    manager.config.tunnels[0] = tunnel.clone();
    let (release, publication) = hold_running(&mut manager, &tunnel);
    manager.begin_disconnect(tunnel.id);
    // Change ports while an older generation is still releasing its listener.
    manager.config.tunnels[0].local_port = if tunnel.local_port == 65535 {
        65534
    } else {
        tunnel.local_port + 1
    };
    manager.connect(tunnel.id).unwrap();
    let stopped = manager.begin_disconnect(tunnel.id).unwrap();
    // A different tunnel can reuse the retired port only after the whole chain.
    let old = tunnel.id;
    tunnel.id = Uuid::new_v4();
    manager.config.tunnels = vec![tunnel.clone()];
    manager.runtime.lock().unwrap().tunnels.remove(&old);
    manager.connect(tunnel.id).unwrap();
    publication.state(ConnectionState::new(Status::Connected));
    publication.health(Vec::new());
    assert!(!manager.runtime.lock().unwrap().tunnels.contains_key(&old));
    assert!(
        !manager
            .runtime
            .lock()
            .unwrap()
            .tunnel_health
            .contains_key(&old)
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(100), ssh.accept())
            .await
            .is_err()
    );
    assert!(!stopped.task.is_finished());
    assert_eq!(
        manager.runtime.lock().unwrap().tunnels[&tunnel.id].status,
        Status::Connecting
    );
    drop(listener);
    release.send(()).unwrap();
    // No real authentication: the listener proves startup was deferred until
    // cleanup without depending on a saved server or credential provider.
    let _connection = tokio::time::timeout(Duration::from_secs(2), ssh.accept())
        .await
        .unwrap()
        .unwrap();
    manager.disconnect(tunnel.id).await;
    std::net::TcpListener::bind(("127.0.0.1", tunnel.local_port)).unwrap();
}

#[tokio::test]
async fn server_deletion_keeps_history_exclusive_without_blocking_snapshots() {
    use sqlx::Connection;
    let (directory, mut manager, tunnel) = fixture();
    manager.save(manager.config.clone()).await.unwrap();
    let mut replacement = manager.config.servers[0].clone();
    replacement.name = "Recreated profile".into();
    let (release, publication) = hold_running(&mut manager, &tunnel);
    let state = crate::manager::share(manager);
    let metrics = crate::metrics_db::MetricsDb::open(
        &directory.path().join("metrics.sqlite"),
        Ok(vec![7; 32]),
    )
    .await
    .unwrap();
    let mut database = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .filename(directory.path().join("metrics.sqlite")),
    )
    .await
    .unwrap();
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut database)
        .await
        .unwrap();
    let deleting = tokio::spawn({
        let state = state.clone();
        let metrics = metrics.clone();
        async move {
            let directory = tempfile::tempdir().unwrap();
            let preferences = crate::preferences::Preferences::open(directory.path()).unwrap();
            commands::delete_server(
                &crate::files::Operations::default(),
                &crate::terminal::Sessions::default(),
                &state,
                &metrics,
                &preferences,
                tunnel.server_id,
            )
            .await
        }
    });
    retired(&publication).await;
    // Retirement happens inside the commit guard, before its drop publishes
    // the new snapshot. Synchronize with that commit, not with cleanup (which
    // is deliberately held behind the SQLite transaction below).
    drop(
        tokio::time::timeout(Duration::from_millis(200), state.lock())
            .await
            .unwrap(),
    );
    let snapshot = tokio::time::timeout(Duration::from_millis(200), commands::snapshot(&state))
        .await
        .unwrap()
        .unwrap();
    assert!(snapshot.config.servers.is_empty());
    assert!(!snapshot.runtime.tunnels.contains_key(&tunnel.id));
    publication.state(ConnectionState::error("obsolete"));
    assert!(
        !commands::snapshot(&state)
            .await
            .unwrap()
            .runtime
            .tunnels
            .contains_key(&tunnel.id)
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(100), history_guard(&state))
            .await
            .is_err()
    );
    let cleanup = state.lock().await.profile_cleanup[0].clone();
    deleting.abort();
    assert!(deleting.await.unwrap_err().is_cancelled());
    assert!(
        tokio::time::timeout(Duration::from_millis(100), history_guard(&state))
            .await
            .is_err()
    );
    sqlx::query("ROLLBACK")
        .execute(&mut database)
        .await
        .unwrap();
    let history = tokio::time::timeout(Duration::from_secs(2), history_guard(&state))
        .await
        .unwrap();
    assert!(!cleanup.task.is_finished(), "SSH cleanup is still held");
    // Once old history is gone, a new profile with the same UUID can be saved
    // while the retired listener drains, without an old write deleting its data.
    state
        .lock()
        .await
        .save(Config {
            servers: vec![replacement.clone()],
            tunnels: Vec::new(),
        })
        .await
        .unwrap();
    metrics
        .record(
            &replacement,
            crate::metrics_db::now_ms(),
            &serde_json::json!({"cpu":42,"network":[]}),
        )
        .await
        .unwrap();
    drop(history);
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), cleanup.wait())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        state.lock().await.server(replacement.id).unwrap().name,
        replacement.name
    );
    assert_eq!(metrics.cache_info().await.unwrap().samples, 1);
    metrics.close().await;
}

#[tokio::test]
async fn shutdown_drains_pending_stops_and_rejects_new_forwarding() {
    let (_directory, mut manager, tunnel) = fixture();
    let (release, _) = hold_running(&mut manager, &tunnel);
    manager.begin_disconnect(tunnel.id);
    let shutdown = manager.begin_shutdown();
    assert!(
        manager
            .connect(tunnel.id)
            .unwrap_err()
            .contains("shutting down")
    );
    let shutting_down = tokio::spawn(shutdown.run());
    tokio::task::yield_now().await;
    assert!(!shutting_down.is_finished());
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), shutting_down)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
#[ignore = "Run with cargo xtask test ssh and its ephemeral SSH fixture"]
async fn command_edits_replace_live_forwards_and_release_listeners() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    async fn connected(state: &Shared, id: Uuid) {
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let snapshot = commands::snapshot(state).await.unwrap();
                let status = &snapshot.runtime.tunnels[&id];
                assert_ne!(status.status, Status::Error, "{:?}", status.error_message);
                if status.status == Status::Connected {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
    async fn echo(port: u16) {
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        stream.write_all(b"forward replaced").await.unwrap();
        let mut bytes = [0; 16];
        tokio::time::timeout(Duration::from_secs(3), stream.read_exact(&mut bytes))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&bytes, b"forward replaced");
    }
    let (_directory, mut manager, mut tunnel) = fixture();
    let server = &mut manager.config.servers[0];
    server.ssh_port = std::env::var("NEWPORT_TEST_SSH_PORT")
        .unwrap()
        .parse()
        .unwrap();
    server.identity_file = Some(std::env::var("NEWPORT_TEST_IDENTITY").unwrap());
    let port = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    tunnel.local_port = port.local_addr().unwrap().port();
    drop(port);
    tunnel.remote_port = 12345;
    manager.config.tunnels[0] = tunnel.clone();
    manager.save(manager.config.clone()).await.unwrap();
    let state = crate::manager::share(manager);
    commands::set_tunnel_connected(&state, tunnel.id, true)
        .await
        .unwrap();
    connected(&state, tunnel.id).await;
    echo(tunnel.local_port).await;
    for n in 0..3 {
        let mut next = tunnel.clone();
        next.name = format!("Replacement {n}");
        commands::update_tunnel(&state, next.clone(), tunnel)
            .await
            .unwrap();
        tunnel = next;
    }
    connected(&state, tunnel.id).await;
    echo(tunnel.local_port).await;
    commands::delete_tunnel(&state, tunnel.id).await.unwrap();
    assert!(
        !commands::snapshot(&state)
            .await
            .unwrap()
            .runtime
            .tunnels
            .contains_key(&tunnel.id)
    );
    std::net::TcpListener::bind(("127.0.0.1", tunnel.local_port)).unwrap();
    shutdown(&state).await;
}
