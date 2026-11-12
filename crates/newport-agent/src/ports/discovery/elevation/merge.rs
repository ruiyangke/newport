use super::super::{Failure, budget::record};
use newport_protocol::monitor::ports::discovery::{
    Endpoint, HolderState, MAX_HOLDER_REFERENCES, MAX_HOLDERS, MAX_IDENTITIES, Outcome, Process,
    ProcessIdentity, Report, Scope, Snapshot, Socket,
};
use std::collections::BTreeMap;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Key(Endpoint, u64, u32);
fn key(socket: &Socket) -> Key {
    Key(socket.endpoint, socket.inode, socket.uid)
}

fn same_scope(left: &Scope, right: &Scope) -> bool {
    left.network.is_some() && left.processes.is_some() && left.boot_id.is_some() && left == right
}

fn cookies_agree(left: &Socket, right: &Socket) -> bool {
    match (left.cookie, right.cookie) {
        (Some(a), Some(b)) => a == b,
        _ => true,
    }
}

struct Sockets<'a>(BTreeMap<Key, Vec<&'a Socket>>);
impl<'a> Sockets<'a> {
    fn new(snapshot: &'a Snapshot) -> Self {
        let mut values: BTreeMap<Key, Vec<&Socket>> = BTreeMap::new();
        for socket in &snapshot.sockets {
            values.entry(key(socket)).or_default().push(socket);
        }
        Self(values)
    }

    fn matching(&self, socket: &Socket) -> Option<&'a Socket> {
        let candidates = self.0.get(&key(socket))?;
        let mut matching = candidates
            .iter()
            .copied()
            .filter(|candidate| cookies_agree(socket, candidate));
        let first = matching.next()?;
        matching.next().is_none().then_some(first)
    }
}

pub(super) fn needs_revalidation(base: &Snapshot, elevated: &Snapshot) -> bool {
    let other = Sockets::new(elevated);
    base.sockets.iter().any(|socket| {
        other.matching(socket).is_some_and(|extra| {
            !extra.holders.is_empty() && (socket.cookie.is_none() || extra.cookie.is_none())
        })
    })
}

fn confirmed(base: &Socket, extra: &Socket, fresh: Option<(&Snapshot, &Sockets<'_>)>) -> bool {
    if base.cookie.is_some() && extra.cookie.is_some() {
        return true;
    }
    let Some((snapshot, sockets)) = fresh else {
        return false;
    };
    base.inode != 0
        && snapshot
            .coverage
            .socket_inventory_complete(base.endpoint.address)
        && sockets.matching(base).is_some_and(|current| {
            cookies_agree(extra, current)
                && base
                    .cookie
                    .or(extra.cookie)
                    .is_none_or(|cookie| current.cookie == Some(cookie))
        })
}

/// Applies the entire candidate atomically, preserving the base on validation failure.
pub(super) fn apply(
    base: &mut Snapshot,
    elevated: &Snapshot,
    fresh: Option<&Snapshot>,
) -> Result<(), Failure> {
    base.validate()?;
    elevated.validate()?;
    if base.generation != elevated.generation || !same_scope(&base.scope, &elevated.scope) {
        return Err(Failure::Invalid(
            "Elevated discovery returned another scan or namespace",
        ));
    }
    if let Some(fresh) = fresh {
        fresh.validate()?;
    }
    let fresh = fresh.filter(|fresh| {
        fresh.generation == base.generation && same_scope(&base.scope, &fresh.scope)
    });
    let fresh_index = fresh.map(Sockets::new);
    let fresh = fresh.zip(fresh_index.as_ref());
    let extras = Sockets::new(elevated);
    let original = Sockets::new(base);
    let processes = elevated
        .processes
        .iter()
        .map(|process| (process.id, process))
        .collect::<BTreeMap<_, _>>();
    let mut candidate = base.clone();
    let references = candidate
        .sockets
        .iter()
        .map(|socket| socket.holders.len())
        .sum();
    let mut holders = Holders::new(
        &mut candidate.processes,
        references,
        elevated.coverage.processes.clone(),
    );
    for socket in &mut candidate.sockets {
        let Some(extra) = extras
            .matching(socket)
            .filter(|extra| !extra.holders.is_empty())
        else {
            continue;
        };
        if original.matching(socket).is_none() || !confirmed(socket, extra, fresh) {
            record(&mut holders.report, &Failure::Interrupted);
            continue;
        }
        let observed = extra
            .holders
            .iter()
            .filter_map(|id| processes.get(id).copied())
            .map(|process| (process.identity, process))
            .collect::<BTreeMap<_, _>>();
        for process in observed.values() {
            if !holders.add(socket, process) {
                break;
            }
        }
        if extra.holders_truncated {
            socket.holders_truncated = true;
        }
    }
    candidate.coverage.elevation = holders.report;
    if !candidate.processes.is_empty()
        && !matches!(
            candidate.coverage.processes.outcome,
            Outcome::Complete | Outcome::Partial
        )
    {
        record(
            &mut candidate.coverage.processes,
            &Failure::Unavailable("Process identities were supplied by the elevated scan"),
        );
    }
    candidate.validate()?;
    *base = candidate;
    Ok(())
}

struct Holders<'a> {
    processes: &'a mut Vec<Process>,
    identities: BTreeMap<ProcessIdentity, (u32, usize)>,
    references: usize,
    next: u32,
    report: Report,
}

