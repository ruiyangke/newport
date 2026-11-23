use super::*;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum GroupKind {
    Project(Project),
    Compose {
        name: String,
        directory: Option<String>,
    },
    System,
    /// Published container ports outside a Compose project.
    Containers,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub kind: GroupKind,
    pub rows: Vec<Row>,
}

/// Who holds a socket, as a row tells holders apart. Processes of one program
/// run by one account share an owner, so its IPv4 and IPv6 listeners share a
/// row even when separate processes hold them.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Owner {
    Program {
        name: String,
        path: Option<String>,
        uid: u32,
    },
    /// Without a program name and account, only the same process is the same owner.
    Process(Process),
}

fn owner(holder: &Holder) -> Owner {
    match (
        holder.name.as_ref().or(holder.executable.as_ref()),
        holder.uid,
    ) {
        (Some(name), Some(uid)) => Owner::Program {
            name: name.clone(),
            path: holder.path.clone(),
            uid,
        },
        _ => Owner::Process(holder.identity),
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Attribution {
    Holders(Vec<Owner>, Vec<Unit>),
    Unit(Unit),
    Publication {
        engine: Option<String>,
        container: String,
        target: u16,
        address: Option<IpAddr>,
        running: Option<bool>,
    },
    Separate(Reference),
}

fn attribution(member: &Member) -> Attribution {
    if member.scope.is_none() {
        return Attribution::Separate(member.reference);
    }
    match &member.kind {
        Kind::Socket(socket)
            if !socket.holders.is_empty()
                && (socket.holder_coverage == HolderCoverage::Complete
                    || (socket.holder_coverage == HolderCoverage::Partial
                        && member.unit().is_some())) =>
        {
            // Verified service evidence survives unrelated inspection failures.
            // Different programs still receive different row keys.
            let holders = socket
                .holders
                .iter()
                .map(|holder| owner(holder))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let units = socket
                .units
                .iter()
                .cloned()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            Attribution::Holders(holders, units)
        }
        Kind::Socket(socket)
            if socket.holders.is_empty() && socket.holder_coverage != HolderCoverage::Truncated =>
        {
            member
                .unit()
                .cloned()
                .map(Attribution::Unit)
                .unwrap_or(Attribution::Separate(member.reference))
        }
        Kind::Socket(_) => Attribution::Separate(member.reference),
        Kind::Publication(publication) => Attribution::Publication {
            engine: publication.engine.clone(),
            container: publication.container.clone(),
            target: publication.target_port,
            address: publication.target_address,
            running: publication.running,
        },
    }
}

fn group_kind(member: &Member) -> GroupKind {
    if let Some(project) = member.project() {
        return GroupKind::Project(project.clone());
    }
    match &member.kind {
        Kind::Publication(publication) => publication
            .compose
            .as_ref()
            .filter(|name| !name.is_empty())
            .map(|name| GroupKind::Compose {
                name: name.clone(),
                directory: publication.directory.clone(),
            })
            .unwrap_or(GroupKind::Containers),
        Kind::Socket(_) if member.unit().is_some_and(|unit| unit.manager_uid.is_none()) => {
            GroupKind::System
        }
        Kind::Socket(_) => GroupKind::Other,
    }
}

/// Programs that listen on a published container port for the engine: rootful
/// Docker's proxy, rootless Docker's RootlessKit and Podman's network helpers.
const PROXIES: [&str; 6] = [
    "docker-proxy",
    "rootlesskit",
    "rootlessport",
    "pasta",
    "pasta.avx2",
    "slirp4netns",
];

/// Every holder of the socket is a known engine proxy.
fn named_proxy(socket: &Socket) -> bool {
    !socket.holders.is_empty()
        && socket.holders.iter().all(|holder| {
            holder
                .name
                .iter()
                .chain(&holder.executable)
                .any(|name| PROXIES.contains(&name.as_str()))
        })
}

/// A socket a container engine may hold for a published port: its holders are
/// engine proxies, or the scan couldn't see who holds it. A socket a complete
/// scan found no holder for is not one.
fn proxy(member: &Member) -> bool {
    let Kind::Socket(socket) = &member.kind else {
        return false;
    };
    named_proxy(socket)
        || (socket.holders.is_empty() && socket.holder_coverage != HolderCoverage::Complete)
}

/// How closely a publication's host address accounts for a socket's address,
/// best first. Docker can list only `0.0.0.0` for a port its proxy also holds
/// on `::`; only a known proxy is matched across families that way.
fn covers(publication: IpAddr, socket: &Member) -> Option<u8> {
    let address = socket.endpoint.address;
    if publication == address {
        Some(0)
    } else if publication.is_unspecified() && publication.is_ipv4() == address.is_ipv4() {
        Some(1)
    } else if publication.is_unspecified()
        && address.is_unspecified()
        && matches!(&socket.kind, Kind::Socket(value) if named_proxy(value))
    {
        Some(2)
    } else {
        None
    }
}

pub(super) fn shares_attribution(members: &[Member]) -> bool {
    let Some(first) = members
        .iter()
        .find(|member| matches!(member.kind, Kind::Publication(_)))
        .or(members.first())
    else {
        return false;
    };
    let owner = attribution(first);
    if matches!(owner, Attribution::Separate(_)) {
        return false;
    }
    let kind = group_kind(first);
    let published = matches!(first.kind, Kind::Publication(_));
    members.iter().all(|member| {
        member.scope == first.scope
            && member.reference.generation == first.reference.generation
            && ((published && member.endpoint.port == first.endpoint.port && proxy(member))
                || (attribution(member) == owner && group_kind(member) == kind))
    })
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    port: u16,
    scope: Option<Arc<Scope>>,
    generation: u64,
    stale: bool,
    attribution: Attribution,
}

impl Key {
    fn same_observation(&self, other: &Self) -> bool {
        self.port == other.port
            && self.scope.is_some()
            && self.scope == other.scope
            && self.generation == other.generation
            && self.stale == other.stale
    }
}

/// The publication row that best accounts for a proxy socket, when exactly
/// one does.
fn publication_for<'a>(
    socket: &Member,
    key: &Key,
    publications: &'a [(GroupKind, Key)],
    rows: &BTreeMap<(GroupKind, Key), Row>,
) -> Option<&'a (GroupKind, Key)> {
    let mut best = None;
    let mut tied = false;
    // A stopped container's port is not listening, so a socket there belongs
    // to something else.
    for publication in publications.iter().filter(|(_, candidate)| {
        candidate.same_observation(key)
            && !matches!(
                candidate.attribution,
                Attribution::Publication {
                    running: Some(false),
                    ..
                }
            )
    }) {
        let Some(rank) = rows[publication]
            .members
            .iter()
            .filter_map(|member| covers(member.endpoint.address, socket))
            .min()
        else {
            continue;
        };
        match best {
            Some((closest, _)) if rank > closest => {}
            Some((closest, _)) if rank == closest => tied = true,
            _ => {
                best = Some((rank, publication));
                tied = false;
            }
        }
    }
    best.filter(|_| !tied).map(|(_, publication)| publication)
}

