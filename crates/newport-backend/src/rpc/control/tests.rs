use super::*;
use crate::{
    desktop_integration::DesktopIntegration,
    manager::Manager,
    model::{ConnectionState, Server, Status},
    profile::Store,
    rpc,
};
use serde_json::json;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use uuid::Uuid;

struct DesktopProbe {
    manager: Shared,
    urls: Mutex<Vec<String>>,
}

#[tokio::test]
async fn terminal_link_rpc_opens_public_url_but_requires_terminal_for_loopback() {
    let fixture = Fixture::start().await;
    let terminal = Uuid::new_v4();
    fixture
        .call(
            "open_terminal_link",
            json!({"terminalId":terminal,"url":"https://example.test/path?q=1#section"}),
        )
        .await
        .unwrap();
    assert!(
        fixture
            .call(
                "open_terminal_link",
                json!({"terminalId":terminal,"url":"http://localhost:5178/path"})
            )
            .await
            .is_err()
    );
    assert_eq!(
        *fixture.desktop.urls.lock().unwrap(),
        vec!["https://example.test/path?q=1#section"]
    );
    assert!(fixture.manager.snapshot().config.tunnels.is_empty());
    fixture.stop().await;
}

#[tokio::test]
async fn connections_query_returns_immediately_and_shares_backend_owned_refresh() {
    let fixture = Fixture::start().await;
    let server: Server = serde_json::from_value(json!({"id":Uuid::new_v4(),"name":"Query scope", "sshHost":"127.0.0.1", "sshPort":9,"sshUser":"fixture"})).unwrap();
    fixture
        .manager
        .lock()
        .await
        .config
        .servers
        .push(server.clone());
    let owner = crate::server_session::resolve(&fixture.manager, server.id)
        .await
        .unwrap();
    let (calls, mut receive) = tokio::sync::mpsc::unbounded_channel();
    let query = crate::query::Query::new(
        crate::query::Policy {
            stale_after: Duration::from_secs(30),
            retry_after: Duration::from_secs(10),
            timeout: Duration::from_secs(35),
            observation_lease: Duration::from_secs(5),
        },
        move || {
            let calls = calls.clone();
            async move {
                let (reply, outcome) = tokio::sync::oneshot::channel();
                calls.send(reply).unwrap();
                outcome.await.unwrap()
            }
        },
    );
    assert!(owner.ports.set(query).is_ok());
    let args = json!({"id":server.id});
    let first = tokio::time::timeout(
        Duration::from_secs(1),
        fixture.call("connections_query", args.clone()),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(first["data"].is_null());
    assert_eq!(first["fetching"], true);
    let reply = receive.recv().await.unwrap();
    let second = tokio::time::timeout(
        Duration::from_secs(1),
        fixture.call("connections_query", args.clone()),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(second["fetching"], true);
    assert!(receive.try_recv().is_err());
    let mut changes = owner.ports.get().unwrap().subscribe();
    reply
        .send(Ok(vec![crate::ports::DiscoveredPort {
            port: 8080,
            ..Default::default()
        }]))
        .unwrap();
    while owner.ports.get().unwrap().snapshot().data.is_none() {
        changes.changed().await.unwrap();
    }
    let cached = fixture.call("connections_query", args).await.unwrap();
    assert_eq!(cached["data"][0]["port"], 8080);
    assert_eq!(cached["fetching"], false);
    assert!(receive.try_recv().is_err());
    assert!(
        owner.connected_lease().await.is_none(),
        "query state itself must not connect or install Needle"
    );
}

#[tokio::test]
async fn uninstall_requires_the_reviewed_connection_and_never_connects_when_stale() {
    let fixture = Fixture::start().await;
    let server: Server = serde_json::from_value(json!({"id":Uuid::new_v4(),"name":"Uninstall scope", "sshHost":"127.0.0.1", "sshPort":9,"sshUser":"fixture"})).unwrap();
    fixture
        .manager
        .lock()
        .await
        .config
        .servers
        .push(server.clone());
    let mut stale = json!({"expectedConnection":server});
    stale["expectedConnection"]["sshPort"] = 10.into();
    assert!(
        fixture
            .call("inspect_agent", stale.clone())
            .await
            .unwrap_err()
            .contains("connection changed")
    );
    assert!(fixture.call("inspect_agent", json!({})).await.is_err());
    assert!(
        fixture
            .call("uninstall_agent", stale)
            .await
            .unwrap_err()
            .contains("connection changed")
    );
    assert!(fixture.call("uninstall_agent", json!({})).await.is_err());
    let owner = fixture
        .manager
        .lock()
        .await
        .sessions
        .get(server.id)
        .unwrap();
    assert!(
        owner.connected_lease().await.is_none(),
        "invalid uninstall requests must not connect"
    );
}
impl DesktopIntegration for DesktopProbe {
    fn on_main_thread(&self, action: Box<dyn FnOnce() + Send>) -> Result<(), String> {
        action();
        Ok(())
    }
    fn clipboard_revision(&self) -> Option<isize> {
        None
    }
    fn clipboard_offer(&self, _: Option<isize>) -> Option<(isize, Vec<String>)> {
        None
    }
    fn read_clipboard_format(&self, _: isize, _: &str) -> Option<Vec<u8>> {
        None
    }
    fn open_url(&self, url: &str) -> Result<(), String> {
        // Opening an external application must not hold the manager lock.
        let _guard = self
            .manager
            .try_lock()
            .map_err(|_| "Manager locked during browser opening")?;
        self.urls.lock().unwrap().push(url.into());
        Ok(())
    }
}
struct ControlHost {
    manager: Shared,
    integration_access: crate::integration_permissions::Access,
    metrics: MetricsDb,
    files: Arc<crate::files::Operations>,
    terminals: crate::terminal::Sessions,
    preferences: crate::preferences::Preferences,
    desktop: Arc<DesktopProbe>,
}
impl rpc::Host for ControlHost {
    async fn dispatch(&self, method: &str, args: Value) -> Result<Value, Value> {
        Command::parse(method)
            .ok_or_else(|| json!("Unknown command"))?
            .dispatch(
                &Services {
                    manager: &self.manager,
                    integration_access: &self.integration_access,
                    metrics: &self.metrics,
                    files: &self.files,
                    terminals: &self.terminals,
                    preferences: &self.preferences,
                    desktop: self.desktop.clone(),
                },
                &args,
            )
            .await
    }
    async fn open_terminal(&self, _: rpc::TerminalOpen) -> Result<Box<dyn Send>, String> {
        Err("Not a terminal fixture".into())
    }
}
struct Fixture {
    directory: tempfile::TempDir,
    endpoint: PathBuf,
    manager: Shared,
    metrics: MetricsDb,
    files: Arc<crate::files::Operations>,
    desktop: Arc<DesktopProbe>,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
}
impl Fixture {
    async fn start() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let endpoint = directory.path().join("native-client/desktop.sock");
        let manager = crate::manager::share(Manager::new(Store::for_test(directory.path().into())));
        let metrics = MetricsDb::open(&directory.path().join("metrics.db"), Ok(vec![7; 32]))
            .await
            .unwrap();
        let files = Arc::new(crate::files::Operations::default());
        let desktop = Arc::new(DesktopProbe {
            manager: manager.clone(),
            urls: Mutex::default(),
        });
        let listener = rpc::Server::bind(&endpoint).unwrap();
        let integration_access = manager.lock().await.integration_access();
        let task = tokio::spawn(listener.run(ControlHost {
            integration_access,
            manager: manager.clone(),
            metrics: metrics.clone(),
            files: files.clone(),
            terminals: Default::default(),
            preferences: crate::preferences::Preferences::open(directory.path()).unwrap(),
            desktop: desktop.clone(),
        }));
        Self {
            directory,
            endpoint,
            manager,
            metrics,
            files,
            desktop,
            task,
        }
    }
    async fn call(&self, method: &'static str, args: Value) -> Result<Value, String> {
        let endpoint = self.endpoint.clone();
        tokio::task::spawn_blocking(move || {
            newport_client::backend::Backend::at_path(endpoint).call_with_timeout(
                method,
                args,
                Duration::from_secs(60),
            )
        })
        .await
        .unwrap()
    }
    async fn connection(&self, id: Uuid) -> Value {
        let snapshot = serde_json::to_value(self.manager.lock().await.snapshot()).unwrap();
        let server = snapshot["config"]["servers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|server| server["id"] == id.to_string())
            .unwrap();
        newport_domain::connection_scope::connection(server, &snapshot)
    }
    async fn stop(self) {
        self.manager.lock().await.shutdown().await;
        self.task.abort();
        assert!(self.task.await.unwrap_err().is_cancelled());
        self.metrics.close().await;
        self.directory.close().unwrap();
    }
}
fn server() -> Server {
    serde_json::from_value(json!({"id":Uuid::new_v4(),"name":"Isolated native control",
        "sshHost":"127.0.0.1","sshPort":9,"sshUser":"fixture"}))
    .unwrap()
}

#[tokio::test]
async fn native_profile_edits_preserve_scope_and_failed_delete_preserves_active_work() {
    let fixture = Fixture::start().await;
    let mut server = server();
    let id = server.id;
    fixture
        .call(
            "save_server",
            json!({"server":server,"password":"isolated test password"}),
        )
        .await
        .unwrap();
    let store = fixture.manager.lock().await.store.clone();
    let revision = fixture.manager.lock().await.connection_revision(id);
    assert_eq!(
        store.password(id).unwrap().as_str(),
        "isolated test password"
    );
    fixture
        .metrics
        .record(
            &server,
            crate::metrics_db::now_ms(),
            &json!({"cpu":42,"network":[]}),
        )
        .await
        .unwrap();
    let (started, ready) = tokio::sync::oneshot::channel();
    let files = fixture.files.clone();
    let manager = fixture.manager.clone();
    let running = tokio::spawn(async move {
        files
            .run(&manager, id, Uuid::new_v4(), async move |_| {
                started.send(()).unwrap();
                std::future::pending::<anyhow::Result<()>>().await
            })
            .await
    });
    ready.await.unwrap();
    server.name = "Renamed".into();
    fixture
        .call("save_server", json!({"server":server}))
        .await
        .unwrap();
    assert_eq!(
        fixture.manager.lock().await.connection_revision(id),
        revision
    );
    assert!(!running.is_finished());
    fixture.manager.lock().await.load_error = Some("Profile is unavailable".into());
    let error = fixture
        .call("delete_server", json!({"id":id}))
        .await
        .unwrap_err();
    assert!(error.contains("Profile is unavailable"));
    assert!(!running.is_finished());
    assert_eq!(fixture.metrics.cache_info().await.unwrap().samples, 1);
    assert!(fixture.manager.lock().await.server(id).is_ok());
    fixture.manager.lock().await.load_error = None;
    server.ssh_user = "edited-user".into();
    fixture
        .call("save_server", json!({"server":server}))
        .await
        .unwrap();
    assert!(fixture.manager.lock().await.connection_revision(id) > revision);
    assert!(
        tokio::time::timeout(Duration::from_secs(2), running)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    assert_eq!(
        store.password(id).unwrap().as_str(),
        "isolated test password"
    );
    fixture
        .call("delete_server", json!({"id":id}))
        .await
        .unwrap();
    assert!(fixture.manager.lock().await.server(id).is_err());
    assert!(store.load().unwrap().servers.is_empty());
    assert!(store.password(id).is_err());
    assert_eq!(fixture.metrics.cache_info().await.unwrap().samples, 0);
    fixture.stop().await;
}

#[tokio::test]
async fn native_forward_edits_and_browser_opening_reject_stale_requests() {
    let fixture = Fixture::start().await;
    let server = server();
    fixture
        .call("save_server", json!({"server":server}))
        .await
        .unwrap();
    let mut tunnel =
        newport_domain::forwarding::discovered_tunnel(&[], server.id, "127.0.0.1", 3000).unwrap();
    tunnel.local_port = 9000;
    fixture
        .call("save_tunnel", json!({"tunnel":tunnel}))
        .await
        .unwrap();
    let expected = tunnel.clone();
    tunnel.name = "Edited".into();
    fixture
        .call(
            "update_tunnel",
            json!({"tunnel":tunnel,"expected":expected}),
        )
        .await
        .unwrap();
    assert!(
        fixture
            .call(
                "update_tunnel",
                json!({"tunnel":expected,"expected":expected})
            )
            .await
            .is_err()
    );
    let connection = fixture.connection(server.id).await;
    fixture
        .manager
        .lock()
        .await
        .runtime
        .lock()
        .unwrap()
        .tunnels
        .insert(tunnel.id, ConnectionState::new(Status::Connected));
    let args = json!({"id":tunnel.id,"expected":tunnel,"expectedConnection":connection});
    fixture.call("open_tunnel", args.clone()).await.unwrap();
    assert_eq!(
        *fixture.desktop.urls.lock().unwrap(),
        vec!["http://localhost:9000"]
    );
    fixture
        .manager
        .lock()
        .await
        .advance_connection_revision(server.id);
    assert!(
        fixture
            .call("open_tunnel", args)
            .await
            .unwrap_err()
            .contains("connection changed")
    );
    assert!(fixture.call("forward_tunnel_batch", json!({"serverId":server.id,"create":[],"connect":[tunnel],"expectedConnection":connection})).await.unwrap_err().contains("connection changed"));
    fixture
        .call("delete_tunnel", json!({"id":tunnel.id}))
        .await
        .unwrap();
    assert!(
        fixture
            .call("update_tunnel", json!({"tunnel":tunnel,"expected":tunnel}))
            .await
            .is_err()
    );
    assert!(fixture.manager.lock().await.config.tunnels.is_empty());
    fixture.stop().await;
}

mod remote_tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    #[ignore = "Requires cargo xtask test remote and real OpenSSH"]
    async fn native_forward_lifecycle_reaches_real_http_and_releases_listener() {
        let fixture = Fixture::start().await;
        let mut server = server();
        server.ssh_port = std::env::var("NEWPORT_TEST_SSH_PORT")
            .unwrap()
            .parse()
            .unwrap();
        server.identity_file = Some(std::env::var("NEWPORT_TEST_IDENTITY").unwrap());
        fixture
            .call("save_server", json!({"server":server}))
            .await
            .unwrap();
        fixture
            .call("test_connection", json!({"id":server.id}))
            .await
            .unwrap();
        let reservation = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let local = reservation.local_addr().unwrap().port();
        drop(reservation);
        let mut tunnel =
            newport_domain::forwarding::discovered_tunnel(&[], server.id, "127.0.0.1", 8080)
                .unwrap();
        tunnel.local_port = local;
        fixture
            .call("save_tunnel", json!({"tunnel":tunnel}))
            .await
            .unwrap();
        fixture
            .call(
                "set_tunnel_connected",
                json!({"id":tunnel.id,"connected":true}),
            )
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                if fixture
                    .manager
                    .lock()
                    .await
                    .runtime
                    .lock()
                    .unwrap()
                    .tunnels
                    .get(&tunnel.id)
                    .is_some_and(|s| s.status == Status::Connected)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .unwrap();
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", local))
            .await
            .unwrap();
        stream
            .write_all(b"GET / HTTP/1.0\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let mut response = String::new();
        tokio::time::timeout(
            Duration::from_secs(10),
            stream.read_to_string(&mut response),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(response.contains("200 OK"), "{response}");
        fixture
            .call(
                "forward_discovered_port",
                json!({"serverId":server.id,"address":"127.0.0.1","port":8080}),
            )
            .await
            .unwrap();
        let connection = fixture.connection(server.id).await;
        let resolved = fixture.call("forward_tunnel_batch", json!({"serverId":server.id,"create":[],"connect":[tunnel],"expectedConnection":connection})).await.unwrap();
        assert_eq!(resolved.as_array().unwrap().len(), 1);
        assert_eq!(fixture.manager.lock().await.config.tunnels.len(), 1);
        fixture
            .call(
                "open_tunnel",
                json!({"id":tunnel.id,"expected":tunnel,"expectedConnection":connection}),
            )
            .await
            .unwrap();
        assert_eq!(
            *fixture.desktop.urls.lock().unwrap(),
            vec![format!("http://localhost:{local}")]
        );
        fixture
            .call(
                "set_tunnel_connected",
                json!({"id":tunnel.id,"connected":false}),
            )
            .await
            .unwrap();
        assert!(
            tokio::net::TcpStream::connect(("127.0.0.1", local))
                .await
                .is_err()
        );
        fixture
            .call("delete_server", json!({"id":server.id}))
            .await
            .unwrap();
        assert!(fixture.manager.lock().await.config.tunnels.is_empty());
        fixture.stop().await;
    }
}

#[tokio::test]
async fn integration_routes_reject_obsolete_connection_identity_before_any_side_effect() {
    let fixture = Fixture::start().await;
    let mut server = server();
    server.browser_enabled = true;
    server.clipboard_enabled = true;
    let (expected, owner) = {
        let mut manager = fixture.manager.lock().await;
        manager.config.servers.push(server.clone());
        let owner = manager.sessions.session(&server, 0);
        (manager.connection_scope(server.id).unwrap(), owner)
    };
    let permissions = serde_json::to_value(owner.permissions.snapshot()).unwrap();
    let before = serde_json::to_value(fixture.manager.lock().await.config.clone()).unwrap();
    // Every identity field, credential-only changes, and backend restarts must fence
    // both the Clipboard and integration routes, in both directions.
    for field in 0..10 {
        let mut stale = expected.clone();
        stale[field] = if field == 8 {
            json!(99)
        } else {
            json!("obsolete")
        };
        for feature in ["clipboard", "browser"] {
            for enabled in [false, true] {
                let args = json!({"id":server.id,"feature":feature,"enabled":enabled,"expectedConnection":stale});
                assert!(
                    fixture
                        .call("set_integration_enabled", args.clone())
                        .await
                        .unwrap_err()
                        .contains("connection changed")
                );
                assert!(
                    fixture
                        .call("set_clipboard_enabled", args)
                        .await
                        .unwrap_err()
                        .contains("connection changed")
                );
            }
        }
    }
    for method in ["set_clipboard_enabled", "set_integration_enabled"] {
        for identity in [None, Some(Value::Null), Some(json!([]))] {
            let mut args = json!({"id":server.id,"feature":"browser","enabled":false});
            if let Some(identity) = identity {
                args["expectedConnection"] = identity;
            }
            assert!(fixture.call(method, args).await.is_err());
        }
    }
    assert_eq!(
        serde_json::to_value(owner.permissions.snapshot()).unwrap(),
        permissions
    );
    assert_eq!(
        serde_json::to_value(fixture.manager.lock().await.config.clone()).unwrap(),
        before
    );
    assert_eq!(owner.transport_snapshot().demand, 0);
    assert!(fixture.desktop.urls.lock().unwrap().is_empty());
    // Presentation edits and the other feature's saved choice do not invalidate
    // explicit intent for this same connection.
    fixture.manager.lock().await.config.servers[0].name = "Renamed".into();
    for (method, feature) in [
        ("set_integration_enabled", "browser"),
        ("set_clipboard_enabled", "clipboard"),
    ] {
        fixture.call(method, json!({"id":server.id,"feature":feature,"enabled":false,"expectedConnection":expected})).await.unwrap();
    }
    let stored = fixture.manager.lock().await.store.load().unwrap();
    assert!(!stored.servers[0].browser_enabled);
    assert!(!stored.servers[0].clipboard_enabled);
    assert_eq!(owner.transport_snapshot().demand, 0);
    fixture.stop().await;
}

#[tokio::test]
async fn integration_off_revokes_before_manager_lock_and_supersedes_waiting_enable() {
    let fixture = Fixture::start().await;
    let mut server = server();
    server.browser_enabled = true;
    server.clipboard_enabled = true;
    let (owner, expected) = {
        let mut manager = fixture.manager.lock().await;
        let mut config = manager.config.clone();
        config.servers.push(server.clone());
        manager.save(config).await.unwrap();
        (
            manager.sessions.get(server.id).unwrap(),
            manager.connection_scope(server.id).unwrap(),
        )
    };
    let original = owner.permissions.snapshot().desired;
    assert!(
        owner
            .permissions
            .acknowledge(crate::agent::control::Applied {
                permissions: original,
                error: None
            })
    );
    let ticket = owner.permissions.clipboard_ticket().unwrap();
    let manager = fixture.manager.lock().await;
    let before = owner.permissions.preference_revisions();
    let mut enable = Box::pin(fixture.call(
        "set_integration_enabled",
        json!({"id":server.id,"feature":"clipboard","enabled":true,"expectedConnection":expected}),
    ));
    tokio::select! {
        result = &mut enable => panic!("save must wait for manager: {result:?}"),
        _ = tokio::time::timeout(Duration::from_secs(3), async {
            while owner.permissions.preference_revisions() == before { tokio::task::yield_now().await; }
        }) => {},
    }
    assert_ne!(owner.permissions.preference_revisions(), before);
    let mut disable = Box::pin(fixture.call(
        "set_clipboard_enabled",
        json!({"id":server.id,"enabled":false,"expectedConnection":expected}),
    ));
    tokio::select! {
        result = &mut disable => panic!("persistence must wait for manager: {result:?}"),
        result = tokio::time::timeout(Duration::from_secs(3), async {
            while owner.permissions.clipboard_enabled() { tokio::task::yield_now().await; }
        }) => result.unwrap(),
    }
    assert!(
        owner
            .permissions
            .clipboard_at(ticket, || panic!("obsolete local read"))
            .is_none()
    );
    assert!(owner.permissions.browser_enabled());
    assert!(
        !owner
            .permissions
            .acknowledge(crate::agent::control::Applied {
                permissions: original,
                error: None
            })
    );
    assert!(
        manager.config.servers[0].clipboard_enabled,
        "no premature claim of saved preference"
    );
    assert_eq!(owner.transport_snapshot().demand, 0);
    drop(manager);
    assert!(enable.await.unwrap_err().contains("superseded"));
    disable.await.unwrap();
    assert!(!fixture.manager.lock().await.store.load().unwrap().servers[0].clipboard_enabled);
    assert!(owner.permissions.browser_enabled());
    fixture.stop().await;
}

#[tokio::test]
async fn server_editor_cannot_replay_old_integration_choices_or_transfer_consent() {
    let fixture = Fixture::start().await;
    let mut server = server();
    server.browser_enabled = true;
    server.clipboard_enabled = true;
    server.agent_install_approved = true;
    server.setup_complete = true;
    let (owner, expected) = {
        let mut manager = fixture.manager.lock().await;
        manager
            .save(crate::model::Config {
                servers: vec![server.clone()],
                tunnels: vec![],
            })
            .await
            .unwrap();
        (
            manager.sessions.get(server.id).unwrap(),
            manager.connection_scope(server.id).unwrap(),
        )
    };
    fixture
        .call(
            "set_clipboard_enabled",
            json!({"id":server.id,"enabled":false,"expectedConnection":expected}),
        )
        .await
        .unwrap();
    // An editor opened before the toggle still carries clipboardEnabled=true.
    server.name = "Renamed after off".into();
    fixture
        .call("save_server", json!({"server":server,"password":null}))
        .await
        .unwrap();
    {
        let manager = fixture.manager.lock().await;
        let saved = manager.server(server.id).unwrap();
        assert!(!saved.clipboard_enabled);
        assert!(saved.browser_enabled);
        assert_eq!(manager.connection_scope(server.id).unwrap(), expected);
        assert!(Arc::ptr_eq(
            &manager.sessions.get(server.id).unwrap(),
            &owner
        ));
    }
    server.ssh_port += 1;
    fixture
        .call("save_server", json!({"server":server,"password":null}))
        .await
        .unwrap();
    {
        let manager = fixture.manager.lock().await;
        let saved = manager.server(server.id).unwrap();
        assert!(!saved.browser_enabled && !saved.clipboard_enabled);
        assert!(!saved.agent_install_approved && !saved.setup_complete);
        assert_ne!(manager.connection_scope(server.id).unwrap(), expected);
        assert!(owner.is_revoked());
        let replacement = manager.sessions.get(server.id).unwrap();
        assert_eq!(replacement.transport_snapshot().demand, 0);
        assert!(!replacement.permissions.snapshot().desired.browser);
    }
    fixture.stop().await;
}
