//! Forwarding operations outlive page reads and never replay a submitted call.
//!
//! Keep one owner for the desktop session. Tickets may be held by transient
//! editors; dropping an editor does not cancel its write or release its targets.
use crate::backend::Backend;
use newport_domain::{
    forwarding::{host, local_endpoint},
    model::{Direction, Tunnel},
};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    thread,
};
use uuid::Uuid;

#[path = "tunnel_forward_open.rs"]
mod forward_open;

const MAX_PENDING: usize = 16;
const MAX_BATCH: usize = 256;
type Transport = dyn Fn(&str, Value) -> Result<Value, String> + Send + Sync;

#[derive(Clone, Debug)]
pub enum Action {
    Save(Tunnel),
    Update {
        tunnel: Tunnel,
        expected: Tunnel,
    },
    SetConnected {
        server: Uuid,
        tunnels: Vec<Tunnel>,
        connected: bool,
    },
    /// Resolve and save the batch once on the server before connecting forwards.
    ForwardBatch {
        server: Uuid,
        create: Vec<Tunnel>,
        connect: Vec<Tunnel>,
    },
    Forward {
        server: Uuid,
        address: String,
        port: u16,
    },
    /// Connect one captured destination and open it once, within its original scope.
    ForwardAndOpen {
        server: Uuid,
        address: String,
        port: u16,
        name: String,
        connection: Value,
    },
    Delete(Tunnel),
    /// Delete one server's forwards together; any changed since being shown keeps all.
    DeleteMany {
        server: Uuid,
        tunnels: Vec<Tunnel>,
    },
    Open(Tunnel),
    /// Open the exact discovered destination within a saved port range.
    OpenPort {
        tunnel: Tunnel,
        remote_port: u16,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Save,
    SetConnected,
    ForwardBatch,
    Forward,
    ForwardAndOpen,
    Delete,
    Open,
}
impl Action {
    pub fn server(&self) -> Uuid {
        match self {
            Self::Save(t) | Self::Delete(t) | Self::Open(t) | Self::OpenPort { tunnel: t, .. } => {
                t.server_id
            }
            Self::Update { tunnel, .. } => tunnel.server_id,
            Self::SetConnected { server, .. }
            | Self::DeleteMany { server, .. }
            | Self::ForwardBatch { server, .. }
            | Self::Forward { server, .. }
            | Self::ForwardAndOpen { server, .. } => *server,
        }
    }
    pub fn kind(&self) -> Kind {
        match self {
            Self::Save(_) | Self::Update { .. } => Kind::Save,
            Self::SetConnected { .. } => Kind::SetConnected,
            Self::ForwardBatch { .. } => Kind::ForwardBatch,
            Self::Forward { .. } => Kind::Forward,
            Self::ForwardAndOpen { .. } => Kind::ForwardAndOpen,
            Self::Delete(_) | Self::DeleteMany { .. } => Kind::Delete,
            Self::Open(_) | Self::OpenPort { .. } => Kind::Open,
        }
    }
    fn targets(&self) -> Result<Vec<Target>, String> {
        if self.server().is_nil() {
            return Err("A server is required".into());
        }
        if let Self::Update { tunnel, expected } = self {
            // Validate identity locally too; only the backend can verify freshness.
            newport_domain::forwarding::validate_update(
                std::slice::from_ref(expected),
                expected,
                tunnel,
            )?;
        }
        let tunnels: &[Tunnel] = match self {
            Self::Save(t) | Self::Delete(t) | Self::Open(t) | Self::OpenPort { tunnel: t, .. } => {
                std::slice::from_ref(t)
            }
            Self::Update { tunnel, .. } => std::slice::from_ref(tunnel),
            Self::SetConnected { tunnels, .. } | Self::DeleteMany { tunnels, .. } => tunnels,
            Self::ForwardBatch {
                server,
                create,
                connect,
            } => return Self::forward_batch_targets(*server, create, connect),
            Self::Forward {
                server,
                address,
                port,
            }
            | Self::ForwardAndOpen {
                server,
                address,
                port,
                ..
            } => {
                if *port == 0 || address.trim().is_empty() {
                    return Err("A valid destination is required".into());
                }
                return Ok(vec![Target {
                    id: None,
                    server: *server,
                    direction: Direction::Local,
                    host: host(address),
                    first: *port,
                    last: *port,
                }]);
            }
        };
        if tunnels.is_empty() || tunnels.len() > MAX_BATCH {
            return Err("Choose between 1 and 256 forwards".into());
        }
        let mut targets = Vec::with_capacity(tunnels.len());
        for t in tunnels {
            if t.server_id != self.server() || t.id.is_nil() {
                return Err("All forwards must belong to the selected server".into());
            }
            // An old invalid configuration must still be deletable/stoppable.
            if matches!(self, Self::Save(_) | Self::Update { .. }) {
                t.pairs()?;
            }
            match self {
                Self::Open(_) => {
                    local_endpoint(t, None)?;
                }
                Self::OpenPort { remote_port, .. } => {
                    local_endpoint(t, Some(*remote_port))?;
                }
                _ => {}
            }
            targets.push(Target::tunnel(t));
        }
        Ok(targets)
    }
    fn forward_batch_targets(
        server: Uuid,
        create: &[Tunnel],
        connect: &[Tunnel],
    ) -> Result<Vec<Target>, String> {
        let mut seen: HashMap<Uuid, (&Tunnel, bool)> = HashMap::new();
        let mut targets = Vec::new();
        for (t, creating) in create
            .iter()
            .map(|t| (t, true))
            .chain(connect.iter().map(|t| (t, false)))
        {
            if t.server_id != server || t.id.is_nil() || t.direction != Direction::Local {
                return Err("All forwards must be local forwards for the selected server".into());
            }
            if creating {
                if t.local_port_end.is_some() || t.remote_port_end.is_some() {
                    return Err("New batch forwards must describe a single discovered port".into());
                }
                t.pairs()?;
            }
            if let Some((previous, was_creating)) = seen.get(&t.id) {
                if *was_creating != creating || *previous != t {
                    return Err(
                        "A forward ID must have one definition and cannot be both created and connected"
                            .into(),
                    );
                }
                continue;
            }
            seen.insert(t.id, (t, creating));
            targets.push(Target::tunnel(t));
            if targets.len() > MAX_BATCH {
                return Err("Choose between 1 and 256 forwards".into());
            }
        }
        if targets.is_empty() {
            return Err("Choose between 1 and 256 forwards".into());
        }
        Ok(targets)
    }
    fn creations(&self) -> Vec<Tunnel> {
        match self {
            Self::Save(t) | Self::Update { tunnel: t, .. } => vec![t.clone()],
            Self::ForwardBatch { create, .. } => {
                let mut seen = HashSet::new();
                create
                    .iter()
                    .filter(|t| seen.insert(t.id))
                    .cloned()
                    .collect()
            }
            _ => Vec::new(),
        }
    }
    fn execute(&self, call: &Transport) -> Result<(), String> {
        match self {
            Self::Save(t) => call("save_tunnel", json!({"tunnel":t})).map(|_| ()),
            Self::Update { tunnel, expected } => call(
                "update_tunnel",
                json!({"tunnel":tunnel,"expected":expected}),
            )
            .map(|_| ()),
            Self::ForwardBatch {
                server,
                create,
                connect,
            } => call(
                "forward_tunnel_batch",
                json!({"serverId":server,"create":create,"connect":connect}),
            )
            .map(|_| ()),
            Self::Forward {
                server,
                address,
                port,
            } => call(
                "forward_discovered_port",
                json!({"serverId":server,"address":address,"port":port}),
            )
            .map(|_| ()),
            Self::ForwardAndOpen {
                server,
                address,
                port,
                name,
                connection,
            } => forward_open::execute(*server, address, *port, name, connection, call),
            Self::Delete(t) => call("delete_tunnel", json!({"id":t.id})).map(|_| ()),
            Self::DeleteMany { tunnels, .. } => {
                call("delete_tunnels", json!({"tunnels":tunnels})).map(|_| ())
            }
            Self::Open(t) => call("open_tunnel", json!({"id":t.id})).map(|_| ()),
            Self::OpenPort {
                tunnel,
                remote_port,
            } => call(
                "open_tunnel",
                json!({"id":tunnel.id,"remotePort":remote_port}),
            )
            .map(|_| ()),
            Self::SetConnected {
                tunnels, connected, ..
            } => {
                let mut seen = HashSet::new();
                for t in tunnels {
                    if seen.insert(t.id) {
                        call(
                            "set_tunnel_connected",
                            json!({"id":t.id,"connected":connected}),
                        )?;
                    }
                }
                Ok(())
            }
        }
    }
}
#[derive(Clone)]
struct Target {
    id: Option<Uuid>,
    server: Uuid,
    direction: Direction,
    host: String,
    first: u16,
    last: u16,
}
impl Target {
    fn tunnel(t: &Tunnel) -> Self {
        Self {
            id: Some(t.id),
            server: t.server_id,
            direction: t.direction,
            host: host(&t.remote_host),
            first: t.remote_port,
            last: t.remote_port_end.unwrap_or(t.remote_port),
        }
    }
    fn overlaps(&self, other: &Self) -> bool {
        (self.id.is_some() && self.id == other.id)
            || (self.server == other.server
                && self.direction == other.direction
                && self.host == other.host
                && self.first <= other.last
                && other.first <= self.last)
    }
}

#[derive(Clone)]
pub struct Ticket {
    pub id: Uuid,
    result: Arc<Mutex<Option<Result<(), String>>>>,
}
impl Ticket {
    /// None means the call is still running. A transport error is never retried.
    pub fn result(&self) -> Option<Result<(), String>> {
        self.result.lock().unwrap().clone()
    }
}
struct Pending {
    ticket: Ticket,
    server: Uuid,
    kind: Kind,
    targets: Vec<Target>,
    creations: Vec<Tunnel>,
}
pub struct Outcome {
    pub id: Uuid,
    pub server: Uuid,
    pub kind: Kind,
    pub result: Result<(), String>,
}
#[derive(Debug, PartialEq, Eq)]
pub enum StartError {
    Busy,
    Capacity,
    Invalid(String),
    Worker(String),
}
impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy => write!(
                f,
                "A forwarding operation is already running for this destination."
            ),
            Self::Capacity => write!(
                f,
                "Too many forwarding operations are running. Try again after one finishes."
            ),
            Self::Invalid(s) | Self::Worker(s) => write!(f, "{s}"),
        }
    }
}
impl std::error::Error for StartError {}

