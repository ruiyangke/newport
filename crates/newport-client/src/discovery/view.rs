//! Explicit protocol-to-domain adapter. Full evidence remains available for
//! details; the smaller domain inputs decide grouping, naming and routing.
use newport_domain::{port_context::ProcessFacts, port_discovery as d};
use newport_protocol::{
    desktop::connection::discovery::Discovery,
    monitor::{
        Invalid,
        ports::{Project, discovery as p},
    },
};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, Debug)]
pub struct View {
    evidence: Arc<Discovery>,
    members: Vec<d::Member>,
    inventory: d::Inventory,
}

impl View {
    pub fn new(evidence: Discovery, stale: bool) -> Result<Self, Invalid> {
        evidence.validate()?;
        let mut members = Vec::new();
        append(&evidence.current, stale, &mut members);
        for snapshot in &evidence.retained {
            append(snapshot, true, &mut members);
        }
        let inventory = d::Inventory { fresh: !stale };
        Ok(Self {
            evidence: Arc::new(evidence),
            members,
            inventory,
        })
    }

    pub fn evidence(&self) -> &Discovery {
        &self.evidence
    }
    pub fn members(&self) -> &[d::Member] {
        &self.members
    }
    pub fn inventory(&self) -> &d::Inventory {
        &self.inventory
    }
    pub fn inventory_incomplete(&self) -> bool {
        let coverage = &self.evidence.current.coverage;
        !self.inventory.fresh
            || [
                &coverage.ipv4,
                &coverage.ipv6,
                &coverage.containers,
                &self.evidence.retention,
            ]
            .into_iter()
            .any(|report| {
                !matches!(
                    report.outcome,
                    p::Outcome::Complete | p::Outcome::NotApplicable
                )
            })
    }
    pub fn groups(&self) -> Vec<d::Group> {
        d::group(self.members.clone())
    }

    pub(super) fn as_stale(&self) -> Self {
        let mut retained = self.clone();
        retained.inventory.fresh = false;
        for member in &mut retained.members {
            member.stale = true;
        }
        retained
    }

    /// A generation-local reference resolves only within this immutable view.
    /// Cross-refresh choices must be resolved using the domain Selection rules.
    pub fn snapshot(&self, reference: d::Reference) -> Option<&p::Snapshot> {
        std::iter::once(&self.evidence.current)
            .chain(&self.evidence.retained)
            .find(|snapshot| snapshot.generation == reference.generation)
    }

    pub fn socket(&self, reference: d::Reference) -> Option<&p::Socket> {
        let d::Record::Socket(id) = reference.record else {
            return None;
        };
        self.snapshot(reference)?
            .sockets
            .iter()
            .find(|socket| socket.id == id)
    }

    pub fn publication(&self, reference: d::Reference) -> Option<&p::Publication> {
        let d::Record::Publication(index) = reference.record else {
            return None;
        };
        self.snapshot(reference)?.publications.get(index)
    }
}

fn scope(value: &p::Scope) -> Option<Arc<d::Scope>> {
    Some(Arc::new(d::Scope {
        network: (value.network?.device, value.network?.inode),
        processes: (value.processes?.device, value.processes?.inode),
        boot: value.boot_id.clone()?,
    }))
}

fn endpoint(value: p::Endpoint) -> d::Endpoint {
    d::Endpoint {
        address: value.address,
        port: value.port,
        interface: value.interface,
    }
}

fn project(value: &Project) -> d::Project {
    d::Project {
        root: value.root.clone(),
        marker: value.marker.clone(),
        checkout: value.worktree.as_ref().map(|path| d::Checkout {
            path: path.clone(),
            branch: value.branch.clone(),
            main: value.main,
        }),
    }
}

