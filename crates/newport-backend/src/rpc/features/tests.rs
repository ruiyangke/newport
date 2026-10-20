use super::*;
use crate::{manager::Manager, model::Server, profile::Store, rpc};
use std::{path::PathBuf, time::Duration};
use uuid::Uuid;

struct FeatureHost {
    manager: Shared,
    metrics: MetricsDb,
    terminals: Sessions,
    files: crate::files::Operations,
}
impl rpc::Host for FeatureHost {
    async fn dispatch(&self, method: &str, args: Value) -> Result<Value, Value> {
        Command::parse(method)
            .ok_or_else(|| json!("Unknown native command"))?
            .dispatch(
                &Services {
                    manager: &self.manager,
                    metrics: &self.metrics,
                    terminals: &self.terminals,
                    files: &self.files,
                },
                &args,
            )
            .await
    }
    async fn open_terminal(&self, _: rpc::TerminalOpen) -> Result<Box<dyn Send>, String> {
        Err("Terminal leases have their own fixtures".into())
    }
}

struct Fixture {
    directory: tempfile::TempDir,
    endpoint: PathBuf,
    manager: Shared,
    metrics: MetricsDb,
    server: Server,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
    shutdown: tokio::sync::oneshot::Sender<()>,
}
impl Fixture {
    async fn start() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let endpoint = directory.path().join("native-client/desktop.sock");
        let mut manager = Manager::new(Store::for_test(directory.path().into()));
        let server: Server = serde_json::from_value(json!({
            "id":Uuid::new_v4(), "name":"Isolated feature fixture",
            "sshHost":"127.0.0.1", "sshPort":9, "sshUser":"fixture"
        }))
        .unwrap();
        manager.config.servers.push(server.clone());
        let manager = crate::manager::share(manager);
        let metrics = MetricsDb::open(&directory.path().join("metrics.db"), Ok(vec![7; 32]))
            .await
            .unwrap();
        let host = FeatureHost {
            manager: manager.clone(),
            metrics: metrics.clone(),
            terminals: Sessions::default(),
            files: crate::files::Operations::default(),
        };
        let listener = rpc::Server::bind(&endpoint).unwrap();
        let (shutdown, stop) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(listener.run_until(host, async {
            let _ = stop.await;
        }));
        Self {
            directory,
            endpoint,
            manager,
            metrics,
            server,
            task,
            shutdown,
        }
    }
    async fn call(&self, method: &'static str, args: Value) -> Result<Value, String> {
        let endpoint = self.endpoint.clone();
        tokio::task::spawn_blocking(move || {
            newport_client::backend::Backend::at_path(endpoint).call_with_timeout(
                method,
                args,
                Duration::from_secs(10),
            )
        })
        .await
        .unwrap()
    }
    async fn stop(self) {
        self.shutdown.send(()).unwrap();
        self.task.await.unwrap().unwrap();
        self.metrics.close().await;
        drop(self.manager);
        drop(self.metrics);
        // Windows can retain a transient sharing violation after all service
        // handles are closed. Retry only that OS condition, with a deadline;
        // a persistent leaked handle must still fail teardown.
        #[cfg(windows)]
        {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            loop {
                match std::fs::remove_dir_all(self.directory.path()) {
                    Ok(()) => break,
                    Err(error)
                        if matches!(error.raw_os_error(), Some(32 | 145))
                            && std::time::Instant::now() < deadline =>
                    {
                        tokio::time::sleep(Duration::from_millis(25)).await;
                    }
                    Err(error) => panic!("Feature fixture cleanup failed: {error}"),
                }
            }
        }
        #[cfg(not(windows))]
        self.directory.close().unwrap();
    }
}

