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
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub kind: GroupKind,
    pub rows: Vec<Row>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Attribution {
    Holders(Vec<Process>, Vec<Unit>),
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
            // Different observed holder sets still receive different row keys.
            let holders = socket
                .holders
                .iter()
                .map(|h| h.identity)
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
            .unwrap_or(GroupKind::Other),
        Kind::Socket(_) if member.unit().is_some_and(|unit| unit.manager_uid.is_none()) => {
            GroupKind::System
        }
        Kind::Socket(_) => GroupKind::Other,
    }
}

pub(super) fn shares_attribution(members: &[Member]) -> bool {
    let Some(first) = members.first() else {
        return false;
    };
    let owner = attribution(first);
    if matches!(owner, Attribution::Separate(_)) {
        return false;
    }
    let kind = group_kind(first);
    members.iter().skip(1).all(|member| {
        member.scope == first.scope
            && member.reference.generation == first.reference.generation
            && attribution(member) == owner
            && group_kind(member) == kind
    })
}

/// Merge compatible attribution only within a known scope and one observation
/// generation. Unknown owners and differing holder sets stay separate. Sort by
/// facts, never by input/hash-map order, so refreshes do not change the first row.
pub fn group(mut members: Vec<Member>) -> Vec<Group> {
    members.sort_by(|a, b| {
        a.endpoint
            .cmp(&b.endpoint)
            .then_with(|| a.reference.cmp(&b.reference))
    });
    let mut groups: BTreeMap<GroupKind, BTreeMap<_, Row>> = BTreeMap::new();
    for member in members {
        let key = (
            member.endpoint.port,
            member.scope.clone(),
            member.reference.generation,
            member.stale,
            attribution(&member),
        );
        groups
            .entry(group_kind(&member))
            .or_default()
            .entry(key)
            .or_insert_with(|| Row {
                members: Vec::new(),
            })
            .members
            .push(member);
    }
    groups
        .into_iter()
        .map(|(kind, rows)| Group {
            kind,
            rows: rows.into_values().collect(),
        })
        .collect()
}
