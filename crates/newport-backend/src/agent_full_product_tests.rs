//! Run explicitly with full binary resources and the disposable SSH fixture.
use crate::{agent::Service, agent_lifecycle::BuildRelation};
use newport_protocol::encoding::protobuf::{self, tokio::FrameReader};
use newport_protocol::product::Variant;
use tokio::io::AsyncWriteExt;

#[tokio::test]
#[ignore = "Requires full NEWPORT_AGENT_RESOURCE_DIR and disposable Linux OpenSSH fixture"]
async fn full_product_install_opens_registered_gui_and_core_workers() {
    assert_eq!(crate::agent_payload::variant(), Variant::Full);
    let server: crate::model::Server = serde_json::from_value(serde_json::json!({
        "id": uuid::Uuid::new_v4(), "name": "Full product fixture", "sshUser": "fixture",
        "sshHost": "127.0.0.1", "sshPort": std::env::var("NEWPORT_TEST_SSH_PORT").unwrap().parse::<u16>().unwrap(),
        "identityFile": std::env::var("NEWPORT_TEST_IDENTITY").unwrap(), "agentInstallApproved": false
    })).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let mut manager =
        crate::manager::Manager::new(crate::profile::Store::for_test(directory.path().into()));
    manager
        .save(crate::model::Config {
            servers: vec![server.clone()],
            tunnels: vec![],
        })
        .await
        .unwrap();
    let owner = manager
        .sessions
        .session(&server, manager.connection_revision(server.id));
    let state = crate::manager::share(manager);
    let registry = state.sessions.clone();
    crate::remote::command::stdout(
        owner.link(),
        r#"legacy="$HOME/.cache/newport-remote/releases/system/legacy/current"
mkdir -p "$legacy"
printf '#!/bin/sh\nexit 91\n' > "$legacy/run-agent"
chmod +x "$legacy/run-agent"
touch "$legacy/.validated-v1"
"#,
        None,
    )
    .await
    .unwrap();
    let before = crate::remote_apps::status(&owner).await.unwrap();
    assert!(before["agent"].is_null());
    assert!(before["legacyAgent"].is_string());
    assert_eq!(before["bundledVariant"], "full");
    // Remote Apps invokes the same consent/install flow as Needle. Installing
    // the full product must not enable clipboard/browser sharing or finish setup.
    crate::server_setup::install(&state, server.clone())
        .await
        .unwrap();
    let configured = state.snapshot().config.servers[0].clone();
    assert!(configured.agent_install_approved);
    assert!(!configured.setup_complete);
    assert!(!owner.permissions.browser_enabled());
    assert!(!owner.permissions.clipboard_enabled());
    let gui_status = crate::remote_apps::status(&owner).await.unwrap();
    assert_eq!(gui_status["agent"], "newport-agent");
    assert_eq!(gui_status["installedVariant"], "full");
    assert_eq!(gui_status["selectedRuntime"]["product"], "full");
    assert_eq!(
        gui_status["selectedRuntime"]["buildSha256"],
        gui_status["bundledRevision"]
    );
    assert!(gui_status["agentError"].is_null());
    assert_eq!(
        gui_status["expectedConnection"]["id"],
        server.id.to_string()
    );
    assert!(gui_status["legacyAgent"].is_string());
    assert!(crate::remote_apps::catalogue(&owner).await.unwrap()["apps"].is_array());
    let status = owner.check_installation().await.unwrap();
    assert_eq!(status.installed_variant, Some(Variant::Full));
    assert_eq!(status.build_relation, BuildRelation::Bundled);
    let monitor_lease = owner.needle(&server).unwrap();
    let mut monitor = monitor_lease.launch(Service::Monitor).await.unwrap();
    let runtime = owner.agent_runtime.snapshot().hello.unwrap();
    assert_eq!(runtime.product, newport_protocol::product::Variant::Full);
    let catalogue_lease = owner.needle(&server).unwrap();
    let (mut channel, worker) = catalogue_lease
        .launch_channel(Service::RemoteApps {
            arguments: vec!["--catalog".into()],
        })
        .await
        .unwrap();
    let (stdout, stderr, status) = channel_output(&mut channel).await;
    assert_eq!(status, Some(0), "{}", String::from_utf8_lossy(&stderr));
    newport_protocol::remote_apps::apps::wire::decode_complete(&stdout)
        .unwrap()
        .into_result()
        .unwrap();
    drop(worker);
    drop(catalogue_lease);

    let endpoint = directory.path().join("gui.sock");
    let rpc = crate::rpc::Server::bind(&endpoint).unwrap();
    let serving = tokio::spawn(rpc.run(GuiHost(owner.clone())));
    let path = endpoint.clone();
    let id = server.id.to_string();
    let (stdout, report) = tokio::task::spawn_blocking(move || {
        relay_worker(path, &id, vec!["--invalid-option".into()], b"")
    })
    .await
    .unwrap();
    assert!(stdout.is_empty());
    assert_eq!(report["status"], 1);
    assert!(
        report["stderr"]
            .as_str()
            .unwrap()
            .contains("Remote Apps worker exited with status")
    );
    // The real native client, RPC relay and registered SSH worker all share
    // the same runtime. An ended GUI channel releases its worker permit.
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if owner
                .agent_runtime
                .snapshot()
                .instances
                .iter()
                .map(|i| i.worker_leases)
                .sum::<usize>()
                == 1
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    serving.abort();
    let _ = serving.await;
    let gui_lease = owner.needle(&server).unwrap();
    let mut gui = gui_lease
        .launch(Service::RemoteApps {
            arguments: vec!["--attach".into()],
        })
        .await
        .unwrap();
    // Entitlement still belongs to the private service. Its rejection must not
    // replace the installation or kill the shared runtime/core feature channel.
    use newport_protocol::remote_apps::{
        self,
        v1::{ClientMessage, ServerMessage},
    };
    let hello = ClientMessage::Hello {
        versions: vec![1],
        features: Default::default(),
        license: None,
        performance: None,
        display: None,
    };
    gui.write_all(&remote_apps::wire::encode_client(&hello).unwrap())
        .await
        .unwrap();
    let mut gui = FrameReader::new(gui, remote_apps::SERVER_LIMITS);
    let body = gui.next().await.unwrap().unwrap();
    let mut framed = Vec::new();
    protobuf::write_frame(&mut framed, &body).unwrap();
    assert!(matches!(
        remote_apps::wire::read_server(&mut framed.as_slice())
            .unwrap()
            .0,
        ServerMessage::Rejected { .. }
    ));
    drop(gui);
    use newport_protocol::monitor::{self, Call, Reply, Request};
    let call = Call {
        id: 1,
        request: Request::Metrics,
    };
    monitor
        .write_all(&monitor::wire::encode_request(&call).unwrap())
        .await
        .unwrap();
    let mut monitor = FrameReader::new(monitor, monitor::SERVER_LIMITS);
    let body = monitor.next().await.unwrap().unwrap();
    assert!(matches!(
        monitor::wire::decode_reply(body, &call).unwrap().reply,
        Reply::Metrics(_)
    ));
    assert_eq!(
        owner.agent_runtime.snapshot().hello.unwrap().instance_id,
        runtime.instance_id
    );
    assert_eq!(
        owner.check_installation().await.unwrap().installed_variant,
        Some(Variant::Full)
    );
    drop(monitor);
    registry.shutdown().await;
}