#[tokio::test]
async fn native_client_reads_and_clears_real_metric_storage() {
    let fixture = Fixture::start().await;
    let snapshot = fixture.call("snapshot", json!({})).await.unwrap();
    assert_eq!(
        snapshot["config"]["servers"][0]["id"],
        fixture.server.id.to_string()
    );
    let at = crate::metrics_db::now_ms();
    fixture
        .metrics
        .record(
            &fixture.server,
            at,
            &json!({
                "cpu":42, "network":[], "processes":["private command"], "password":"private"
            }),
        )
        .await
        .unwrap();
    let args = json!({"id":fixture.server.id});
    let history = fixture.call("cockpit_history", args.clone()).await.unwrap();
    assert_eq!(history.as_array().unwrap().len(), 1);
    assert_eq!(history[0]["at"], at);
    assert_eq!(history[0]["data"], json!({"cpu":42,"network":[]}));
    let cache = fixture.call("get_metrics_cache", json!({})).await.unwrap();
    assert_eq!(cache["samples"], 1);
    assert!(cache["bytes"].as_u64().unwrap() > 0);

    // An edited profile must not expose the old endpoint's history.
    fixture.manager.lock().await.config.servers[0].ssh_host = "new-endpoint".into();
    assert_eq!(
        fixture.call("cockpit_history", args).await.unwrap(),
        json!([])
    );
    let cleared = fixture
        .call("clear_metrics_cache", json!({}))
        .await
        .unwrap();
    assert_eq!(cleared["samples"], 0);
    assert_eq!(fixture.metrics.cache_info().await.unwrap().samples, 0);
    fixture.stop().await;
}

#[tokio::test]
async fn native_feature_routes_preserve_validation_before_any_remote_action() {
    let fixture = Fixture::start().await;
    let id = fixture.server.id;
    let session = Uuid::new_v4();
    for (method, args, expected) in [
        (
            "files_list",
            json!({"id":id,"operation":Uuid::new_v4(),"path":"/","expectedConnection":[]}),
            "connection changed",
        ),
        (
            "files_preview",
            json!({"id":id,"operation":Uuid::new_v4(),"path":"/file","expectedConnection":[]}),
            "connection changed",
        ),
        (
            "native.files_upload",
            json!({"id":id,"operation":Uuid::new_v4(),"path":"/","localPath":std::ffi::OsString::from("relative"),"expectedConnection":[]}),
            "selected local path",
        ),
        (
            "native.files_download",
            json!({"id":id,"operation":Uuid::new_v4(),"path":"/file","expectedConnection":[]}),
            "localPath",
        ),
        ("services_snapshot", json!({"id":"invalid"}), "Invalid id"),
        (
            "service_action",
            json!({"id":id,"unit":"-bad.service","scope":"system","verb":"start"}),
            "Not a valid unit name",
        ),
        (
            "service_logs",
            json!({"id":id,"scope":"system","cursor":"bad\ncursor"}),
            "Invalid journal cursor",
        ),
        (
            "service_unit_file",
            json!({"id":id,"unit":"name.socket","scope":"system"}),
            "Not a valid unit name",
        ),
        (
            "container_action",
            json!({"id":id,"container":"--bad","verb":"start"}),
            "Not a valid container",
        ),
        (
            "containers_remove",
            json!({"id":id,"containers":[]}),
            "Choose 1",
        ),
        (
            "docker_prune",
            json!({"id":id,"what":"unknown"}),
            "Invalid what",
        ),
        (
            "compose_run",
            json!({"id":id,"project":"fixture","action":{"kind":"scale","service":"web","replicas":21}}),
            "Replicas must be",
        ),
        (
            "compose_job",
            json!({"id":id,"job":"../bad"}),
            "Not a valid Compose command",
        ),
        (
            "terminal_open",
            json!({"id":id,"session":session,"cols":0,"rows":24}),
            "Invalid terminal dimensions",
        ),
        (
            "terminal_resize",
            json!({"session":session,"cols":80,"rows":301}),
            "Invalid terminal dimensions",
        ),
        (
            "terminal_write",
            json!({"session":session,"data":vec![0;65537]}),
            "Terminal input is too large",
        ),
        ("fixture.unknown", json!({}), "Unknown native command"),
    ] {
        let error = fixture.call(method, args).await.unwrap_err();
        assert!(error.contains(expected), "{method}: {error}");
    }
    // Rejected terminal opens did not insert a session or start SSH.
    let error = fixture
        .call("terminal_read", json!({"session":session}))
        .await
        .unwrap_err();
    assert!(error.contains("Terminal is closed"), "{error}");
    assert_eq!(
        fixture
            .call("terminal_close", json!({"session":session}))
            .await
            .unwrap(),
        Value::Null
    );
    assert!(fixture.call("snapshot", json!({})).await.is_ok());
    fixture.stop().await;
}

