//! A captured destination is resolved once, then its saved identity stays pinned.
use super::{Transport, desktop_inputs, protocol};
use newport_domain::{
    forwarding::{discovered_tunnel, host, local_endpoint},
    model::{Direction, Tunnel},
};
use std::time::{Duration, Instant};
use uuid::Uuid;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_secs(1);

pub(super) fn execute(
    server: Uuid,
    address: &str,
    port: u16,
    name: &str,
    connection: Option<&protocol::ConnectionScope>,
    call: &Transport,
) -> Result<(), String> {
    let mut deadline = None;
    run(
        server,
        address,
        port,
        name,
        connection,
        call,
        &mut |pause| {
            let deadline = *deadline.get_or_insert_with(|| Instant::now() + CONNECT_TIMEOUT);
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            if pause {
                std::thread::sleep(POLL_INTERVAL.min(remaining));
            }
            Instant::now() < deadline
        },
    )
}

fn run(
    server: Uuid,
    address: &str,
    port: u16,
    name: &str,
    connection: Option<&protocol::ConnectionScope>,
    call: &Transport,
    wait: &mut dyn FnMut(bool) -> bool,
) -> Result<(), String> {
    let snapshot = read_snapshot(call)?;
    let tunnels = checked_tunnels(&snapshot, server, connection)?;
    let mut target = discovered_tunnel(&tunnels, server, address, port)?;
    local_endpoint(&target, Some(port))?;
    let existing = tunnels.iter().any(|t| t.id == target.id);
    if existing
        && snapshot
            .runtime
            .tunnel(target.id)
            .is_some_and(|state| state.status == protocol::ConnectionStatus::Connected)
    {
        return open(&target, port, connection, call);
    }
    let (create, connect) = if existing {
        (vec![], vec![target])
    } else {
        target.name = name.to_owned();
        (vec![target], vec![])
    };
    // The backend atomically resolves allocations/reuse and checks the captured
    // connection before any save/connect. Its reply pins the actual saved ID.
    let reply = call(protocol::Operation::ForwardTunnelBatch(Box::new(
        protocol::ForwardTunnelBatchArgs {
            server,
            create: create.iter().map(protocol::Tunnel::from).collect(),
            connect: connect.iter().map(protocol::Tunnel::from).collect(),
            expected: Some(
                desktop_inputs::captured(connection).map_err(|error| error.to_string())?,
            ),
        },
    )))?;
    let protocol::Reply::ForwardTunnelBatch(saved) = reply else {
        return Err("Forwarding completed without a confirmed saved destination. Refresh before trying again.".into());
    };
    let saved: Vec<Tunnel> = saved.tunnels.iter().map(Tunnel::from).collect();
    let [target] = saved.as_slice() else {
        return Err("Forwarding did not confirm exactly one saved destination.".into());
    };
    if target.id.is_nil()
        || target.server_id != server
        || target.direction != Direction::Local
        || host(&target.remote_host) != host(address)
    {
        return Err("The saved forward no longer matches the requested destination.".into());
    }
    local_endpoint(target, Some(port))?;
    loop {
        let timeout =
            "Timed out waiting for the forward to connect. Check its status before trying again.";
        if !wait(false) {
            return Err(timeout.into());
        }
        let snapshot = read_snapshot(call)?;
        if !wait(false) {
            return Err(timeout.into());
        }
        let current = checked_tunnels(&snapshot, server, connection)?;
        if current.iter().find(|t| t.id == target.id) != Some(target) {
            return Err("The forward changed or was deleted before it could be opened.".into());
        }
        match snapshot.runtime.tunnel(target.id) {
            Some(state) if state.status == protocol::ConnectionStatus::Connected => {
                return open(target, port, connection, call);
            }
            Some(state)
                if matches!(
                    state.status,
                    protocol::ConnectionStatus::Connecting
                        | protocol::ConnectionStatus::Reconnecting
                ) => {}
            Some(state) if state.status == protocol::ConnectionStatus::Error => {
                return Err(state
                    .error_message
                    .as_deref()
                    .filter(|message| !message.is_empty())
                    .unwrap_or("The forward could not connect.")
                    .into());
            }
            _ => return Err("The forward disconnected before it could be opened.".into()),
        }
        if !wait(true) {
            return Err(timeout.into());
        }
    }
}