pub struct Operations {
    call: Arc<Transport>,
    pending: Vec<Pending>,
}
impl Operations {
    pub fn new(backend: Backend) -> Self {
        Self::with_transport(move |method, args| {
            if method == "snapshot" || args.get("expectedConnection").is_some() {
                backend.call_with_timeout(method, args, std::time::Duration::from_secs(30))
            } else {
                backend.call(method, args)
            }
        })
    }
    fn with_transport(
        call: impl Fn(&str, Value) -> Result<Value, String> + Send + Sync + 'static,
    ) -> Self {
        Self {
            call: Arc::new(call),
            pending: Vec::new(),
        }
    }
    pub fn busy_tunnel(&self, tunnel: &Tunnel) -> bool {
        self.busy(&[Target::tunnel(tunnel)])
    }
    /// Saved/created configurations reserved until their outcome is collected.
    /// Include these with saved tunnels when planning local-port allocation.
    pub fn reserved_creations(&self) -> Vec<Tunnel> {
        self.pending
            .iter()
            .flat_map(|p| p.creations.iter().cloned())
            .collect()
    }
    pub fn busy_destination(&self, server: Uuid, address: &str, port: u16) -> bool {
        self.busy(&[Target {
            id: None,
            server,
            direction: Direction::Local,
            host: host(address),
            first: port,
            last: port,
        }])
    }
    fn busy(&self, targets: &[Target]) -> bool {
        self.pending.iter().any(|p| {
            p.targets
                .iter()
                .any(|a| targets.iter().any(|b| a.overlaps(b)))
        })
    }
    pub fn start(&mut self, action: Action) -> Result<Ticket, StartError> {
        let targets = action.targets().map_err(StartError::Invalid)?;
        if self.busy(&targets) {
            return Err(StartError::Busy);
        }
        if self.pending.len() >= MAX_PENDING {
            return Err(StartError::Capacity);
        }
        let ticket = Ticket {
            id: Uuid::new_v4(),
            result: Arc::new(Mutex::new(None)),
        };
        let server = action.server();
        let kind = action.kind();
        let creations = action.creations();
        let result = ticket.result.clone();
        let call = self.call.clone();
        thread::Builder::new()
            .name("newport-forward".into())
            .spawn(move || {
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    action.execute(call.as_ref())
                }))
                .unwrap_or_else(|_| {
                    Err("Forwarding worker stopped before confirming the result. Refresh the connection state before trying again.".into())
                });
                *result.lock().unwrap() = Some(outcome);
            })
            .map_err(|e| {
                StartError::Worker(format!("Could not start forwarding operation: {e}"))
            })?;
        self.pending.push(Pending {
            ticket: ticket.clone(),
            server,
            kind,
            targets,
            creations,
        });
        Ok(ticket)
    }
    /// Collect each outcome once. The caller must refresh snapshots even on error.
    /// Reservations remain held until collection, including if every editor closed.
    pub fn poll(&mut self) -> Vec<Outcome> {
        let mut outcomes = Vec::new();
        self.pending.retain(|p| {
            if let Some(result) = p.ticket.result() {
                outcomes.push(Outcome {
                    id: p.ticket.id,
                    server: p.server,
                    kind: p.kind,
                    result,
                });
                false
            } else {
                true
            }
        });
        outcomes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc, time::Duration};
    fn tunnel(server: Uuid, port: u16) -> Tunnel {
        Tunnel {
            id: Uuid::new_v4(),
            server_id: server,
            name: "fixture".into(),
            local_port: port,
            local_port_end: None,
            remote_port: port,
            remote_port_end: None,
            remote_host: "127.0.0.1".into(),
            direction: Direction::Local,
            auto_connect: false,
            auto_reconnect: false,
            project: None,
        }
    }
    fn wait(ticket: &Ticket) -> Result<(), String> {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(result) = ticket.result() {
                return result;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "worker did not settle"
            );
            thread::sleep(Duration::from_millis(1));
        }
    }
    #[test]
    fn open_preserves_default_endpoint_and_sends_selected_destination_once() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let seen = calls.clone();
        let mut owner = Operations::with_transport(move |method, args| {
            seen.lock().unwrap().push((method.to_owned(), args));
            Ok(Value::Null)
        });
        let mut t = tunnel(Uuid::new_v4(), 3000);
        t.remote_port_end = Some(3005);
        t.local_port = 9000;
        t.local_port_end = Some(9005);
        let ticket = owner.start(Action::Open(t.clone())).unwrap();
        wait(&ticket).unwrap();
        assert_eq!(owner.poll()[0].kind, Kind::Open);
        let ticket = owner
            .start(Action::OpenPort {
                tunnel: t.clone(),
                remote_port: 3003,
            })
            .unwrap();
        wait(&ticket).unwrap();
        assert_eq!(owner.poll()[0].kind, Kind::Open);
        assert_eq!(
            *calls.lock().unwrap(),
            vec![
                ("open_tunnel".to_owned(), json!({"id":t.id})),
                (
                    "open_tunnel".to_owned(),
                    json!({"id":t.id,"remotePort":3003})
                ),
            ]
        );
    }
    #[test]
    fn open_rejects_reverse_out_of_range_and_invalid_ranges_before_dispatch() {
        let mut owner = Operations::with_transport(|_, _| panic!("Invalid endpoint dispatched"));
        let mut t = tunnel(Uuid::new_v4(), 3000);
        for remote_port in [0, 2999, 3001] {
            assert!(matches!(
                owner.start(Action::OpenPort {
                    tunnel: t.clone(),
                    remote_port,
                }),
                Err(StartError::Invalid(_))
            ));
        }
        t.direction = Direction::Remote;
        assert!(matches!(
            owner.start(Action::Open(t.clone())),
            Err(StartError::Invalid(_))
        ));
        assert!(matches!(
            owner.start(Action::OpenPort {
                tunnel: t.clone(),
                remote_port: 3000,
            }),
            Err(StartError::Invalid(_))
        ));
        t.direction = Direction::Local;
        t.remote_port_end = Some(3004);
        assert!(matches!(
            owner.start(Action::Open(t)),
            Err(StartError::Invalid(_))
        ));
        assert!(owner.poll().is_empty());
    }
    #[test]
    fn stale_edit_failure_never_falls_back_to_save_or_replays() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let seen = calls.clone();
        let mut owner = Operations::with_transport(move |method, args| {
            seen.lock().unwrap().push((method.to_owned(), args));
            Err("This forward was deleted while the editor was open.".into())
        });
        let original = tunnel(Uuid::new_v4(), 3000);
        let mut edited = original.clone();
        edited.remote_port = 3001;
        edited.name = "Changed".into();
        let ticket = owner
            .start(Action::Update {
                tunnel: edited.clone(),
                expected: original.clone(),
            })
            .unwrap();
        assert!(owner.busy_tunnel(&original));
        assert!(owner.busy_destination(original.server_id, "127.0.0.1", 3001));
        assert_eq!(owner.reserved_creations(), vec![edited.clone()]);
        assert!(wait(&ticket).unwrap_err().contains("deleted"));
        let outcomes = owner.poll();
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].kind, Kind::Save);
        assert!(owner.poll().is_empty());
        assert!(!owner.busy_tunnel(&original));
        assert_eq!(
            *calls.lock().unwrap(),
            vec![(
                "update_tunnel".into(),
                json!({"tunnel":edited,"expected":original}),
            )]
        );
    }

    #[test]
    fn editing_a_different_identity_is_rejected_before_dispatch() {
        let mut owner = Operations::with_transport(|_, _| panic!("invalid edit dispatched"));
        let original = tunnel(Uuid::new_v4(), 3000);
        let mut edited = original.clone();
        edited.id = Uuid::new_v4();
        assert!(matches!(
            owner.start(Action::Update {
                tunnel: edited,
                expected: original
            }),
            Err(StartError::Invalid(_))
        ));
        assert!(owner.poll().is_empty());
    }

    #[test]
    fn ownership_survives_dropped_page_tickets_and_prevents_alias_duplicates() {
        let (started_tx, started) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        let blocked = Mutex::new(blocked);
        let mut owner = Operations::with_transport(move |method, _| {
            started_tx.send(method.to_owned()).unwrap();
            blocked.lock().unwrap().recv().unwrap();
            Ok(Value::Null)
        });
        let server = Uuid::new_v4();
        let t = tunnel(server, 3000);
        let ticket = owner
            .start(Action::SetConnected {
                server,
                tunnels: vec![t.clone()],
                connected: true,
            })
            .unwrap();
        started.recv_timeout(Duration::from_secs(3)).unwrap();
        // A page's local ticket disappearing has no bearing on operation lifetime.
        drop(ticket);
        assert!(owner.busy_tunnel(&t));
        assert!(matches!(
            owner.start(Action::Forward {
                server,
                address: "0.0.0.0".into(),
                port: 3000
            }),
            Err(StartError::Busy)
        ));
        assert!(matches!(
            owner.start(Action::Save(t.clone())),
            Err(StartError::Busy)
        ));
        assert!(owner.poll().is_empty());
        release.send(()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let done = loop {
            let done = owner.poll();
            if !done.is_empty() {
                break done;
            }
            assert!(std::time::Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        };
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].server, server);
        assert_eq!(done[0].kind, Kind::SetConnected);
        assert!(!owner.busy_tunnel(&t));
        assert!(owner.poll().is_empty());
        assert!(started.try_recv().is_err());
    }
    #[test]
    fn ranges_conflict_but_other_destinations_and_servers_can_progress() {
        let (release, blocked) = mpsc::channel();
        let blocked = Mutex::new(blocked);
        let mut owner = Operations::with_transport(move |_, _| {
            blocked.lock().unwrap().recv().unwrap();
            Ok(Value::Null)
        });
        let server = Uuid::new_v4();
        let mut t = tunnel(server, 3000);
        t.remote_port_end = Some(3005);
        t.local_port_end = Some(3005);
        let first = owner.start(Action::Save(t.clone())).unwrap();
        assert!(owner.busy_destination(server, "localhost", 3002));
        assert!(!owner.busy_destination(server, "::1", 3002));
        let second = owner
            .start(Action::Forward {
                server,
                address: "127.0.0.1".into(),
                port: 4000,
            })
            .unwrap();
        let third = owner
            .start(Action::Forward {
                server: Uuid::new_v4(),
                address: "127.0.0.1".into(),
                port: 3002,
            })
            .unwrap();
        for _ in 0..3 {
            release.send(()).unwrap();
        }
        wait(&first).unwrap();
        wait(&second).unwrap();
        wait(&third).unwrap();
        assert_eq!(owner.poll().len(), 3);
    }
    #[test]
    fn errors_are_delivered_once_without_replaying_or_continuing_a_batch() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let seen = calls.clone();
        let mut owner = Operations::with_transport(move |method, args| {
            seen.lock().unwrap().push((method.to_owned(), args));
            Err("connection interrupted".into())
        });
        let server = Uuid::new_v4();
        let a = tunnel(server, 3000);
        let b = tunnel(server, 3001);
        let ticket = owner
            .start(Action::SetConnected {
                server,
                tunnels: vec![a.clone(), b],
                connected: true,
            })
            .unwrap();
        assert!(wait(&ticket).is_err());
        let outcomes = owner.poll();
        assert_eq!(outcomes.len(), 1);
        assert!(outcomes[0].result.is_err());
        assert!(owner.poll().is_empty());
        assert_eq!(calls.lock().unwrap().len(), 1);
        assert!(!owner.busy_tunnel(&a));
    }
    #[test]
    fn batch_deduplicates_ids_and_rejects_cross_server_selection_before_dispatch() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let seen = calls.clone();
        let mut owner = Operations::with_transport(move |method, args| {
            seen.lock().unwrap().push((method.to_owned(), args));
            Ok(Value::Null)
        });
        let server = Uuid::new_v4();
        let a = tunnel(server, 3000);
        assert!(matches!(
            owner.start(Action::SetConnected {
                server,
                tunnels: vec![a.clone(), tunnel(Uuid::new_v4(), 3001)],
                connected: true
            }),
            Err(StartError::Invalid(_))
        ));
        let ticket = owner
            .start(Action::SetConnected {
                server,
                tunnels: vec![a.clone(), a],
                connected: false,
            })
            .unwrap();
        wait(&ticket).unwrap();
        assert_eq!(calls.lock().unwrap().len(), 1);
        assert_eq!(calls.lock().unwrap()[0].1["connected"], false);
    }

    #[test]
    fn forward_batch_submits_one_server_authoritative_rpc_and_reserves_unique_creations() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let seen = calls.clone();
        let mut owner = Operations::with_transport(move |method, args| {
            seen.lock().unwrap().push((method.to_owned(), args));
            Ok(Value::Null)
        });
        let server = Uuid::new_v4();
        let a = tunnel(server, 3000);
        let b = tunnel(server, 3001);
        let existing = tunnel(server, 3002);
        let ticket = owner
            .start(Action::ForwardBatch {
                server,
                create: vec![a.clone(), b.clone(), a.clone()],
                connect: vec![existing.clone(), existing.clone()],
            })
            .unwrap();
        wait(&ticket).unwrap();
        assert_eq!(
            *calls.lock().unwrap(),
            vec![(
                "forward_tunnel_batch".to_owned(),
                json!({"serverId":server,"create":[a,b,a],"connect":[existing,existing]})
            )]
        );
        assert_eq!(owner.reserved_creations(), vec![a, b]);
        let outcomes = owner.poll();
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].id, ticket.id);
        assert_eq!(outcomes[0].kind, Kind::ForwardBatch);
        assert_eq!(outcomes[0].server, server);
        assert_eq!(outcomes[0].result, Ok(()));
        assert!(owner.reserved_creations().is_empty());
    }

    #[test]
    fn forward_batch_failure_is_delivered_once_without_retry_or_fallback_writes() {
        let server = Uuid::new_v4();
        let a = tunnel(server, 3000);
        let b = tunnel(server, 3001);
        let existing = tunnel(server, 3002);
        for error in [
            "connection interrupted",
            "batch saved, but connection failed",
        ] {
            let calls = Arc::new(Mutex::new(Vec::new()));
            let seen = calls.clone();
            let mut owner = Operations::with_transport(move |method, args| {
                let mut calls = seen.lock().unwrap();
                calls.push((method.to_owned(), args));
                Err(error.into())
            });
            let ticket = owner
                .start(Action::ForwardBatch {
                    server,
                    create: vec![a.clone(), b.clone()],
                    connect: vec![existing.clone()],
                })
                .unwrap();
            assert_eq!(wait(&ticket), Err(error.into()));
            assert!(owner.busy_tunnel(&existing));
            assert_eq!(owner.reserved_creations(), vec![a.clone(), b.clone()]);
            let outcomes = owner.poll();
            assert_eq!(outcomes.len(), 1);
            assert_eq!(outcomes[0].result, Err(error.into()));
            assert!(owner.poll().is_empty());
            assert!(owner.reserved_creations().is_empty());
            assert!(!owner.busy_tunnel(&existing));
            assert_eq!(
                *calls.lock().unwrap(),
                vec![(
                    "forward_tunnel_batch".to_owned(),
                    json!({"serverId":server,"create":[a,b],"connect":[existing]})
                )]
            );
        }
    }

    #[test]
    fn forward_batch_rejects_invalid_or_ambiguous_definitions_before_dispatch() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let seen = calls.clone();
        let mut owner = Operations::with_transport(move |method, _| {
            seen.lock().unwrap().push(method.to_owned());
            Ok(Value::Null)
        });
        let server = Uuid::new_v4();
        let a = tunnel(server, 3000);
        let mut conflicting = a.clone();
        conflicting.local_port = 4000;
        let mut nil_id = a.clone();
        nil_id.id = Uuid::nil();
        let mut invalid_pairs = a.clone();
        invalid_pairs.remote_port = 0;
        let mut range = a.clone();
        range.local_port_end = Some(3001);
        range.remote_port_end = Some(3001);
        let mut reverse = a.clone();
        reverse.direction = Direction::Remote;
        let other_server = tunnel(Uuid::new_v4(), 3001);
        for (create, connect) in [
            (vec![], vec![]),
            (vec![nil_id.clone()], vec![]),
            (vec![], vec![nil_id]),
            (vec![other_server.clone()], vec![]),
            (vec![], vec![other_server]),
            (vec![invalid_pairs], vec![]),
            (vec![range], vec![]),
            (vec![reverse.clone()], vec![]),
            (vec![], vec![reverse]),
            (vec![a.clone(), conflicting.clone()], vec![]),
            (vec![], vec![a.clone(), conflicting]),
            (vec![a.clone()], vec![a]),
        ] {
            assert!(matches!(
                owner.start(Action::ForwardBatch {
                    server,
                    create,
                    connect,
                }),
                Err(StartError::Invalid(_))
            ));
        }
        assert!(matches!(
            owner.start(Action::ForwardBatch {
                server: Uuid::nil(),
                create: vec![tunnel(Uuid::nil(), 3000)],
                connect: vec![],
            }),
            Err(StartError::Invalid(_))
        ));
        assert!(calls.lock().unwrap().is_empty());
        assert!(owner.reserved_creations().is_empty());
    }

    #[test]
    fn forward_batch_limits_unique_targets_across_both_lists() {
        let server = Uuid::new_v4();
        let tunnels: Vec<_> = (0..MAX_BATCH)
            .map(|i| tunnel(server, 3000 + i as u16))
            .collect();
        let mut create = tunnels[..128].to_vec();
        create.extend(tunnels[..128].iter().cloned());
        create.extend(tunnels[..128].iter().cloned());
        let connect = tunnels[128..].to_vec();
        assert_eq!(
            Action::ForwardBatch {
                server,
                create: create.clone(),
                connect: connect.clone(),
            }
            .targets()
            .unwrap()
            .len(),
            MAX_BATCH
        );
        create.push(tunnel(server, 4000));
        assert!(
            Action::ForwardBatch {
                server,
                create,
                connect,
            }
            .targets()
            .is_err()
        );
    }

    #[test]
    fn forward_batch_reserves_all_destinations_and_ids_until_collection() {
        let (started_tx, started) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        let blocked = Mutex::new(blocked);
        let mut owner = Operations::with_transport(move |method, _| {
            if method == "forward_tunnel_batch" {
                started_tx.send(()).unwrap();
                blocked.lock().unwrap().recv().unwrap();
            }
            Ok(Value::Null)
        });
        let server = Uuid::new_v4();
        let a = tunnel(server, 3000);
        let existing = tunnel(server, 3001);
        let ticket = owner
            .start(Action::ForwardBatch {
                server,
                create: vec![a.clone()],
                connect: vec![existing.clone()],
            })
            .unwrap();
        started.recv_timeout(Duration::from_secs(3)).unwrap();
        for t in [&a, &existing] {
            assert!(owner.busy_destination(server, "localhost", t.remote_port));
            assert!(matches!(
                owner.start(Action::Delete(t.clone())),
                Err(StartError::Busy)
            ));
            let mut moved = t.clone();
            moved.server_id = Uuid::new_v4();
            moved.remote_port = 9000;
            assert!(owner.busy_tunnel(&moved));
        }
        assert_eq!(owner.reserved_creations(), vec![a.clone()]);
        let mut copy = owner.reserved_creations();
        copy[0].local_port = 9000;
        assert_eq!(owner.reserved_creations(), vec![a.clone()]);
        assert!(owner.poll().is_empty());
        release.send(()).unwrap();
        wait(&ticket).unwrap();
        drop(ticket);
        assert!(owner.busy_tunnel(&a));
        assert!(owner.busy_tunnel(&existing));
        assert_eq!(owner.reserved_creations(), vec![a.clone()]);
        assert_eq!(owner.poll().len(), 1);
        assert!(!owner.busy_tunnel(&a));
        assert!(!owner.busy_tunnel(&existing));
        assert!(owner.reserved_creations().is_empty());
    }

    #[test]
    fn saved_tunnels_are_also_reserved_for_parallel_port_planning() {
        let mut owner = Operations::with_transport(|_, _| Ok(Value::Null));
        let t = tunnel(Uuid::new_v4(), 3000);
        let ticket = owner.start(Action::Save(t.clone())).unwrap();
        wait(&ticket).unwrap();
        assert_eq!(owner.reserved_creations(), vec![t]);
        assert_eq!(owner.poll().len(), 1);
        assert!(owner.reserved_creations().is_empty());
    }
}