async fn channel_output(
    channel: &mut crate::service_channel::Channel,
) -> (Vec<u8>, Vec<u8>, Option<u32>) {
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        let (mut stdout, mut stderr, mut status) = (Vec::new(), Vec::new(), None);
        while let Some(message) = channel.wait().await.unwrap() {
            match message {
                russh::ChannelMsg::Data { data } => stdout.extend_from_slice(&data),
                russh::ChannelMsg::ExtendedData { data, .. } => stderr.extend_from_slice(&data),
                russh::ChannelMsg::ExitStatus { exit_status } => status = Some(exit_status),
                _ => (),
            }
            assert!(stdout.len() < 8 * 1024 * 1024 && stderr.len() < 65536);
        }
        (stdout, stderr, status)
    })
    .await
    .expect("registered worker did not exit")
}

/// Exercise the production relay against an actual registered SSH worker.
struct GuiHost(std::sync::Arc<crate::server_session::ServerSession>);
impl crate::rpc::Host for GuiHost {
    fn instance_id(&self) -> uuid::Uuid {
        uuid::Uuid::from_u128(1)
    }
    async fn dispatch(
        &self,
        _: &newport_protocol::desktop::Operation,
    ) -> Result<newport_protocol::desktop::Reply, newport_protocol::desktop::Failure> {
        Err(crate::rpc::failure::operation(
            "Unexpected general dispatch",
        ))
    }
    async fn open_terminal(
        &self,
        _: crate::rpc::TerminalOpen,
    ) -> Result<crate::rpc::TerminalChannel, newport_protocol::desktop::Failure> {
        Err(crate::rpc::failure::operation("Unexpected terminal"))
    }
    async fn open_remote_apps(
        &self,
        server: uuid::Uuid,
        arguments: Vec<String>,
    ) -> Result<crate::rpc::RemoteChannel, newport_protocol::desktop::Failure> {
        assert_eq!(server, self.0.server().id);
        let lease = self
            .0
            .needle(self.0.server())
            .map_err(crate::rpc::failure::operation)?;
        let (channel, worker) = lease
            .launch_channel(Service::RemoteApps { arguments })
            .await
            .map_err(crate::rpc::failure::runtime)?;
        Ok(crate::rpc::RemoteChannel {
            channel,
            guard: Box::new((worker, lease)),
        })
    }
}
fn relay_worker(
    path: std::path::PathBuf,
    server: &str,
    arguments: Vec<String>,
    input: &[u8],
) -> (Vec<u8>, serde_json::Value) {
    use newport_protocol::{
        desktop::{self, stream},
        worker::{self, Direction, Transport, wire},
    };
    use std::time::{Duration, Instant};
    let mut opened = newport_client::backend::Backend::at_path(path)
        .open(
            stream::Operation::RemoteAppsOpen(Box::new(stream::RemoteAppsOpenArgs {
                server: server.parse().unwrap(),
                arguments,
            })),
            Duration::from_secs(10),
        )
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    if !input.is_empty() {
        opened.writer.send_until(input, deadline).unwrap();
    }
    let limits = Transport::REMOTE_APPS.limits(Direction::FromWorker);
    let mut decoder = wire::ServerDecoder::default();
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    loop {
        let frame = opened
            .reader
            .next_until(limits.message, deadline)
            .unwrap()
            .expect("worker final outcome");
        match decoder.decode(frame.body(), &limits).unwrap() {
            wire::Frame::Service(_) => stdout.extend_from_slice(&frame.into_bytes()),
            wire::Frame::Control(worker::ServerControl::Stderr(bytes)) => {
                stderr.extend_from_slice(bytes.data())
            }
            wire::Frame::Control(worker::ServerControl::Exit(exit)) => {
                assert!(
                    opened
                        .reader
                        .next_until(limits.message, Instant::now() + desktop::CLOSURE_TIMEOUT)
                        .unwrap()
                        .is_none(),
                    "trailing worker data"
                );
                let (status, signal) = match exit {
                    worker::Exit::Status(status) => (Some(status), None),
                    worker::Exit::Signal(signal) => (None, Some(signal.number())),
                };
                return (
                    stdout,
                    serde_json::json!({"status":status,"signal":signal,"stderr":String::from_utf8(stderr).unwrap()}),
                );
            }
        }
        assert!(stdout.len() <= 65536 && stderr.len() <= 65536);
    }
}
