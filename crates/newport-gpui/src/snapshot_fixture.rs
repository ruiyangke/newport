//! Configuration snapshots for tests, built from the protocol's own types.
use newport_protocol::desktop::{self, configuration::*};
use newport_protocol::{product, runtime};
use uuid::Uuid;

/// The server most fixtures select.
pub(crate) const SERVER: Uuid = Uuid::from_u128(0x00000000_0000_4000_8000_000000000001);
/// A second saved server.
pub(crate) const SECOND: Uuid = Uuid::from_u128(0x00000000_0000_4000_8000_000000000002);
/// The backend instance fixture snapshots come from.
pub(crate) const INSTANCE: Uuid = Uuid::from_u128(0x00000000_0000_4000_8000_000000000003);
/// A backend instance that replaced [`INSTANCE`].
pub(crate) const NEXT_INSTANCE: Uuid = Uuid::from_u128(0x00000000_0000_4000_8000_000000000004);

/// A connection scope of [`SERVER`] in [`INSTANCE`], told apart from other
/// scopes by its SSH host.
pub(crate) fn scope(host: &str) -> ConnectionScope {
    ConnectionScope {
        settings: ConnectionSettings {
            ssh_host: host.into(),
            ..server(SERVER, "Server").connection_settings()
        },
        connection_revision: 0,
        backend_instance: INSTANCE,
    }
}

/// A saved, set-up server reached by public key at `fixture@localhost:22`.
pub(crate) fn server(id: Uuid, name: &str) -> Server {
    Server {
        id,
        name: name.into(),
        ssh_user: "fixture".into(),
        ssh_host: "localhost".into(),
        ssh_port: 22,
        identity_file: None,
        agent_source: None,
        agent_key_fingerprint: None,
        auth_method: AuthMethod::PublicKey,
        clipboard_enabled: false,
        browser_enabled: false,
        agent_install_approved: true,
        setup_complete: true,
        proxy: None,
    }
}

/// A local forward from `local_port` to `localhost:remote_port`.
pub(crate) fn tunnel(id: Uuid, server: Uuid, local_port: u16, remote_port: u16) -> Tunnel {
    Tunnel {
        id,
        name: format!("Port {remote_port}"),
        server_id: server,
        local_port,
        local_port_end: None,
        remote_host: "localhost".into(),
        remote_port,
        remote_port_end: None,
        auto_connect: false,
        auto_reconnect: true,
        direction: Direction::Local,
        project: None,
    }
}

/// Runtime state with nothing observed yet.
pub(crate) fn runtime() -> Runtime {
    Runtime {
        agent_transport: Vec::new(),
        agent_runtime: Vec::new(),
        agent_permissions: Vec::new(),
        agent_status: Vec::new(),
        agent_installation: Vec::new(),
        tunnels: Vec::new(),
        tunnel_health: Vec::new(),
        clipboard: Vec::new(),
        clipboard_messages: Vec::new(),
        connection_revisions: Vec::new(),
        awaiting_agent: Vec::new(),
    }
}

/// A snapshot from [`INSTANCE`] with these servers, no forwards, and each
/// server at connection revision 1.
pub(crate) fn snapshot(servers: Vec<Server>) -> Snapshot {
    let mut runtime = runtime();
    for server in &servers {
        set(&mut runtime.connection_revisions, server.id, 1);
    }
    Snapshot {
        instance_id: INSTANCE,
        revision: 1,
        config: Config {
            servers,
            tunnels: Vec::new(),
        },
        runtime,
        load_error: None,
    }
}

/// The reply the backend sends for a configuration read.
pub(crate) fn reply(snapshot: Snapshot) -> desktop::Reply {
    desktop::Reply::Configuration(Box::new(Reply::Snapshot(Box::new(snapshot))))
}

/// A finished configuration read, as a worker hands it to the window.
pub(crate) fn completion(generation: u64, snapshot: Snapshot) -> crate::Completion {
    crate::Completion {
        server: String::new(),
        key: "snapshot".into(),
        generation,
        result: crate::Outcome::Typed(Ok(reply(snapshot))),
    }
}

/// A failed configuration read.
pub(crate) fn failure(generation: u64, error: &str) -> crate::Completion {
    crate::Completion {
        result: crate::Outcome::Typed(Err(error.into())),
        ..completion(generation, snapshot(Vec::new()))
    }
}

pub(crate) fn connection(status: ConnectionStatus) -> ConnectionState {
    ConnectionState {
        status,
        error_message: None,
        reconnect_attempt: 0,
    }
}

pub(crate) fn transport(status: TransportStatus) -> Transport {
    Transport {
        status,
        generation: 1,
        waiting: 0,
    }
}

/// An agent hello for build `build` that announces no services.
pub(crate) fn hello(build: &str) -> runtime::Hello {
    runtime::Hello {
        versions: vec![runtime::VERSION],
        instance_id: Uuid::new_v4().to_string(),
        build_sha256: build.into(),
        agent_version: None,
        worker_socket: "/run/workers.sock".into(),
        product: product::Variant::Core,
        services: Default::default(),
        dependencies: Default::default(),
        limits: runtime::Limits {
            max_control_bytes: runtime::MAX_FRAME,
            max_inflight: 1,
            max_workers: runtime::MAX_WORKERS,
            max_registrations: runtime::MAX_REGISTRATIONS,
        },
    }
}

pub(crate) fn agent_runtime(
    status: RuntimeStatus,
    hello: Option<runtime::Hello>,
) -> RuntimeSnapshot {
    RuntimeSnapshot {
        status,
        generation: 1,
        hello,
        error: None,
        retryable: false,
        failures: 0,
        instances: Vec::new(),
    }
}