/// A container's published port is one row: the engine's proxy sockets on that
/// port join the publication's row. A proxy that fits no single publication,
/// or a different program on the port, keeps its own row.
fn fold_proxies(rows: &mut BTreeMap<(GroupKind, Key), Row>) {
    let publications: Vec<_> = rows
        .keys()
        .filter(|(_, key)| matches!(key.attribution, Attribution::Publication { .. }))
        .cloned()
        .collect();
    if publications.is_empty() {
        return;
    }
    let mut moves = Vec::new();
    for (source, row) in rows.iter() {
        for (index, member) in row.members.iter().enumerate() {
            if proxy(member)
                && let Some(target) = publication_for(member, &source.1, &publications, rows)
            {
                moves.push((source.clone(), index, target.clone()));
            }
        }
    }
    // Remove from the back so earlier indexes stay valid.
    for (source, index, target) in moves.into_iter().rev() {
        let Some(row) = rows.get_mut(&source) else {
            continue;
        };
        let member = row.members.remove(index);
        if row.members.is_empty() {
            rows.remove(&source);
        }
        if let Some(row) = rows.get_mut(&target) {
            row.members.push(member);
        }
    }
    for row in rows.values_mut() {
        row.members.sort_by(|a, b| {
            a.endpoint
                .cmp(&b.endpoint)
                .then_with(|| a.reference.cmp(&b.reference))
        });
    }
}

/// Merge compatible attribution only within a known scope and one observation
/// generation. Unknown owners and different programs stay separate. Sort by
/// facts, never by input/hash-map order, so refreshes do not change the first row.
pub fn group(mut members: Vec<Member>) -> Vec<Group> {
    members.sort_by(|a, b| {
        a.endpoint
            .cmp(&b.endpoint)
            .then_with(|| a.reference.cmp(&b.reference))
    });
    let mut rows: BTreeMap<(GroupKind, Key), Row> = BTreeMap::new();
    for member in members {
        let key = Key {
            port: member.endpoint.port,
            scope: member.scope.clone(),
            generation: member.reference.generation,
            stale: member.stale,
            attribution: attribution(&member),
        };
        rows.entry((group_kind(&member), key))
            .or_insert_with(|| Row {
                members: Vec::new(),
            })
            .members
            .push(member);
    }
    fold_proxies(&mut rows);
    let mut groups: Vec<Group> = Vec::new();
    for ((kind, _), row) in rows {
        match groups.last_mut() {
            Some(group) if group.kind == kind => group.rows.push(row),
            _ => groups.push(Group {
                kind,
                rows: vec![row],
            }),
        }
    }
    groups
}