#[tokio::test]
#[ignore = "Run with npm run test:ssh"]
async fn native_file_client_roundtrip_preserves_scope_and_cancellation() {
    let mut fixture = Fixture::start().await;
    fixture.server.ssh_port = std::env::var("NEWPORT_TEST_SSH_PORT")
        .unwrap()
        .parse()
        .unwrap();
    fixture.server.identity_file = Some(std::env::var("NEWPORT_TEST_IDENTITY").unwrap());
    fixture.manager.lock().await.config.servers[0] = fixture.server.clone();
    let snapshot = fixture.call("snapshot", json!({})).await.unwrap();
    let scope =
        newport_domain::connection_scope::connection(&snapshot["config"]["servers"][0], &snapshot);
    let id = fixture.server.id;
    let listing = fixture
        .call(
            "files_list",
            json!({
                "id":id,"operation":Uuid::new_v4(),"path":"/Documents","expectedConnection":scope
            }),
        )
        .await
        .unwrap();
    assert_eq!(listing["path"], "/Documents");
    let payload = "native RPC transfer ' 世界\n".repeat(12000);
    let local = fixture
        .directory
        .path()
        .join(format!("native-{}.txt", Uuid::new_v4()));
    std::fs::write(&local, &payload).unwrap();
    let uploaded = fixture
        .call(
            "native.files_upload",
            json!({
                "id":id,"operation":Uuid::new_v4(),"path":"/Documents",
                "localPath":local.as_os_str(),"expectedConnection":scope
            }),
        )
        .await
        .unwrap();
    let remote = uploaded.as_str().unwrap();
    let preview = fixture
        .call(
            "files_preview",
            json!({
                "id":id,"operation":Uuid::new_v4(),"path":remote,"expectedConnection":scope
            }),
        )
        .await
        .unwrap();
    assert_eq!(preview["kind"], "text");
    assert_eq!(preview["content"], payload);
    let destination = fixture.directory.path().join("download.txt");
    std::fs::write(&destination, "old destination").unwrap();
    fixture
        .call(
            "native.files_download",
            json!({
                "id":id,"operation":Uuid::new_v4(),"path":remote,
                "localPath":destination.as_os_str(),"expectedConnection":scope
            }),
        )
        .await
        .unwrap();
    assert_eq!(std::fs::read_to_string(&destination).unwrap(), payload);

    let operation = Uuid::new_v4();
    assert_eq!(
        fixture
            .call("files_cancel", json!({"operation":operation}))
            .await
            .unwrap(),
        Value::Null
    );
    let cancelled = fixture
        .call(
            "native.files_download",
            json!({
                "id":id,"operation":operation,"path":remote,
                "localPath":destination.as_os_str(),"expectedConnection":scope
            }),
        )
        .await
        .unwrap_err();
    assert!(cancelled.contains("cancelled"), "{cancelled}");
    assert_eq!(
        fixture
            .call("files_progress", json!({"operation":operation}))
            .await
            .unwrap(),
        Value::Null
    );
    assert_eq!(std::fs::read_to_string(&destination).unwrap(), payload);

    fixture.manager.lock().await.advance_connection_revision(id);
    let rejected = fixture
        .call(
            "native.files_upload",
            json!({
                "id":id,"operation":Uuid::new_v4(),"path":"/Documents",
                "localPath":local.as_os_str(),"expectedConnection":scope
            }),
        )
        .await
        .unwrap_err();
    assert!(rejected.contains("connection changed"), "{rejected}");
    fixture.stop().await;
}