/// An agent check that found nothing to report.
pub(crate) fn installation() -> InstallationStatus {
    InstallationStatus {
        installed: None,
        update_available: None,
        build_relation: BuildRelation::Unknown,
        installed_hash: None,
        bundled_hash: None,
        installed_version: None,
        bundled_version: None,
        installed_variant: None,
        bundled_variant: product::Variant::Core,
        installed_running: None,
        bundled_staged: false,
        checked_at: 0,
        error: None,
        automatic_update: None,
    }
}

pub(crate) fn install(stage: InstallStage, sent: u64, total: u64) -> InstallProgress {
    InstallProgress {
        stage,
        sent,
        total,
        error: None,
        failed_stage: None,
        bytes_per_second: None,
    }
}

pub(crate) fn permissions(clipboard: bool, browser: bool) -> PermissionsSnapshot {
    let permissions = runtime::Permissions {
        revision: 1,
        clipboard,
        browser,
    };
    PermissionsSnapshot {
        desired: permissions,
        effective: permissions,
        error: None,
        clipboard_epoch: 0,
        browser_epoch: 0,
        session_epoch: 0,
    }
}

/// A runtime row keyed by server or forward id.
pub(crate) trait Row {
    type Value;
    fn row(id: Uuid, value: Self::Value) -> Self;
    fn id(&self) -> Uuid;
    fn value_mut(&mut self) -> &mut Self::Value;
}
macro_rules! rows {
    ($($row:ident: $value:ty),* $(,)?) => {$(
        impl Row for $row {
            type Value = $value;
            fn row(id: Uuid, value: $value) -> Self {
                Self { id, value }
            }
            fn id(&self) -> Uuid {
                self.id
            }
            fn value_mut(&mut self) -> &mut $value {
                &mut self.value
            }
        }
    )*};
}
rows!(
    TransportRow: Transport,
    RuntimeRow: RuntimeSnapshot,
    PermissionsRow: PermissionsSnapshot,
    InstallRow: InstallProgress,
    InstallationRow: InstallationStatus,
    ConnectionRow: ConnectionState,
    HealthRow: DestinationHealthList,
    MessageRow: String,
    RevisionRow: u64,
);

/// Sets `id`'s row, replacing the one already there.
pub(crate) fn set<R: Row>(rows: &mut Vec<R>, id: Uuid, value: R::Value) {
    rows.retain(|row| row.id() != id);
    rows.push(R::row(id, value));
}

/// `id`'s row, which must be there.
pub(crate) fn row_mut<R: Row>(rows: &mut [R], id: Uuid) -> &mut R::Value {
    rows.iter_mut()
        .find(|row| row.id() == id)
        .expect("the row is set")
        .value_mut()
}

/// Removes `id`'s row.
pub(crate) fn clear<R: Row>(rows: &mut Vec<R>, id: Uuid) {
    rows.retain(|row| row.id() != id);
}

/// Records `snapshot` as a finished configuration read and shows it, as an
/// accepted reply does.
pub(crate) fn accept(view: &mut crate::Newport, snapshot: Snapshot) {
    if let Some(generation) = view.resources.begin_refresh("snapshot", false) {
        let (accepted, _) = view.complete_snapshot(generation, Ok(reply(snapshot.clone())));
        assert!(accepted);
    }
    view.snapshot = Some(snapshot);
}

impl crate::Newport {
    /// The shown snapshot, for a test to change.
    pub(crate) fn snapshot_mut(&mut self) -> &mut Snapshot {
        self.snapshot.as_mut().expect("a snapshot is shown")
    }
}

/// An agent check that found the agent installed, or found it missing.
pub(crate) fn installed(installed: bool) -> InstallationStatus {
    InstallationStatus {
        installed: Some(installed),
        ..installation()
    }
}

/// The outcome of a configuration read that delivered `snapshot` or failed.
pub(crate) fn outcome(result: Result<Snapshot, &str>) -> crate::Outcome {
    crate::Outcome::Typed(result.map(reply).map_err(Into::into))
}

/// The forward the Connections fixtures save: `Saved forward`, from local
/// port 5432 to the server's `127.0.0.1:5432`.
pub(crate) fn saved_forward(server: Uuid) -> Tunnel {
    Tunnel {
        name: "Saved forward".into(),
        remote_host: "127.0.0.1".into(),
        ..tunnel(
            Uuid::from_u128(0x00000000_0000_4000_8000_0000000000f0),
            server,
            5432,
            5432,
        )
    }
}
/// Settles the connection request `key` with `reply`, as its completion would.
pub(crate) fn settle_connection(
    view: &mut crate::Newport,
    key: &str,
    generation: u64,
    reply: desktop::connection::Reply,
) -> bool {
    view.complete_connection(
        key,
        generation,
        Ok(desktop::Reply::Connection(Box::new(reply))),
    )
}
/// Settles the configuration change `key` with its acknowledgement.
pub(crate) fn acknowledge(view: &mut crate::Newport, key: &str, generation: u64) -> bool {
    view.complete_connection(
        key,
        generation,
        Ok(desktop::Reply::Configuration(Box::new(Reply::SaveServer(
            Box::new(newport_protocol::common::Present {}),
        )))),
    )
}
/// A server inspection that found `total` bytes to install.
pub(crate) fn inspection(total: u64) -> desktop::connection::SetupInspection {
    desktop::connection::SetupInspection {
        os: String::new(),
        architecture: String::new(),
        cores: Some(8),
        memory_kib: None,
        free_kib: None,
        installed: false,
        total,
    }
}
/// An SSH config lookup that found only `proxy_command`.
pub(crate) fn resolved(proxy_command: &str) -> desktop::connection::SshResolved {
    desktop::connection::SshResolved {
        proxy_command: Some(proxy_command.into()),
        ..Default::default()
    }
}
