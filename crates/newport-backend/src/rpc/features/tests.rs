use super::*;
use crate::{manager::Manager, model::Server, profile::Store, rpc};
use newport_protocol::desktop::{files, metrics};
use serde_json::{Value, json};
use std::{path::PathBuf, time::Duration};
use uuid::Uuid;

struct FeatureHost {
    manager: Shared,
    metrics: MetricsDb,
    terminals: Sessions,
    files: crate::files::Operations,
    preferences: crate::preferences::Preferences,
}
impl rpc::Host for FeatureHost {
    fn instance_id(&self) -> Uuid {
        self.manager.instance_id
    }
    async fn dispatch(
        &self,
        command: &newport_protocol::desktop::Operation,
    ) -> Result<newport_protocol::desktop::Reply, newport_protocol::desktop::Failure> {
        use newport_protocol::desktop::{Operation, Reply};
        let state = Services {
            manager: &self.manager,
            metrics: &self.metrics,
            terminals: &self.terminals,
            files: &self.files,
            preferences: &self.preferences,
        };
        match command {
            Operation::Files(command) => rpc::files::dispatch(command, &state)
                .await
                .map(|reply| Reply::Files(Box::new(reply))),
            Operation::Metrics(command) => rpc::metrics::dispatch(command, &state)
                .await
                .map(|reply| Reply::Metrics(Box::new(reply))),
            Operation::Services(command) => services(command, &state)
                .await
                .map(|reply| Reply::Services(Box::new(reply))),
            Operation::Containers(command) => containers(command, &state)
                .await
                .map(|reply| Reply::Containers(Box::new(reply))),
            Operation::Terminal(command) => rpc::terminal::dispatch(command, &state)
                .await
                .map(|reply| Reply::Terminal(Box::new(reply))),
            Operation::Configuration(command)
                if matches!(
                    command.as_ref(),
                    newport_protocol::desktop::configuration::Operation::Snapshot(_)
                ) =>
            {
                Ok(Reply::Configuration(Box::new(
                    newport_protocol::desktop::configuration::Reply::Snapshot(Box::new(
                        rpc::configuration::snapshot(self.manager.snapshot())?,
                    )),
                )))
            }
            _ => Err(rpc::failure::operation("Unknown command")),
        }
    }
    async fn open_terminal(
        &self,
        request: rpc::TerminalOpen,
    ) -> Result<rpc::TerminalChannel, newport_protocol::desktop::Failure> {
        let io = crate::commands::terminal_open(
            &self.manager,
            &self.terminals,
            request.server,
            request.session,
            u32::from(request.columns),
            u32::from(request.rows),
        )
        .await
        .map_err(rpc::failure::operation)?;
        Ok(rpc::TerminalChannel {
            io,
            guard: Box::new(()),
        })
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
        let metrics = MetricsDb::open(&directory.path().join("metrics.db"), || None)
            .await
            .unwrap();
        let host = FeatureHost {
            manager: manager.clone(),
            metrics: metrics.clone(),
            terminals: Sessions::default(),
            files: crate::files::Operations::default(),
            preferences: crate::preferences::Preferences::open(directory.path()).unwrap(),
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
            rpc::test_fixture::call(endpoint, method, args, std::time::Duration::from_secs(30))
        })
        .await
        .unwrap()
    }
    async fn execute(
        &self,
        method: &'static str,
        args: Value,
    ) -> Result<newport_protocol::desktop::Reply, String> {
        let endpoint = self.endpoint.clone();
        tokio::task::spawn_blocking(move || {
            rpc::test_fixture::execute(endpoint, method, args, std::time::Duration::from_secs(30))
        })
        .await
        .unwrap()
    }
    async fn metrics(&self, method: &'static str, args: Value) -> metrics::Reply {
        match self.execute(method, args).await.unwrap() {
            newport_protocol::desktop::Reply::Metrics(reply) => *reply,
            other => panic!("expected a metrics reply, got {other:?}"),
        }
    }
    async fn files(&self, method: &'static str, args: Value) -> files::Reply {
        match self.execute(method, args).await.unwrap() {
            newport_protocol::desktop::Reply::Files(reply) => *reply,
            other => panic!("expected a files reply, got {other:?}"),
        }
    }
    async fn snapshot(&self) -> Result<newport_protocol::desktop::configuration::Snapshot, String> {
        let endpoint = self.endpoint.clone();
        tokio::task::spawn_blocking(move || {
            rpc::test_fixture::snapshot(endpoint, std::time::Duration::from_secs(30))
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
    let snapshot = fixture.snapshot().await.unwrap();
    assert_eq!(snapshot.config.servers[0].id, fixture.server.id);
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
    let metrics::Reply::History(history) = fixture.metrics("cockpit_history", args.clone()).await
    else {
        panic!("expected history");
    };
    assert_eq!(history.samples.len(), 1);
    assert_eq!(history.samples[0].at, at);
    let metrics::SampleValue::Raw(raw) = &history.samples[0].value else {
        panic!("expected a raw sample");
    };
    assert_eq!(raw.measures.cpu, Some(42.));
    assert_eq!(raw.measures.memory_total, None);
    assert!(raw.network.is_empty() && raw.uptime.is_none() && raw.disks.is_none());
    let metrics::Reply::Cache(cache) = fixture.metrics("get_metrics_cache", json!({})).await else {
        panic!("expected cache information");
    };
    assert_eq!(cache.samples, 1);
    assert!(cache.bytes > 0);

    // An edited profile must not expose the old endpoint's history.
    fixture.manager.lock().await.config.servers[0].ssh_host = "new-endpoint".into();
    assert!(matches!(
        fixture.metrics("cockpit_history", args).await,
        metrics::Reply::History(history) if history.samples.is_empty()
    ));
    let metrics::Reply::ClearCache(cleared) =
        fixture.metrics("clear_metrics_cache", json!({})).await
    else {
        panic!("expected cleared cache information");
    };
    assert_eq!(cleared.samples, 0);
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
        ("fixture.unknown", json!({}), "Unknown command"),
    ] {
        let error = fixture.call(method, args).await.unwrap_err();
        assert!(error.contains(expected), "{method}: {error}");
    }
    // Terminals open through a lease; an invalid size is refused there.
    let endpoint = fixture.endpoint.clone();
    let refused = tokio::task::spawn_blocking(move || {
        newport_client::backend::Backend::at_path(endpoint)
            .open(
                newport_protocol::desktop::stream::Operation::TerminalOpen(Box::new(
                    newport_protocol::desktop::stream::TerminalOpenArgs {
                        server: id,
                        session,
                        columns: 0,
                        rows: 24,
                        persist: false,
                        attach: false,
                        colors: None,
                    },
                )),
                Duration::from_secs(3),
            )
            .map(drop)
    })
    .await
    .unwrap()
    .unwrap_err();
    assert!(refused.to_string().contains("terminal"), "{refused}");
    let endpoint = fixture.endpoint.clone();
    let closed = tokio::task::spawn_blocking(move || {
        newport_client::backend::Backend::at_path(endpoint).execute_with_timeout(
            newport_protocol::desktop::Operation::Terminal(Box::new(
                newport_protocol::desktop::terminal::Operation::Close(Box::new(
                    newport_protocol::desktop::terminal::CloseArgs { session },
                )),
            )),
            Duration::from_secs(30),
        )
    })
    .await
    .unwrap()
    .unwrap();
    assert!(matches!(
        closed,
        newport_protocol::desktop::Reply::Terminal(reply)
            if matches!(*reply, newport_protocol::desktop::terminal::Reply::Close(_))
    ));
    assert!(fixture.snapshot().await.is_ok());
    fixture.stop().await;
}

#[tokio::test]
#[ignore = "Run with cargo xtask test ssh"]
async fn native_file_client_roundtrip_preserves_scope_and_cancellation() {
    let mut fixture = Fixture::start().await;
    fixture.server.ssh_port = std::env::var("NEWPORT_TEST_SSH_PORT")
        .unwrap()
        .parse()
        .unwrap();
    fixture.server.identity_file = Some(std::env::var("NEWPORT_TEST_IDENTITY").unwrap());
    fixture.manager.lock().await.config.servers[0] = fixture.server.clone();
    let snapshot = fixture.snapshot().await.unwrap();
    let revision = snapshot.runtime.connection_revision(fixture.server.id);
    let scope = crate::rpc::test_fixture::tuple(
        &fixture
            .server
            .connection_scope(revision, snapshot.instance_id),
    );
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
    let files::Reply::Preview(preview) = fixture
        .files(
            "files_preview",
            json!({
                "id":id,"operation":Uuid::new_v4(),"path":remote,"expectedConnection":scope
            }),
        )
        .await
    else {
        panic!("expected a preview");
    };
    assert_eq!(preview.kind, files::PreviewKind::Text);
    assert_eq!(
        preview.content,
        files::Content::Text(Box::new(payload.clone()))
    );
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
    assert!(matches!(
        fixture
            .files("files_progress", json!({"operation":operation}))
            .await,
        files::Reply::Progress(lookup) if lookup.progress.is_none()
    ));
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
