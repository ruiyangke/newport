use super::*;
use crate::{
    git::{pending::ReceiptState, protocol::Request},
    manager::Manager,
    model::Server,
    profile::Store,
    rpc,
};
use serde_json::json;
use std::{path::PathBuf, sync::Arc};
use uuid::Uuid;

struct Identity(Result<String, String>);
impl ClientIdentity for Identity {
    fn git_client_id(&self) -> crate::git::IdentityFuture {
        Box::pin(std::future::ready(self.0.clone()))
    }
}
struct GitHost {
    identity: Arc<dyn ClientIdentity>,
    manager: Shared,
    pending: PendingOperations,
    events: rpc::EventLog,
}
impl rpc::Host for GitHost {
    async fn dispatch(&self, method: &str, args: Value) -> Result<Value, Value> {
        let events = self.events.clone();
        Command::parse(method)
            .ok_or_else(|| json!("Unknown command"))?
            .dispatch(
                &Services {
                    identity: self.identity.as_ref(),
                    manager: &self.manager,
                    pending: &self.pending,
                    events: Arc::new(move |event| events.record(event)),
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
    server: Server,
    pending: PendingOperations,
    manager: Shared,
    identity: Arc<dyn ClientIdentity>,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
}
impl Fixture {
    fn start(server: Server, identity: Result<String, String>) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let endpoint = directory.path().join("native-client/desktop.sock");
        let mut manager = Manager::new(Store::for_test(directory.path().into()));
        manager.config.servers.push(server.clone());
        let pending = PendingOperations::new(directory.path().join("git-operations.json"));
        let listener = rpc::Server::bind(&endpoint).unwrap();
        let identity: Arc<dyn ClientIdentity> = match identity {
            Ok(id) => {
                // The real-SSH fixture uses the production preference store,
                // including persistence before the identity reaches an agent.
                std::fs::write(
                    directory.path().join("preferences.json"),
                    serde_json::to_vec(&json!({"gitClientId":id})).unwrap(),
                )
                .unwrap();
                Arc::new(crate::preferences::Preferences::open(directory.path()).unwrap())
            }
            Err(error) => Arc::new(Identity(Err(error))),
        };
        let manager = crate::manager::share(manager);
        let host = GitHost {
            identity: identity.clone(),
            manager: manager.clone(),
            pending: pending.clone(),
            events: listener.events(),
        };
        let task = tokio::spawn(listener.run(host));
        Self {
            directory,
            endpoint,
            server,
            pending,
            manager,
            identity,
            task,
        }
    }
    fn offline() -> Self {
        let server = serde_json::from_value(json!({
            "id":Uuid::new_v4(), "name":"Isolated Git fixture",
            "sshHost":"127.0.0.1", "sshPort":9, "sshUser":"fixture"
        }))
        .unwrap();
        Self::start(server, Err("Identity could not be persisted".into()))
    }
    async fn call(&self, method: &'static str, args: Value) -> Result<Value, String> {
        let endpoint = self.endpoint.clone();
        tokio::task::spawn_blocking(move || {
            newport_client::backend::Backend::at_path(endpoint)
                .call_detailed(method, args)
                .map_err(|error| format!("{}: {}", error.code.unwrap_or_default(), error.message))
        })
        .await
        .unwrap()
    }
    async fn request(&self, request: Request) -> Result<Value, String> {
        self.call(
            "git_request",
            json!({"serverId":self.server.id,"request":request}),
        )
        .await
    }
    async fn stop(self) {
        crate::git::git_disconnect(&self.manager, self.server.id)
            .await
            .unwrap();
        self.manager.lock().await.shutdown().await;
        self.task.abort();
        assert!(self.task.await.unwrap_err().is_cancelled());
        self.directory.close().unwrap();
    }
}

#[tokio::test]
async fn explicit_checkout_retry_clears_cached_transport_failure_through_rpc() {
    let fixture = Fixture::offline();
    let owner = fixture.manager.owner(fixture.server.id).unwrap();
    assert!(owner.acquire(&fixture.server).await.is_err());
    let failure = owner.transport_snapshot().error;
    assert!(failure.is_some());
    let mut args = json!({
        "serverId":fixture.server.id,
        "request":{"method":"repo.open","params":{"path":{
            "bytesB64":"L3Nydi9hcHA=","display":"/srv/app"
        }}}
    });
    assert!(fixture.call("git_request", args.clone()).await.is_err());
    assert_eq!(owner.transport_snapshot().error, failure);
    args["retryConnection"] = json!(true);
    let mut other = args.clone();
    other["request"] = json!({"method":"repo.close","params":{"repoId":"checkout"}});
    assert!(fixture.call("git_request", other).await.is_err());
    assert_eq!(owner.transport_snapshot().error, failure);
    // Identity persistence is deliberately refused by this fixture, so we can
    // observe the reset before a new connection has a chance to fail again.
    let error = fixture.call("git_request", args).await.unwrap_err();
    assert!(error.contains("Identity could not be persisted"), "{error}");
    assert!(owner.transport_snapshot().error.is_none());
    fixture.stop().await;
}

#[tokio::test]
async fn repository_refresh_is_local_and_does_not_wait_for_configuration_storage() {
    let fixture = Fixture::offline();
    let writer = fixture.manager.lock().await;
    // This fixture cannot establish SSH or persist its Git client identity.
    // Invalidating a closed cache must still succeed under a held writer.
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        fixture.call(
            "git_refresh_repository",
            json!({"serverId":fixture.server.id,"repoId":"checkout-one"}),
        ),
    )
    .await
    .expect("refresh queued behind configuration")
    .unwrap();
    assert_eq!(result, json!({"repoId":"checkout-one"}));
    for args in [
        json!({"serverId":fixture.server.id,"repoId":""}),
        json!({"serverId":fixture.server.id}),
        json!({"serverId":Uuid::new_v4(),"repoId":"checkout-one"}),
    ] {
        assert!(fixture.call("git_refresh_repository", args).await.is_err());
    }
    drop(writer);
    fixture.stop().await;
}

#[tokio::test]
async fn native_git_receipts_preserve_server_scope_and_require_confirmed_outcomes() {
    let fixture = Fixture::offline();
    let id = Uuid::new_v4();
    fixture
        .pending
        .begin(fixture.server.id, id, "commit".into())
        .unwrap();
    let args = json!({"serverId":fixture.server.id,"operationId":id});
    let stranger = json!({"serverId":Uuid::new_v4(),"operationId":id});
    assert_eq!(
        fixture
            .call("git_pending_operations", stranger.clone())
            .await
            .unwrap(),
        json!([])
    );
    fixture
        .call("git_acknowledge_operation", stranger)
        .await
        .unwrap();
    let rows = fixture
        .call("git_pending_operations", args.clone())
        .await
        .unwrap();
    assert_eq!(rows[0]["operationId"], id.to_string());
    assert_eq!(rows[0]["state"], "pending");
    assert!(
        fixture
            .call("git_acknowledge_operation", args.clone())
            .await
            .unwrap_err()
            .contains("no confirmed outcome")
    );
    assert!(
        fixture
            .call("git_review_operation", args.clone())
            .await
            .unwrap_err()
            .contains("OPERATION_NOT_REVIEWABLE")
    );
    fixture
        .pending
        .observe(fixture.server.id, id, ReceiptState::Succeeded)
        .unwrap();
    fixture
        .call("git_acknowledge_operation", args.clone())
        .await
        .unwrap();
    assert_eq!(
        fixture.call("git_pending_operations", args).await.unwrap(),
        json!([])
    );
    fixture.stop().await;
}

mod remote_tests {
    mod git {
        use super::super::*;
        use crate::{
            git::protocol::{Action, Author, Path},
            ssh::ExecSession,
        };

