//! Run explicitly with full binary resources and the disposable SSH fixture.
use crate::{agent::Service, agent_lifecycle::BuildRelation};
use newport_release::agent_product::Variant;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

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
    let installer = owner.acquire(&server).await.unwrap();
    installer
        .execute(
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
    assert_eq!(
        gui_status["expectedConnection"]["id"],
        server.id.to_string()
    );
    assert!(gui_status["legacyAgent"].is_string());
    assert!(crate::remote_apps::catalogue(&owner).await.unwrap()["apps"].is_array());
    let status = installer.check_installation().await.unwrap();
    assert_eq!(status.installed_variant, Some(Variant::Full));
    assert_eq!(status.build_relation, BuildRelation::Bundled);
    let monitor_lease = owner.acquire(&server).await.unwrap();
    let (monitor, _) = monitor_lease.launch(Service::Monitor).await.unwrap();
    let mut monitor = BufReader::new(monitor);
    let runtime = owner.agent_runtime.snapshot().hello.unwrap();
    assert!(runtime.capabilities["product-full"].available);
    let catalogue_lease = owner.acquire(&server).await.unwrap();
    let (mut channel, worker) = catalogue_lease
        .launch_channel(Service::RemoteApps {
            arguments: vec!["--catalog".into()],
        })
        .await
        .unwrap();
    let (stdout, stderr, status) = channel_output(&mut channel).await;
    assert_eq!(status, Some(0), "{}", String::from_utf8_lossy(&stderr));
    let catalogue: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
    assert!(catalogue["apps"].is_array());
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
    let gui_lease = owner.acquire(&server).await.unwrap();
    let (mut gui, _) = gui_lease
        .launch(Service::RemoteApps {
            arguments: vec!["--display".into(), "private".into()],
        })
        .await
        .unwrap();
    // Entitlement still belongs to the private service. Its rejection must not
    // replace the installation or kill the shared runtime/core feature channel.
    gui.write_all(b"{\"type\":\"hello\",\"versions\":[1],\"capabilities\":[\"windows\",\"surface-generations\",\"tile-codecs-v1\"]}\n").await.unwrap();
    let size = gui.read_u32().await.unwrap();
    assert!(size <= 32768);
    let mut bytes = vec![0; size as usize];
    gui.read_exact(&mut bytes).await.unwrap();
    let rejected: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(rejected["type"], "rejected");
    drop(gui);
    monitor.write_all(b"{\"op\":\"snapshot\"}\n").await.unwrap();
    let mut line = String::new();
    monitor.read_line(&mut line).await.unwrap();
    let metrics: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert!(metrics.get("cpus").is_some());
    assert_eq!(
        owner.agent_runtime.snapshot().hello.unwrap().instance_id,
        runtime.instance_id
    );
    assert_eq!(
        installer
            .check_installation()
            .await
            .unwrap()
            .installed_variant,
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
    async fn dispatch(
        &self,
        _: &str,
        _: serde_json::Value,
    ) -> Result<serde_json::Value, serde_json::Value> {
        Err(serde_json::json!("Unexpected general dispatch"))
    }
    async fn open_terminal(&self, _: crate::rpc::TerminalOpen) -> Result<Box<dyn Send>, String> {
        Err("Unexpected terminal".into())
    }
    async fn open_remote_apps(
        &self,
        server: uuid::Uuid,
        arguments: Vec<String>,
    ) -> Result<crate::rpc::RemoteChannel, String> {
        assert_eq!(server, self.0.server().id);
        let lease = self.0.acquire(self.0.server()).await?;
        let (channel, worker) = lease
            .launch_channel(Service::RemoteApps { arguments })
            .await?;
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
    use newport_platform::ipc::protocol::{REMOTE_STREAM_DATA, REMOTE_STREAM_EXIT};
    use std::io::{Read, Write};
    let mut socket = newport_client::backend::Backend::at_path(path)
        .open_remote_apps(server, &arguments)
        .unwrap();
    socket
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .unwrap();
    socket.write_all(input).unwrap();
    let mut stdout = Vec::new();
    loop {
        let mut header = [0; 5];
        socket.read_exact(&mut header).unwrap();
        let size = u32::from_be_bytes(header[1..].try_into().unwrap()) as usize;
        assert!(size <= 65536);
        let mut payload = vec![0; size];
        socket.read_exact(&mut payload).unwrap();
        match header[0] {
            REMOTE_STREAM_DATA => stdout.extend_from_slice(&payload),
            REMOTE_STREAM_EXIT => return (stdout, serde_json::from_slice(&payload).unwrap()),
            other => panic!("Unexpected relay frame {other}"),
        }
        assert!(stdout.len() <= 65536);
    }
}