fn holder(value: &p::Process) -> d::Holder {
    let details = value.details.as_ref();
    let facts = ProcessFacts {
        name: value.name.as_deref(),
        executable_name: value.executable_name.as_deref(),
        executable: details
            .and_then(|details| details.executable.as_ref())
            .map(|path| path.value.as_str()),
        directory: details
            .and_then(|details| details.directory.as_ref())
            .map(|path| path.value.as_str()),
        arguments: details
            .map(|details| details.arguments.as_slice())
            .unwrap_or(&[]),
    };
    d::Holder {
        identity: d::Process {
            pid: value.identity.pid,
            start: value.identity.start_ticks,
        },
        name: value.name.clone(),
        executable: value.executable_name.clone(),
        label: facts.label(),
        account: value.account.clone(),
        uid: value.uid,
        project: details
            .and_then(|details| details.project.as_ref())
            .map(project),
        kinds: facts.kinds(),
        details: facts.details(),
    }
}

fn append(snapshot: &p::Snapshot, stale: bool, members: &mut Vec<d::Member>) {
    let observation = d::Observation::default();
    let scope = scope(&snapshot.scope);
    let processes: BTreeMap<_, _> = snapshot
        .processes
        .iter()
        .map(|process| (process.id, Arc::new(holder(process))))
        .collect();
    let services: BTreeMap<_, _> = snapshot
        .services
        .iter()
        .map(|service| (service.id, service))
        .collect();
    for socket in &snapshot.sockets {
        let mut holders: Vec<_> = socket
            .holders
            .iter()
            .filter_map(|id| processes.get(id))
            .cloned()
            .collect();
        holders.sort_by_key(|holder| holder.identity);
        let mut units: Vec<_> = socket
            .services
            .iter()
            .filter_map(|association| {
                let service = services.get(&association.service)?;
                Some(d::Unit {
                    manager_uid: match service.manager {
                        p::Manager::System => None,
                        p::Manager::User { uid } => Some(uid),
                    },
                    name: service.unit.clone(),
                    socket_cgroup: association.relation == p::ServiceRelation::SocketCgroup,
                })
            })
            .collect();
        units.sort();
        units.dedup();
        members.push(d::Member {
            observation: observation.clone(),
            reference: d::Reference {
                generation: snapshot.generation,
                record: d::Record::Socket(socket.id),
            },
            scope: scope.clone(),
            endpoint: endpoint(socket.endpoint),
            stale,
            kind: d::Kind::Socket(d::Socket {
                inode: socket.inode,
                cookie: socket.cookie,
                uid: socket.uid,
                account: socket.account.clone(),
                ipv6_only: socket.ipv6_only,
                holders,
                holder_coverage: if socket.holders_truncated {
                    d::HolderCoverage::Truncated
                } else if snapshot.coverage.processes.outcome == p::Outcome::Complete
                    && socket.holder_state == p::HolderState::Observed
                {
                    d::HolderCoverage::Complete
                } else {
                    d::HolderCoverage::Partial
                },
                units,
            }),
        });
    }
    for (index, publication) in snapshot.publications.iter().enumerate() {
        let compose = publication.compose.as_ref();
        members.push(d::Member {
            observation: observation.clone(),
            reference: d::Reference {
                generation: snapshot.generation,
                record: d::Record::Publication(index),
            },
            scope: scope.clone(),
            endpoint: endpoint(publication.host),
            stale,
            kind: d::Kind::Publication(d::Publication {
                engine: publication.engine.clone(),
                container: publication.container.clone(),
                name: publication.name.clone(),
                image: publication.image.clone(),
                service: compose.and_then(|value| value.service.clone()),
                compose: compose.and_then(|value| value.project.clone()),
                directory: compose.and_then(|value| value.directory.clone()),
                project: compose
                    .and_then(|value| value.directory_project.as_ref())
                    .map(project),
                target_port: publication.target_port,
                target_address: publication.target_address,
                running: match publication.state {
                    p::ContainerState::Running => Some(true),
                    p::ContainerState::Stopped => Some(false),
                    p::ContainerState::Unknown => None,
                },
            }),
        });
    }
}

#[cfg(test)]
mod tests;
