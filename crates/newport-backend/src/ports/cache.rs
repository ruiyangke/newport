//! Retains missing observations only when their source cannot establish absence.
use newport_protocol::{
    desktop::connection::discovery::{self as desktop, Discovery},
    monitor::{
        ports::discovery::{self, Endpoint, Issue, IssueKind, Outcome, Report, Snapshot},
        wire,
    },
};
use std::{collections::BTreeSet, error, fmt};

#[derive(Debug)]
pub enum Error {
    Invalid(wire::Error),
    /// A failed first scan or a failed scan after the observed scope changed.
    NoInventory(Box<Snapshot>),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(error) => write!(f, "Invalid port discovery: {error}"),
            Self::NoInventory(_) => f.write_str("No port inventory source was available."),
        }
    }
}

impl error::Error for Error {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            Self::Invalid(error) => Some(error),
            Self::NoInventory(_) => None,
        }
    }
}

impl From<wire::Error> for Error {
    fn from(value: wire::Error) -> Self {
        Self::Invalid(value)
    }
}

#[derive(Default)]
pub struct Cache {
    value: Option<Discovery>,
}

impl Cache {
    pub fn snapshot(&self) -> Option<&Discovery> {
        self.value.as_ref()
    }

    /// Invalid input preserves the cache. Scope changes invalidate its identities.
    pub fn apply(&mut self, current: Snapshot) -> Result<&Discovery, Error> {
        let size = discovery::wire::measure(&current)?;
        let previous = self.value.as_ref().filter(|old| {
            desktop::same_scope(&old.current, &current)
                && old.current.generation < current.generation
        });
        let mut result = Discovery {
            current,
            retained: Vec::new(),
            retention: previous.map_or_else(Report::complete, |old| old.retention.clone()),
        };
        if !result.has_current_inventory() && previous.is_none() {
            self.value = None;
            return Err(Error::NoInventory(Box::new(result.current)));
        }
        if let Some(previous) = previous {
            let mut budget = Budget::new(&result.current, size);
            let identities = Identities::new(&result.current);
            let mut candidates: Vec<_> = std::iter::once(&previous.current)
                .chain(&previous.retained)
                .collect();
            candidates.sort_unstable_by_key(|snapshot| std::cmp::Reverse(snapshot.generation));
            for candidate in candidates {
                let mut retained = candidate.clone();
                prune(&mut retained, &result.current, &identities);
                if retained.sockets.is_empty() && retained.publications.is_empty() {
                    continue;
                }
                let size = discovery::wire::measure(&retained)?;
                if result.retained.len() < desktop::MAX_RETAINED && budget.include(&retained, size)
                {
                    result.retained.push(retained);
                } else {
                    record_loss(&mut result.retention);
                }
            }
        }
        // Commit once, after all source slices and their references have been checked.
        result.validate().map_err(wire::Error::from)?;
        Ok(self.value.insert(result))
    }
}

fn record_loss(report: &mut Report) {
    report.outcome = Outcome::Partial;
    if let Some(issue) = report
        .issues
        .iter_mut()
        .find(|issue| issue.kind == IssueKind::BudgetExceeded)
    {
        issue.count = issue.count.saturating_add(1);
    } else {
        report.issues.push(Issue {
            kind: IssueKind::BudgetExceeded,
            count: 1,
            message: "Older observations exceeded the retention budget in this discovery scope."
                .into(),
        });
    }
}

type SocketKey = (Endpoint, u64, u64, u32);
type PublicationKey<'a> = (
    Option<&'a str>,
    &'a str,
    Endpoint,
    u16,
    Option<std::net::IpAddr>,
);

struct Identities<'a> {
    sockets: BTreeSet<SocketKey>,
    publications: BTreeSet<PublicationKey<'a>>,
}

impl<'a> Identities<'a> {
    fn new(snapshot: &'a Snapshot) -> Self {
        Self {
            sockets: snapshot.sockets.iter().filter_map(socket_key).collect(),
            publications: snapshot.publications.iter().map(publication_key).collect(),
        }
    }
}

fn socket_key(socket: &discovery::Socket) -> Option<SocketKey> {
    socket
        .cookie
        .map(|cookie| (socket.endpoint, socket.inode, cookie, socket.uid))
}

fn publication_key(publication: &discovery::Publication) -> PublicationKey<'_> {
    (
        publication.engine.as_deref(),
        &publication.container,
        publication.host,
        publication.target_port,
        publication.target_address,
    )
}

fn prune(old: &mut Snapshot, current: &Snapshot, identities: &Identities<'_>) {
    old.sockets.retain(|socket| {
        !current
            .coverage
            .socket_inventory_complete(socket.endpoint.address)
            && socket_key(socket).is_none_or(|key| !identities.sockets.contains(&key))
    });
    old.publications.retain(|publication| {
        !publication
            .engine
            .as_deref()
            .is_some_and(|engine| current.publications_complete_for(engine))
            && !identities
                .publications
                .contains(&publication_key(publication))
    });
    let processes: BTreeSet<_> = old
        .sockets
        .iter()
        .flat_map(|socket| &socket.holders)
        .copied()
        .collect();
    let services: BTreeSet<_> = old
        .sockets
        .iter()
        .flat_map(|socket| &socket.services)
        .map(|association| association.service)
        .collect();
    old.processes
        .retain(|process| processes.contains(&process.id));
    old.services
        .retain(|service| services.contains(&service.id));
}

struct Budget {
    rows: [usize; 5],
    bytes: usize,
    fields: usize,
}

fn rows(snapshot: &Snapshot) -> [usize; 5] {
    [
        snapshot.sockets.len(),
        snapshot.processes.len(),
        snapshot.services.len(),
        snapshot.publications.len(),
        snapshot
            .processes
            .iter()
            .filter(|p| p.details.is_some())
            .count(),
    ]
}

impl Budget {
    fn new(snapshot: &Snapshot, size: discovery::wire::Size) -> Self {
        Self {
            rows: rows(snapshot),
            bytes: desktop::ENVELOPE_BYTES + size.bytes + 8,
            fields: desktop::ENVELOPE_FIELDS + size.fields + 1,
        }
    }

    fn include(&mut self, snapshot: &Snapshot, size: discovery::wire::Size) -> bool {
        let mut combined = self.rows;
        for (count, next) in combined.iter_mut().zip(rows(snapshot)) {
            *count += next;
        }
        let bytes = self.bytes + size.bytes + 8;
        let fields = self.fields + size.fields + 1;
        if combined[..4].iter().any(|count| *count > desktop::MAX_ROWS)
            || combined[4] > discovery::MAX_DETAILS * 2
            || bytes > desktop::MAX_BYTES
            || fields > desktop::MAX_FIELDS
        {
            return false;
        }
        self.rows = combined;
        self.bytes = bytes;
        self.fields = fields;
        true
    }
}

#[cfg(test)]
mod tests;
