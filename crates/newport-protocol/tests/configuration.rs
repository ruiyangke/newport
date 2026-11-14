//! Lookups on the configuration snapshot find each server's row by its id.
use newport_protocol::desktop::configuration::*;
use uuid::Uuid;

fn server(id: Uuid) -> Server {
    Server {
        id,
        name: "Box".into(),
        ssh_user: "user".into(),
        ssh_host: "host".into(),
        ssh_port: 22,
        identity_file: None,
        agent_source: None,
        agent_key_fingerprint: None,
        auth_method: AuthMethod::PublicKey,
        clipboard_enabled: false,
        browser_enabled: false,
        agent_install_approved: true,
        setup_complete: true,
        proxy: Some(Proxy::Command(Box::new("nc %h %p".into()))),
    }
}

fn connection(status: ConnectionStatus) -> ConnectionState {
    ConnectionState {
        status,
        error_message: None,
        reconnect_attempt: 0,
    }
}

#[test]
fn each_lookup_finds_the_row_of_its_own_id() {
    let [selected, other, forward] = [1, 2, 3].map(Uuid::from_u128);
    let snapshot = Snapshot {
        instance_id: Uuid::from_u128(4),
        revision: 1,
        config: Config {
            servers: vec![server(selected), server(other)],
            tunnels: Vec::new(),
        },
        runtime: Runtime {
            agent_transport: Vec::new(),
            agent_runtime: Vec::new(),
            agent_permissions: Vec::new(),
            agent_status: Vec::new(),
            agent_installation: Vec::new(),
            tunnels: vec![ConnectionRow {
                id: forward,
                value: connection(ConnectionStatus::Connected),
            }],
            tunnel_health: Vec::new(),
            clipboard: vec![
                ConnectionRow {
                    id: other,
                    value: connection(ConnectionStatus::Error),
                },
                ConnectionRow {
                    id: selected,
                    value: connection(ConnectionStatus::Connecting),
                },
            ],
            clipboard_messages: Vec::new(),
            connection_revisions: vec![RevisionRow {
                id: selected,
                value: 4,
            }],
            awaiting_agent: vec![other],
        },
        load_error: None,
    };
    let runtime = &snapshot.runtime;
    assert_eq!(
        snapshot.config.server(other).map(|server| server.id),
        Some(other)
    );
    assert!(snapshot.config.server(forward).is_none());
    assert_eq!(
        runtime.clipboard(selected).map(|state| state.status),
        Some(ConnectionStatus::Connecting)
    );
    assert_eq!(
        runtime.tunnel(forward).map(|state| state.status),
        Some(ConnectionStatus::Connected)
    );
    assert!(runtime.tunnel_health(forward).is_empty());
    assert!(runtime.agent_runtime(selected).is_none());
    assert!(runtime.awaiting_agent(other) && !runtime.awaiting_agent(selected));
    assert_eq!(runtime.connection_revision(selected), 4);
    assert_eq!(
        runtime.connection_revision(other),
        0,
        "a server not connected yet is at revision zero"
    );

    let scope = snapshot.connection_scope(&server(selected));
    assert_eq!(scope.settings, server(selected).connection_settings());
    assert_eq!(scope.connection_revision, 4);
    assert_eq!(scope.backend_instance, snapshot.instance_id);
    scope.validate().unwrap();
}
