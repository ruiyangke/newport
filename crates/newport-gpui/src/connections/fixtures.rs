use newport_domain::{
    port_discovery as d,
    port_grouping::{Entry, Port},
};
use newport_protocol::{
    desktop::connection::{PortsObservation, discovery::Discovery},
    monitor::ports::{Project, discovery as p},
};
use std::{
    collections::BTreeMap,
    hash::{Hash, Hasher},
};

#[derive(Clone)]
pub(crate) struct Listener {
    endpoint: p::Endpoint,
    process: p::Process,
}

impl Listener {
    pub(crate) fn new(port: u16) -> Self {
        Self {
            endpoint: p::Endpoint {
                address: "127.0.0.1".parse().unwrap(),
                port,
                interface: None,
            },
            process: p::Process {
                id: 1,
                identity: p::ProcessIdentity {
                    pid: u32::from(port),
                    start_ticks: 100,
                },
                name: Some("app".into()),
                executable_name: Some("app".into()),
                uid: Some(1000),
                account: None,
                details: None,
                metadata: p::Report::complete(),
            },
        }
    }
    pub(crate) fn address(mut self, address: &str) -> Self {
        self.endpoint.address = address.parse().unwrap();
        self
    }
    pub(crate) fn pid(mut self, pid: u32) -> Self {
        self.process.identity.pid = pid;
        self
    }
    pub(crate) fn process(
        mut self,
        name: &str,
        executable: &str,
        args: &[&str],
        directory: Option<&str>,
    ) -> Self {
        self.process.name = Some(name.into());
        self.process.executable_name = Some(executable.rsplit('/').next().unwrap().into());
        let details = self.process.details.get_or_insert_with(Default::default);
        details.executable = executable.starts_with('/').then(|| p::ObservedPath {
            value: executable.into(),
            deleted: false,
            lossy: false,
        });
        details.directory = directory.map(|value| p::ObservedPath {
            value: value.into(),
            deleted: false,
            lossy: false,
        });
        details.arguments = args.iter().map(|value| (*value).into()).collect();
        self
    }
    pub(crate) fn project(mut self, root: &str) -> Self {
        self.process
            .details
            .get_or_insert_with(Default::default)
            .project = Some(Project {
            root: root.into(),
            marker: ".git".into(),
            ..Default::default()
        });
        self
    }
    pub(crate) fn checkout(mut self, path: &str, branch: Option<&str>, main: Option<bool>) -> Self {
        let project = self
            .process
            .details
            .as_mut()
            .unwrap()
            .project
            .as_mut()
            .expect("project first");
        project.worktree = Some(path.into());
        project.branch = branch.map(str::to_owned);
        project.main = main;
        self
    }
    pub(crate) fn member(self) -> d::Member {
        let reading = newport_client::discovery::Reading::prepare(Ok(observation([self])), None);
        reading.view().expect("valid discovery fixture").members()[0].clone()
    }
    pub(crate) fn port(self) -> Port {
        Port::Observed(d::Row {
            members: vec![self.member()],
        })
    }
    pub(crate) fn entry(self) -> Entry {
        let port = self.port();
        Entry {
            addresses: port.addresses(),
            port,
            forwards: vec![],
        }
    }
}

pub(crate) fn observation(listeners: impl IntoIterator<Item = Listener>) -> PortsObservation {
    let mut snapshot = p::Snapshot {
        generation: 1,
        scope: p::Scope {
            network: Some(p::Namespace {
                device: 1,
                inode: 2,
            }),
            processes: Some(p::Namespace {
                device: 1,
                inode: 3,
            }),
            boot_id: Some("18f68fcb-71e9-47fe-a226-e9da27d82953".into()),
        },
        coverage: p::Coverage {
            ipv4: p::Report::complete(),
            ipv6: p::Report::complete(),
            processes: p::Report::complete(),
            details: p::Report::complete(),
            containers: p::Report::not_applicable(),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut processes = BTreeMap::new();
    for listener in listeners {
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        (
            listener.endpoint.address,
            listener.endpoint.port,
            listener.endpoint.interface,
            listener.process.identity.pid,
            listener.process.identity.start_ticks,
        )
            .hash(&mut hash);
        let process = if let Some(id) = processes.get(&listener.process.identity) {
            let expected = p::Process {
                id: *id,
                ..listener.process
            };
            assert_eq!(
                snapshot.processes[usize::try_from(*id - 1).unwrap()],
                expected
            );
            *id
        } else {
            let id = u32::try_from(snapshot.processes.len() + 1).unwrap();
            processes.insert(listener.process.identity, id);
            snapshot.processes.push(p::Process {
                id,
                ..listener.process
            });
            id
        };
        snapshot.sockets.push(p::Socket {
            id: u32::try_from(snapshot.sockets.len() + 1).unwrap(),
            endpoint: listener.endpoint,
            source: p::SocketSource::Diagnostic,
            inode: hash.finish(),
            cookie: Some(hash.finish()),
            uid: 1000,
            account: None,
            ipv6_only: None,
            cgroup: None,
            holders: vec![process],
            holder_state: p::HolderState::Observed,
            holders_truncated: false,
            services: vec![],
        });
    }
    snapshot.validate().expect("valid discovery fixture");
    PortsObservation {
        data: Some(Discovery {
            current: snapshot,
            retained: vec![],
            retention: p::Report::complete(),
        }),
        error: None,
        fetching: false,
        stale: false,
        revision: 1,
        updated_at: Some(1000),
    }
}

pub(crate) fn put(view: &mut crate::Newport, observation: PortsObservation) {
    let server = view.server.clone();
    let scope = view.selected_connection().unwrap();
    let previous = view
        .connections_state
        .get(&server)
        .and_then(|state| state.reading.as_ref());
    let reading = newport_client::discovery::Reading::prepare(Ok(observation), previous);
    assert!(reading.view().is_some() || reading.status().error.is_none());
    view.resources.invalidate("ports");
    let generation = view.resources.begin_refresh("ports", false).unwrap();
    assert!(view.complete_connections_reading(
        &server,
        generation,
        super::DiscoveryCompletion { scope, reading }
    ));
}

pub(crate) fn publication(port: u16, name: &str, image: &str) -> Port {
    let mut member = Listener::new(port).member();
    member.reference.record = d::Record::Publication(0);
    member.kind = d::Kind::Publication(d::Publication {
        engine: Some("engine".into()),
        container: name.into(),
        name: name.into(),
        image: Some(image.into()),
        service: None,
        compose: None,
        directory: None,
        project: None,
        target_port: port,
        target_address: None,
        running: Some(true),
    });
    Port::Observed(d::Row {
        members: vec![member],
    })
}
