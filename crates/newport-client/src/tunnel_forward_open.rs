//! A captured destination is resolved once, then its saved identity stays pinned.
use super::Transport;
use newport_domain::{
    connection_scope,
    forwarding::{discovered_tunnel, host, local_endpoint},
    model::{Direction, Tunnel},
};
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use uuid::Uuid;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_secs(1);

pub(super) fn execute(
    server: Uuid,
    address: &str,
    port: u16,
    name: &str,
    connection: &Value,
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
    connection: &Value,
    call: &Transport,
    wait: &mut dyn FnMut(bool) -> bool,
) -> Result<(), String> {
    let snapshot = call("snapshot", json!({}))?;
    let tunnels = checked_tunnels(&snapshot, server, connection)?;
    let mut target = discovered_tunnel(&tunnels, server, address, port)?;
    local_endpoint(&target, Some(port))?;
    let existing = tunnels.iter().any(|t| t.id == target.id);
    if existing && status(&snapshot, target.id) == Some("connected") {
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
    let saved: Vec<Tunnel> = serde_json::from_value(call(
        "forward_tunnel_batch",
        json!({"serverId":server,"create":create,"connect":connect,"expectedConnection":connection}),
    )?)
    .map_err(|_| "Forwarding completed without a confirmed saved destination. Refresh before trying again.")?;
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
        let snapshot = call("snapshot", json!({}))?;
        if !wait(false) {
            return Err(timeout.into());
        }
        let current = checked_tunnels(&snapshot, server, connection)?;
        if current.iter().find(|t| t.id == target.id) != Some(target) {
            return Err("The forward changed or was deleted before it could be opened.".into());
        }
        match status(&snapshot, target.id) {
            Some("connected") => return open(target, port, connection, call),
            Some("connecting" | "reconnecting") => {}
            Some("error") => {
                return Err(
                    snapshot["runtime"]["tunnels"][target.id.to_string()]["errorMessage"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .unwrap_or("The forward could not connect.")
                        .to_owned(),
                );
            }
            _ => return Err("The forward disconnected before it could be opened.".into()),
        }
        if !wait(true) {
            return Err(timeout.into());
        }
    }
}

fn checked_tunnels(
    snapshot: &Value,
    server: Uuid,
    connection: &Value,
) -> Result<Vec<Tunnel>, String> {
    if !snapshot["loadError"].is_null() || snapshot["instanceId"].as_str().is_none_or(str::is_empty)
    {
        return Err("Server configuration is unavailable; the destination was not opened.".into());
    }
    let server = snapshot["config"]["servers"]
        .as_array()
        .and_then(|servers| servers.iter().find(|row| row["id"] == server.to_string()))
        .ok_or("The server no longer exists.")?;
    if connection_scope::connection(server, snapshot) != *connection {
        return Err("The server connection changed before the destination could be opened.".into());
    }
    serde_json::from_value(snapshot["config"]["tunnels"].clone())
        .map_err(|_| "The saved forwards could not be read.".into())
}

fn status(snapshot: &Value, id: Uuid) -> Option<&str> {
    snapshot["runtime"]["tunnels"][id.to_string()]["status"].as_str()
}

fn open(tunnel: &Tunnel, port: u16, connection: &Value, call: &Transport) -> Result<(), String> {
    call(
        "open_tunnel",
        json!({"id":tunnel.id,"remotePort":port,"expected":tunnel,"expectedConnection":connection}),
    )
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tunnel_operations::{Action, Kind, Operations, StartError};
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex, mpsc},
    };

    type Calls = Arc<Mutex<Vec<(String, Value)>>>;
    fn transport(
        steps: Vec<(&str, Result<Value, String>)>,
    ) -> (
        Calls,
        impl Fn(&str, Value) -> Result<Value, String> + Send + Sync + 'static,
    ) {
        let steps: Mutex<VecDeque<_>> = Mutex::new(
            steps
                .into_iter()
                .map(|(method, result)| (method.to_owned(), result))
                .collect(),
        );
        let calls = Arc::new(Mutex::new(Vec::new()));
        let observed = calls.clone();
        (calls, move |method, args| {
            observed.lock().unwrap().push((method.to_owned(), args));
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
    fn snapshot(server: Uuid, tunnels: &[Tunnel], status: &str) -> Value {
        let mut runtime = serde_json::Map::new();
        for tunnel in tunnels {
            runtime.insert(
                tunnel.id.to_string(),
                json!({"status":status,"errorMessage":"fixture connection error"}),
            );
        }
        json!({"instanceId":"fixture-backend","config":{"servers":[{
            "id":server,"name":"Fixture","sshHost":"unconnected.invalid","sshUser":"fixture","sshPort":22,"authMethod":"publicKey"
        }],"tunnels":tunnels},"runtime":{"tunnels":runtime,"connectionRevisions":{(server.to_string()):1}},"loadError":null})
    }
    fn connection(snapshot: &Value) -> Value {
        connection_scope::connection(&snapshot["config"]["servers"][0], snapshot)
    }
    fn names(calls: &Calls) -> Vec<String> {
        calls
            .lock()
            .unwrap()
            .iter()
            .map(|(method, _)| method.clone())
            .collect()
    }

    #[test]
    fn connected_range_opens_exact_port_once_without_a_batch() {
        let server = Uuid::new_v4();
        let t = tunnel(server);
        let initial = snapshot(server, std::slice::from_ref(&t), "connected");
        let scope = connection(&initial);
        let (calls, call) = transport(vec![
            ("snapshot", Ok(initial)),
            ("open_tunnel", Ok(Value::Null)),
        ]);
        run(
            server,
            "0.0.0.0",
            3003,
            "web:3003",
            &scope,
            &call,
            &mut |_| panic!("already connected"),
        )
        .unwrap();
        assert_eq!(names(&calls), ["snapshot", "open_tunnel"]);
        assert_eq!(
            calls.lock().unwrap()[1].1,
            json!({"id":t.id,"remotePort":3003,"expected":t,"expectedConnection":scope})
        );
    }

    #[test]
    fn new_named_forward_pins_authoritative_reused_id_then_waits_before_opening() {
        let server = Uuid::new_v4();
        let saved = tunnel(server); // A concurrently saved range wins backend resolution.
        let initial = snapshot(server, &[], "disconnected");
        let scope = connection(&initial);
        let (calls, call) = transport(vec![
            ("snapshot", Ok(initial)),
            ("forward_tunnel_batch", Ok(json!([saved]))),
            (
                "snapshot",
                Ok(snapshot(server, std::slice::from_ref(&saved), "connecting")),
            ),
            (
                "snapshot",
                Ok(snapshot(server, std::slice::from_ref(&saved), "connected")),
            ),
            ("open_tunnel", Ok(Value::Null)),
        ]);
        let mut pauses = 0;
        run(
            server,
            "localhost",
            3003,
            "web:3003",
            &scope,
            &call,
            &mut |pause| {
                pauses += usize::from(pause);
                true
            },
        )
        .unwrap();
        assert_eq!(pauses, 1);
        let calls = calls.lock().unwrap();
        let batch = &calls[1].1;
        assert_eq!(batch["expectedConnection"], scope);
        assert_eq!(batch["connect"], json!([]));
        assert_eq!(batch["create"].as_array().unwrap().len(), 1);
        assert_eq!(batch["create"][0]["name"], "web:3003");
        assert_eq!(batch["create"][0]["remoteHost"], "127.0.0.1");
        assert_eq!(batch["create"][0]["remotePort"], 3003);
        assert_ne!(batch["create"][0]["id"], saved.id.to_string());
        assert_eq!(calls[4].1["expected"], json!(saved));
        assert_eq!(calls[4].1["remotePort"], 3003);
    }

    #[test]
    fn disconnected_saved_forward_connects_once_and_reuses_its_definition() {
        let server = Uuid::new_v4();
        let saved = tunnel(server);
        let initial = snapshot(server, std::slice::from_ref(&saved), "disconnected");
        let scope = connection(&initial);
        let (calls, call) = transport(vec![
            ("snapshot", Ok(initial)),
            ("forward_tunnel_batch", Ok(json!([saved]))),
            (
                "snapshot",
                Ok(snapshot(server, std::slice::from_ref(&saved), "connected")),
            ),
            ("open_tunnel", Ok(Value::Null)),
        ]);
        run(
            server,
            "127.0.0.1",
            3003,
            "ignored rename",
            &scope,
            &call,
            &mut |_| true,
        )
        .unwrap();
        assert_eq!(
            calls.lock().unwrap()[1].1,
            json!({"serverId":server,"create":[],"connect":[saved],"expectedConnection":scope})
        );
    }

    #[test]
    fn scope_changes_configuration_outage_and_target_changes_stop_before_open() {
        let server = Uuid::new_v4();
        let saved = tunnel(server);
        let initial = snapshot(server, std::slice::from_ref(&saved), "disconnected");
        let scope = connection(&initial);
        for change in 0..9 {
            let mut changed = snapshot(server, std::slice::from_ref(&saved), "connected");
            match change {
                0 => changed["config"]["tunnels"] = json!([]),
                1 => changed["config"]["tunnels"][0]["id"] = json!(Uuid::new_v4()),
                2 => changed["config"]["tunnels"][0]["remoteHost"] = json!("10.0.0.1"),
                3 => changed["config"]["tunnels"][0]["localPort"] = json!(9001),
                4 => changed["config"]["servers"][0]["sshHost"] = json!("changed.invalid"),
                5 => changed["runtime"]["connectionRevisions"][server.to_string()] = json!(2),
                6 => changed["instanceId"] = json!("replaced-backend"),
                7 => changed["config"]["servers"] = json!([]),
                _ => changed["loadError"] = json!("profile unavailable"),
            }
            let (calls, call) = transport(vec![
                ("snapshot", Ok(initial.clone())),
                ("forward_tunnel_batch", Ok(json!([saved]))),
                ("snapshot", Ok(changed.clone())),
            ]);
            assert!(
                run(server, "127.0.0.1", 3003, "web", &scope, &call, &mut |_| {
                    true
                })
                .is_err(),
                "change {change}"
            );
            assert_eq!(
                names(&calls),
                ["snapshot", "forward_tunnel_batch", "snapshot"]
            );
            if change >= 4 {
                let (calls, call) = transport(vec![("snapshot", Ok(changed))]);
                assert!(
                    run(server, "127.0.0.1", 3003, "web", &scope, &call, &mut |_| {
                        true
                    })
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
        let initial = snapshot(server, std::slice::from_ref(&saved), "disconnected");
        let scope = connection(&initial);
        for status in ["error", "disconnected", "unknown"] {
            let (calls, call) = transport(vec![
                ("snapshot", Ok(initial.clone())),
                ("forward_tunnel_batch", Ok(json!([saved]))),
                (
                    "snapshot",
                    Ok(snapshot(server, std::slice::from_ref(&saved), status)),
                ),
            ]);
            assert!(
                run(server, "127.0.0.1", 3003, "web", &scope, &call, &mut |_| {
                    true
                })
                .is_err()
            );
            assert_eq!(
                names(&calls),
                ["snapshot", "forward_tunnel_batch", "snapshot"]
            );
        }
        let mut steps = vec![
            ("snapshot", Ok(initial.clone())),
            ("forward_tunnel_batch", Ok(json!([saved]))),
        ];
        steps.extend((0..3).map(|_| {
            (
                "snapshot",
                Ok(snapshot(server, std::slice::from_ref(&saved), "connecting")),
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
                &scope,
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
            ("snapshot", Ok(initial)),
            ("forward_tunnel_batch", Ok(json!([saved]))),
            (
                "snapshot",
                Ok(snapshot(server, std::slice::from_ref(&saved), "connected")),
            ),
        ]);
        let mut budget_checks = 0;
        assert!(
            run(server, "127.0.0.1", 3003, "web", &scope, &call, &mut |_| {
                budget_checks += 1;
                budget_checks == 1
            })
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
        let initial = snapshot(server, std::slice::from_ref(&saved), "disconnected");
        let scope = connection(&initial);
        let steps = [
            ("snapshot", Ok(initial)),
            ("forward_tunnel_batch", Ok(json!([saved]))),
            (
                "snapshot",
                Ok(snapshot(server, std::slice::from_ref(&saved), "connected")),
            ),
            ("open_tunnel", Ok(Value::Null)),
        ];
        for failed in 0..steps.len() {
            let mut attempt = steps[..=failed].to_vec();
            attempt[failed].1 = Err("uncertain result".into());
            let (calls, call) = transport(attempt);
            assert_eq!(
                run(server, "127.0.0.1", 3003, "web", &scope, &call, &mut |_| {
                    true
                }),
                Err("uncertain result".into())
            );
            assert_eq!(calls.lock().unwrap().len(), failed + 1);
        }
    }

    #[test]
    fn operation_reserves_destination_through_browser_dispatch_and_outcome_collection() {
        let server = Uuid::new_v4();
        let saved = tunnel(server);
        let initial = snapshot(server, std::slice::from_ref(&saved), "connected");
        let scope = connection(&initial);
        let (opened, opening) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        let blocked = Mutex::new(blocked);
        let mut owner = Operations::with_transport(move |method, _| match method {
            "snapshot" => Ok(initial.clone()),
            "open_tunnel" => {
                opened.send(()).unwrap();
                blocked.lock().unwrap().recv().unwrap();
                Ok(Value::Null)
            }
            _ => panic!("unexpected write {method}"),
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