        #[tokio::test]
        #[ignore = "Requires cargo xtask test remote and real OpenSSH"]
        async fn native_client_commit_recovery_and_logs() {
            commit_across_runtime_change(false).await;
        }
        #[tokio::test]
        #[ignore = "Requires disposable Linux OpenSSH fixture"]
        async fn native_client_commit_survives_approved_upgrade_and_next_request_migrates() {
            commit_across_runtime_change(true).await;
        }
        async fn commit_across_runtime_change(upgrade: bool) {
            let server = serde_json::from_value(json!({
                "id":Uuid::new_v4(), "name":"Disposable native Git",
                "agentInstallApproved":true,
                "sshHost":"127.0.0.1", "sshUser":"fixture",
                "sshPort":std::env::var("NEWPORT_TEST_SSH_PORT").unwrap().parse::<u16>().unwrap(),
                "identityFile":std::env::var("NEWPORT_TEST_IDENTITY").unwrap()
            }))
            .unwrap();
            let fixture = Fixture::start(server, Ok(Uuid::new_v4().to_string()));
            let args = json!({"serverId":fixture.server.id});
            let setup = ExecSession::connect(&fixture.server).await.unwrap();
            crate::agent_lifecycle::manage_test(&setup, false, false)
                .await
                .unwrap();
            let old_hash = if upgrade {
                let hash = crate::agent_lifecycle::activate_variant_test(&setup).await;
                let owner = fixture
                    .manager
                    .lock()
                    .await
                    .sessions
                    .session(&fixture.server, 0);
                owner
                    .acquire(&fixture.server)
                    .await
                    .unwrap()
                    .adopt_build_for_test(&hash)
                    .await
                    .unwrap();
                Some(hash)
            } else {
                None
            };
            // All mutations are confined to a generated path in the disposable container.
            let root = format!("/home/fixture/native-git-{}", Uuid::new_v4());
            setup.execute(&format!("mkdir {root}"), None).await.unwrap();
            // Git requests below need a connected Git session.
            crate::git::git_connect(
                fixture.identity.as_ref(),
                &fixture.manager,
                fixture.server.id,
            )
            .await
            .unwrap();
            let init_id = Uuid::new_v4();
            let initialized = fixture
                .request(Request::Init {
                    operation_id: init_id.to_string(),
                    path: Path::new(root.as_bytes()),
                    initial_branch: "main".into(),
                })
                .await
                .unwrap();
            assert_eq!(initialized["state"], "succeeded", "{initialized}");
            setup.execute(&format!("git -C {root} config user.name Fixture && git -C {root} config user.email fixture@example.test && printf 'native Git fixture\\n' > {root}/sample.txt"), None).await.unwrap();
            let opened = fixture
                .request(Request::Open {
                    path: Path::new(root.as_bytes()),
                })
                .await
                .unwrap();
            let repo_id = opened["repoId"].as_str().unwrap().to_owned();
            let status_request = Request::Status {
                repo_id: repo_id.clone(),
                filter: None,
                page_size: 100,
                cursor: None,
            };
            let status = fixture.request(status_request.clone()).await.unwrap();
            assert_eq!(status["entries"].as_array().unwrap().len(), 1);
            let stage_id = Uuid::new_v4();
            let staged = fixture
                .request(Request::Start {
                    operation_id: stage_id.to_string(),
                    repo_id: repo_id.clone(),
                    expected_snapshot: status["snapshot"].as_str().unwrap().into(),
                    action: Action::Stage {
                        entry_ids: vec![status["entries"][0]["entryId"].as_str().unwrap().into()],
                        hunks: None,
                    },
                })
                .await
                .unwrap();
            assert_eq!(staged["state"], "succeeded", "{staged}");
            let status = fixture.request(status_request.clone()).await.unwrap();
            let commit_id = Uuid::new_v4();
            let commit = Request::Start {
                operation_id: commit_id.to_string(),
                repo_id: repo_id.clone(),
                expected_snapshot: status["snapshot"].as_str().unwrap().into(),
                action: Action::Commit {
                    message: "Native RPC commit".into(),
                    author: Some(Author {
                        name: "Fixture".into(),
                        email: "fixture@example.test".into(),
                    }),
                },
            };
            // Hold a real commit inside its hook while a different feature
            // admission rotates the exhausted runtime. The original operation
            // must finish once, with the same durable operation ID and logs.
            let hook = format!(
                "#!/bin/sh\ntouch {root}/.git/commit-entered\ni=0\nwhile test ! -f {root}/.git/commit-release; do\n i=$((i+1)); test \"$i\" -lt 100 || exit 1\n sleep 0.1\ndone\n"
            );
            setup.execute(
                &format!("mkdir -p {root}/.git/hooks && cat > {root}/.git/hooks/pre-commit && chmod 700 {root}/.git/hooks/pre-commit"),
                Some(hook.as_bytes()),
            ).await.unwrap();
            let owner = fixture
                .manager
                .lock()
                .await
                .sessions
                .get(fixture.server.id)
                .unwrap();
            let prior_runtime = owner.agent_runtime.snapshot().hello.unwrap().instance_id;
            if let Some(hash) = old_hash {
                assert_eq!(
                    owner.agent_runtime.snapshot().hello.unwrap().build_sha256,
                    hash
                );
            }
            let rotate = async {
                tokio::time::timeout(std::time::Duration::from_secs(5), async {
                    loop {
                        let entered = setup
                            .execute(
                                &format!(
                                    "if test -f {root}/.git/commit-entered; then printf yes; fi"
                                ),
                                None,
                            )
                            .await
                            .unwrap();
                        if entered == "yes" {
                            break;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    }
                })
                .await
                .expect("commit never entered its hook");
                let lease = owner.acquire(&fixture.server).await.unwrap();
                if upgrade {
                    lease.install(false).await.unwrap();
                } else {
                    owner.agent_runtime.exhaust_registration_budget_for_test();
                }
                let (stream, _) = lease.launch(crate::agent::Service::Git).await.unwrap();
                let (mut client, _) = crate::git::client::Client::start(stream).await.unwrap();
                client.ping().await.unwrap();
                assert_ne!(
                    owner.agent_runtime.snapshot().hello.unwrap().instance_id,
                    prior_runtime
                );
                assert_eq!(owner.transport_snapshot().generation, 1);
                setup
                    .execute(&format!("touch {root}/.git/commit-release"), None)
                    .await
                    .unwrap();
                (lease, client)
            };
            let (committed, _replacement) = tokio::join!(fixture.request(commit.clone()), rotate);
            let committed = committed.unwrap();
            assert_eq!(committed["state"], "succeeded", "{committed}");
            if upgrade {
                // Exercise the cached Git service before any explicit disconnect.
                fixture.request(status_request.clone()).await.unwrap();
                assert!(
                    owner
                        .agent_runtime
                        .snapshot()
                        .instances
                        .iter()
                        .all(|i| !i.retiring)
                );
            }
            let events = fixture.call("$events", json!({"after":0})).await.unwrap();
            let events = events["entries"].as_array().unwrap();
            assert!(
                !events.is_empty(),
                "Git command output must reach native RPC"
            );
            assert!(
                events
                    .iter()
                    .all(|event| event["serverId"] == fixture.server.id.to_string())
            );
            assert!(
                events
                    .iter()
                    .any(|event| event["operationId"] == commit_id.to_string()
                        && event["repoId"] == repo_id
                        && event["entry"].is_object())
            );

            crate::git::git_disconnect(&fixture.manager, fixture.server.id)
                .await
                .unwrap();
            // Reconnecting recovers the saved result; submitting the same write is rejected locally.
            let recovered = fixture
                .request(Request::Get {
                    operation_id: commit_id.to_string(),
                })
                .await
                .unwrap();
            assert_eq!(recovered, committed);
            let error = fixture.request(commit).await.unwrap_err();
            assert!(error.contains("already recorded"), "{error}");
            assert_eq!(
                setup
                    .execute(&format!("git -C {root} rev-list --count HEAD"), None)
                    .await
                    .unwrap()
                    .trim(),
                "1"
            );
            let rows = fixture
                .call("git_pending_operations", args.clone())
                .await
                .unwrap();
            assert_eq!(rows.as_array().unwrap().len(), 3);
            assert!(
                rows.as_array()
                    .unwrap()
                    .iter()
                    .all(|row| row["state"] == "succeeded")
            );
            for id in [init_id, stage_id, commit_id] {
                fixture
                    .call(
                        "git_acknowledge_operation",
                        json!({"serverId":fixture.server.id,"operationId":id}),
                    )
                    .await
                    .unwrap();
            }
            assert_eq!(
                fixture.call("git_pending_operations", args).await.unwrap(),
                json!([])
            );
            assert!(
                fixture.request(status_request).await.unwrap()["entries"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
            setup.close().await;
            fixture.stop().await;
        }
    }
}