fn read_snapshot(call: &Transport) -> Result<protocol::Snapshot, String> {
    let protocol::Reply::Snapshot(value) = call(protocol::Operation::Snapshot(Box::new(
        newport_protocol::common::Present {},
    )))?
    else {
        return Err("Invalid forwarding snapshot.".into());
    };
    Ok(*value)
}
fn checked_tunnels(
    snapshot: &protocol::Snapshot,
    server: Uuid,
    connection: Option<&protocol::ConnectionScope>,
) -> Result<Vec<Tunnel>, String> {
    if snapshot.load_error.is_some() || snapshot.instance_id.is_nil() {
        return Err("Server configuration is unavailable; the destination was not opened.".into());
    }
    let record = snapshot
        .config
        .server(server)
        .ok_or("The server no longer exists.")?;
    let current = snapshot.connection_scope(record);
    if current != desktop_inputs::captured(connection).map_err(|error| error.to_string())? {
        return Err("The server connection changed before the destination could be opened.".into());
    }
    Ok(snapshot.config.tunnels.iter().map(Tunnel::from).collect())
}

fn open(
    tunnel: &Tunnel,
    port: u16,
    connection: Option<&protocol::ConnectionScope>,
    call: &Transport,
) -> Result<(), String> {
    call(protocol::Operation::OpenTunnel(Box::new(
        protocol::OpenTunnelArgs {
            tunnel: tunnel.id,
            remote_port: Some(port),
            expected: Some(protocol::Tunnel::from(tunnel)),
            expected_connection: Some(
                desktop_inputs::captured(connection).map_err(|error| error.to_string())?,
            ),
        },
    )))
    .map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tunnel_operations::{Action, Kind, Operations, StartError};
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex, mpsc},
    };

    type Calls = Arc<Mutex<Vec<protocol::Operation>>>;
    type Reply = Result<protocol::Reply, String>;
    /// The forwarding operation a fixture step expects.
    fn name(operation: &protocol::Operation) -> &'static str {
        match operation {
            protocol::Operation::Snapshot(_) => "snapshot",
            protocol::Operation::ForwardTunnelBatch(_) => "forward_tunnel_batch",
            protocol::Operation::OpenTunnel(_) => "open_tunnel",
            _ => panic!("unexpected forwarding operation {operation:?}"),
        }
    }
    fn transport(
        steps: Vec<(&str, Reply)>,
    ) -> (
        Calls,
        impl Fn(protocol::Operation) -> Reply + Send + Sync + 'static,
    ) {
        let steps: Mutex<VecDeque<_>> = Mutex::new(
            steps
                .into_iter()
                .map(|(method, result)| (method.to_owned(), result))
                .collect(),
        );
        let calls = Arc::new(Mutex::new(Vec::new()));
        let observed = calls.clone();
        (calls, move |operation| {
            let method = name(&operation);
            observed.lock().unwrap().push(operation);
            let (expected, result) = steps
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected/replayed call");
            assert_eq!(method, expected);
            result
        })
    }
    fn tunnel(server: Uuid) -> Tunnel {
        let mut t = discovered_tunnel(&[], server, "127.0.0.1", 3000).unwrap();
        t.local_port = 9000;
        t.local_port_end = Some(9005);
        t.remote_port_end = Some(3005);
        t
    }
    /// A snapshot holding one server and `tunnels`, each in `status`, or with
    /// no runtime state when `status` is `None`.
    fn snapshot(
        server: Uuid,
        tunnels: &[Tunnel],
        status: Option<protocol::ConnectionStatus>,
    ) -> protocol::Snapshot {
        protocol::Snapshot {
            instance_id: Uuid::parse_str("22222222-2222-4222-8222-222222222222").unwrap(),
            revision: 0,
            load_error: None,
            config: protocol::Config {
                servers: vec![protocol::Server {
                    id: server,
                    name: "Fixture".into(),
                    ssh_user: "fixture".into(),
                    ssh_host: "unconnected.invalid".into(),
                    ssh_port: 22,
                    identity_file: None,
                    agent_source: None,
                    agent_key_fingerprint: None,
                    auth_method: protocol::AuthMethod::PublicKey,
                    clipboard_enabled: false,
                    browser_enabled: false,
                    agent_install_approved: false,
                    setup_complete: false,
                    proxy: None,
                }],
                tunnels: tunnels.iter().map(protocol::Tunnel::from).collect(),
            },
            runtime: protocol::Runtime {
                agent_transport: vec![],
                agent_runtime: vec![],
                agent_permissions: vec![],
                agent_status: vec![],
                agent_installation: vec![],
                tunnels: status
                    .map(|status| {
                        tunnels
                            .iter()
                            .map(|tunnel| protocol::ConnectionRow {
                                id: tunnel.id,
                                value: protocol::ConnectionState {
                                    status,
                                    error_message: Some("fixture connection error".into()),
                                    reconnect_attempt: 0,
                                },
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                tunnel_health: vec![],
                clipboard: vec![],
                clipboard_messages: vec![],
                connection_revisions: vec![protocol::RevisionRow {
                    id: server,
                    value: 1,
                }],
                awaiting_agent: vec![],
            },
        }
    }
    fn reply(snapshot: &protocol::Snapshot) -> Reply {
        Ok(protocol::Reply::Snapshot(Box::new(snapshot.clone())))
    }
    fn resolved(tunnel: &Tunnel) -> Reply {
        Ok(protocol::Reply::ForwardTunnelBatch(Box::new(
            protocol::Tunnels {
                tunnels: vec![protocol::Tunnel::from(tunnel)],
            },
        )))
    }
    fn opened() -> Reply {
        Ok(protocol::Reply::OpenTunnel(Box::new(
            newport_protocol::common::Present {},
        )))
    }
    fn connection(snapshot: &protocol::Snapshot) -> Option<protocol::ConnectionScope> {
        Some(snapshot.connection_scope(&snapshot.config.servers[0]))
    }
    fn names(calls: &Calls) -> Vec<&'static str> {
        calls.lock().unwrap().iter().map(name).collect()
    }
    fn batch(operation: &protocol::Operation) -> &protocol::ForwardTunnelBatchArgs {
        match operation {
            protocol::Operation::ForwardTunnelBatch(args) => args,
            _ => panic!("expected a batch: {operation:?}"),
        }
    }
    fn open_args(operation: &protocol::Operation) -> &protocol::OpenTunnelArgs {
        match operation {
            protocol::Operation::OpenTunnel(args) => args,
            _ => panic!("expected an open: {operation:?}"),
        }
    }
    use protocol::ConnectionStatus::{Connected, Connecting, Disconnected, Error};

    #[test]
    fn connected_range_opens_exact_port_once_without_a_batch() {
        let server = Uuid::new_v4();
        let t = tunnel(server);
        let initial = snapshot(server, std::slice::from_ref(&t), Some(Connected));
        let scope = connection(&initial);
        let (calls, call) = transport(vec![
            ("snapshot", reply(&initial)),
            ("open_tunnel", opened()),
        ]);
        run(
            server,
            "0.0.0.0",
            3003,
            "web:3003",
            scope.as_ref(),
            &call,
            &mut |_| panic!("already connected"),
        )
        .unwrap();
        assert_eq!(names(&calls), ["snapshot", "open_tunnel"]);
        let calls = calls.lock().unwrap();
        let open = open_args(&calls[1]);
        assert_eq!(open.tunnel, t.id);
        assert_eq!(open.remote_port, Some(3003));
        assert_eq!(open.expected, Some(protocol::Tunnel::from(&t)));
        assert_eq!(open.expected_connection, scope);
    }

    #[test]
    fn new_named_forward_pins_authoritative_reused_id_then_waits_before_opening() {
        let server = Uuid::new_v4();
        let saved = tunnel(server); // A concurrently saved range wins backend resolution.
        let initial = snapshot(server, &[], Some(Disconnected));
        let scope = connection(&initial);
        let (calls, call) = transport(vec![
            ("snapshot", reply(&initial)),
            ("forward_tunnel_batch", resolved(&saved)),
            (
                "snapshot",
                reply(&snapshot(
                    server,
                    std::slice::from_ref(&saved),
                    Some(Connecting),
                )),
            ),
            (
                "snapshot",
                reply(&snapshot(
                    server,
                    std::slice::from_ref(&saved),
                    Some(Connected),
                )),
            ),
            ("open_tunnel", opened()),
        ]);
        let mut pauses = 0;
        run(
            server,
            "localhost",
            3003,
            "web:3003",
            scope.as_ref(),
            &call,
            &mut |pause| {
                pauses += usize::from(pause);
                true
            },
        )
        .unwrap();
        assert_eq!(pauses, 1);
        let calls = calls.lock().unwrap();
        let batch = batch(&calls[1]);
        assert_eq!(batch.expected, scope);
        assert!(batch.connect.is_empty());
        let [created] = batch.create.as_slice() else {
            panic!("expected one creation: {:?}", batch.create);
        };
        assert_eq!(created.name, "web:3003");
        assert_eq!(created.remote_host, "localhost");
        assert_eq!(created.remote_port, 3003);
        assert_ne!(created.id, saved.id);
        let open = open_args(&calls[4]);
        assert_eq!(open.expected, Some(protocol::Tunnel::from(&saved)));
        assert_eq!(open.remote_port, Some(3003));
    }

    #[test]
    fn disconnected_saved_forward_connects_once_and_reuses_its_definition() {
        let server = Uuid::new_v4();
        let saved = tunnel(server);
        let initial = snapshot(server, std::slice::from_ref(&saved), Some(Disconnected));
        let scope = connection(&initial);
        let (calls, call) = transport(vec![
            ("snapshot", reply(&initial)),
            ("forward_tunnel_batch", resolved(&saved)),
            (
                "snapshot",
                reply(&snapshot(
                    server,
                    std::slice::from_ref(&saved),
                    Some(Connected),
                )),
            ),
            ("open_tunnel", opened()),
        ]);
        run(
            server,
            "127.0.0.1",
            3003,
            "ignored rename",
            scope.as_ref(),
            &call,
            &mut |_| true,
        )
        .unwrap();
        let calls = calls.lock().unwrap();
        let batch = batch(&calls[1]);
        assert_eq!(batch.server, server);
        assert!(batch.create.is_empty());
        assert_eq!(batch.connect, [protocol::Tunnel::from(&saved)]);
        assert_eq!(batch.expected, scope);
    }

    #[test]
    fn scope_changes_configuration_outage_and_target_changes_stop_before_open() {
        let server = Uuid::new_v4();
        let saved = tunnel(server);
        let initial = snapshot(server, std::slice::from_ref(&saved), Some(Disconnected));
        let scope = connection(&initial);
        for change in 0..9 {
            let mut changed = snapshot(server, std::slice::from_ref(&saved), Some(Connected));
            match change {
                0 => changed.config.tunnels.clear(),
                1 => changed.config.tunnels[0].id = Uuid::new_v4(),
                2 => changed.config.tunnels[0].remote_host = "10.0.0.1".into(),
                3 => changed.config.tunnels[0].local_port = 9001,
                4 => changed.config.servers[0].ssh_host = "changed.invalid".into(),
                5 => changed.runtime.connection_revisions[0].value = 2,
                6 => changed.instance_id = Uuid::new_v4(),
                7 => changed.config.servers.clear(),
                _ => changed.load_error = Some("profile unavailable".into()),
            }
            let (calls, call) = transport(vec![
                ("snapshot", reply(&initial)),
                ("forward_tunnel_batch", resolved(&saved)),
                ("snapshot", reply(&changed)),
            ]);
            assert!(
                run(
                    server,
                    "127.0.0.1",
                    3003,
                    "web",
                    scope.as_ref(),
                    &call,
                    &mut |_| { true }
                )
                .is_err(),
                "change {change}"
            );
            assert_eq!(
                names(&calls),
                ["snapshot", "forward_tunnel_batch", "snapshot"]
            );
            if change >= 4 {
                let (calls, call) = transport(vec![("snapshot", reply(&changed))]);
                assert!(
                    run(
                        server,
                        "127.0.0.1",
                        3003,
                        "web",
                        scope.as_ref(),
                        &call,
                        &mut |_| { true }
                    )
                    .is_err()
                );
                assert_eq!(names(&calls), ["snapshot"]);
            }
        }
    }

    #[test]
    fn runtime_failure_timeout_and_late_connected_reply_never_open_or_reconnect_again() {
        let server = Uuid::new_v4();
        let saved = tunnel(server);
        let initial = snapshot(server, std::slice::from_ref(&saved), Some(Disconnected));
        let scope = connection(&initial);
        for status in [Some(Error), Some(Disconnected), None] {
            let (calls, call) = transport(vec![
                ("snapshot", reply(&initial)),
                ("forward_tunnel_batch", resolved(&saved)),
                (
                    "snapshot",
                    reply(&snapshot(server, std::slice::from_ref(&saved), status)),
                ),
            ]);
            assert!(
                run(
                    server,
                    "127.0.0.1",
                    3003,
                    "web",
                    scope.as_ref(),
                    &call,
                    &mut |_| { true }
                )
                .is_err()
            );
            assert_eq!(
                names(&calls),
                ["snapshot", "forward_tunnel_batch", "snapshot"]
            );
        }
        let mut steps = vec![
            ("snapshot", reply(&initial)),
            ("forward_tunnel_batch", resolved(&saved)),
        ];
        steps.extend((0..3).map(|_| {
            (
                "snapshot",
                reply(&snapshot(
                    server,
                    std::slice::from_ref(&saved),
                    Some(Connecting),
                )),
            )
        }));
        let (calls, call) = transport(steps);
        let mut pauses = 0;
        assert!(
            run(
                server,
                "127.0.0.1",
                3003,
                "web",
                scope.as_ref(),
                &call,
                &mut |pause| {
                    pauses += usize::from(pause);
                    pauses < 3
                }
            )
            .unwrap_err()
            .contains("Timed out")
        );
        assert_eq!(calls.lock().unwrap().len(), 5);
        assert_eq!(POLL_INTERVAL, Duration::from_secs(1));
        assert_eq!(CONNECT_TIMEOUT, Duration::from_secs(30));
        let (calls, call) = transport(vec![
            ("snapshot", reply(&initial)),
            ("forward_tunnel_batch", resolved(&saved)),
            (
                "snapshot",
                reply(&snapshot(
                    server,
                    std::slice::from_ref(&saved),
                    Some(Connected),
                )),
            ),
        ]);
        let mut budget_checks = 0;
        assert!(
            run(
                server,
                "127.0.0.1",
                3003,
                "web",
                scope.as_ref(),
                &call,
                &mut |_| {
                    budget_checks += 1;
                    budget_checks == 1
                }
            )
            .unwrap_err()
            .contains("Timed out")
        );
        assert_eq!(
            names(&calls),
            ["snapshot", "forward_tunnel_batch", "snapshot"]
        );
    }

    #[test]
    fn uncertain_rpc_failures_are_returned_without_replay_or_fallback_writes() {
        let server = Uuid::new_v4();
        let saved = tunnel(server);
        let initial = snapshot(server, std::slice::from_ref(&saved), Some(Disconnected));
        let scope = connection(&initial);
        let steps = [
            ("snapshot", reply(&initial)),
            ("forward_tunnel_batch", resolved(&saved)),
            (
                "snapshot",
                reply(&snapshot(
                    server,
                    std::slice::from_ref(&saved),
                    Some(Connected),
                )),
            ),
            ("open_tunnel", opened()),
        ];
        for failed in 0..steps.len() {
            let mut attempt = steps[..=failed].to_vec();
            attempt[failed].1 = Err("uncertain result".into());
            let (calls, call) = transport(attempt);
            assert_eq!(
                run(
                    server,
                    "127.0.0.1",
                    3003,
                    "web",
                    scope.as_ref(),
                    &call,
                    &mut |_| { true }
                ),
                Err("uncertain result".into())
            );
            assert_eq!(calls.lock().unwrap().len(), failed + 1);
        }
    }

    #[test]
    fn operation_reserves_destination_through_browser_dispatch_and_outcome_collection() {
        let server = Uuid::new_v4();
        let saved = tunnel(server);
        let initial = snapshot(server, std::slice::from_ref(&saved), Some(Connected));
        let scope = connection(&initial);
        let (opening_started, opening) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        let blocked = Mutex::new(blocked);
        let mut owner = Operations::with_transport(move |operation| match name(&operation) {
            "snapshot" => reply(&initial),
            "open_tunnel" => {
                opening_started.send(()).unwrap();
                blocked.lock().unwrap().recv().unwrap();
                opened()
            }
            method => panic!("unexpected write {method}"),
        });
        let ticket = owner
            .start(Action::ForwardAndOpen {
                server,
                address: "0.0.0.0".into(),
                port: 3003,
                name: "web".into(),
                connection: scope.clone(),
            })
            .unwrap();
        opening.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(owner.busy_destination(server, "localhost", 3003));
        assert!(owner.busy_tunnel(&saved));
        assert!(matches!(
            owner.start(Action::ForwardAndOpen {
                server,
                address: "127.0.0.1".into(),
                port: 3003,
                name: "web".into(),
                connection: scope
            }),
            Err(StartError::Busy)
        ));
        assert!(matches!(
            owner.start(Action::Delete(saved.clone())),
            Err(StartError::Busy)
        ));
        assert!(owner.poll().is_empty());
        release.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while ticket.result().is_none() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        drop(ticket); // Navigation/editor lifetime has no effect on ownership.
        assert!(owner.busy_tunnel(&saved));
        let outcomes = owner.poll();
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].kind, Kind::ForwardAndOpen);
        assert_eq!(outcomes[0].result, Ok(()));
        assert!(!owner.busy_tunnel(&saved));
        assert!(owner.poll().is_empty());
    }
}