impl<'a> Holders<'a> {
    fn new(processes: &'a mut Vec<Process>, references: usize, report: Report) -> Self {
        let identities = processes
            .iter()
            .enumerate()
            .map(|(index, process)| (process.identity, (process.id, index)))
            .collect();
        let next = processes
            .iter()
            .map(|process| process.id)
            .max()
            .unwrap_or(0);
        Self {
            processes,
            identities,
            references,
            next,
            report,
        }
    }

    fn add(&mut self, socket: &mut Socket, process: &Process) -> bool {
        let existing = self.identities.get(&process.identity).copied();
        if let Some((id, index)) = existing {
            if let Some(current) = self.processes.get_mut(index) {
                enrich_metadata(current, process);
            }
            if socket.holders.contains(&id) {
                return true;
            }
        }
        if socket.holders.len() == MAX_HOLDERS || self.references == MAX_HOLDER_REFERENCES {
            return self.exhausted(socket, "Elevated socket holder limit reached");
        }
        let id = if let Some((id, _)) = existing {
            id
        } else {
            let Some(id) = self
                .next
                .checked_add(1)
                .filter(|_| self.processes.len() < MAX_IDENTITIES)
            else {
                return self.exhausted(socket, "Elevated process identity limit reached");
            };
            self.identities
                .insert(process.identity, (id, self.processes.len()));
            self.processes.push(identity_record(process, id));
            self.next = id;
            id
        };
        socket.holders.push(id);
        socket.holder_state = HolderState::Observed;
        self.references += 1;
        true
    }

    fn exhausted(&mut self, socket: &mut Socket, reason: &'static str) -> bool {
        socket.holders_truncated = true;
        record(&mut self.report, &Failure::Exhausted(reason));
        false
    }
}

fn enrich_metadata(current: &mut Process, extra: &Process) {
    fn compatible<T: Eq>(left: &Option<T>, right: &Option<T>) -> bool {
        match (left, right) {
            (Some(left), Some(right)) => left == right,
            _ => true,
        }
    }
    // Detailed metadata may predate an exec; keep that observation intact.
    if current.details.is_none()
        && compatible(&current.name, &extra.name)
        && compatible(&current.executable_name, &extra.executable_name)
        && compatible(&current.uid, &extra.uid)
        && ((current.name.is_none() && extra.name.is_some())
            || (current.executable_name.is_none() && extra.executable_name.is_some())
            || (current.uid.is_none() && extra.uid.is_some()))
    {
        current.name = current.name.take().or_else(|| extra.name.clone());
        current.executable_name = current
            .executable_name
            .take()
            .or_else(|| extra.executable_name.clone());
        current.uid = current.uid.or(extra.uid);
        current.metadata = extra.metadata.clone();
    }
}

fn identity_record(process: &Process, id: u32) -> Process {
    Process {
        id,
        identity: process.identity,
        name: process.name.clone(),
        executable_name: process.executable_name.clone(),
        uid: process.uid,
        account: None,
        details: None,
        metadata: process.metadata.clone(),
    }
}

#[cfg(test)]
mod tests;
