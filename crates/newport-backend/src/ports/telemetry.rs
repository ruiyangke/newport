//! Redacted, bounded diagnostics. Only local identifiers, enums and counts reach logs.
use newport_protocol::monitor::ports::discovery::{
    Coverage, IssueKind, Outcome, Report, Scope, Snapshot, SocketSource,
};
use std::fmt;

const STAGES: [&str; 8] = [
    "ipv4",
    "ipv6",
    "processes",
    "cgroups",
    "services",
    "containers",
    "elevation",
    "details",
];
const ISSUES: [&str; 8] = [
    "permission_denied",
    "unsupported",
    "timed_out",
    "interrupted",
    "budget_exceeded",
    "io",
    "invalid_response",
    "unavailable",
];

fn index(kind: IssueKind) -> usize {
    match kind {
        IssueKind::PermissionDenied => 0,
        IssueKind::Unsupported => 1,
        IssueKind::TimedOut => 2,
        IssueKind::Interrupted => 3,
        IssueKind::BudgetExceeded => 4,
        IssueKind::Io => 5,
        IssueKind::InvalidResponse => 6,
        IssueKind::Unavailable => 7,
    }
}

fn reports(coverage: &Coverage) -> [&Report; 8] {
    [
        &coverage.ipv4,
        &coverage.ipv6,
        &coverage.processes,
        &coverage.cgroups,
        &coverage.services,
        &coverage.containers,
        &coverage.elevation,
        &coverage.details,
    ]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Condition {
    outcome: Outcome,
    causes: [bool; 8],
}

impl Condition {
    fn from_report(report: &Report) -> Self {
        let mut causes = [false; 8];
        for issue in &report.issues {
            causes[index(issue.kind)] = true;
        }
        Self {
            outcome: report.outcome,
            causes,
        }
    }

    fn limited(self) -> bool {
        matches!(self.outcome, Outcome::Partial | Outcome::Unavailable)
            || self.causes.contains(&true)
    }
}

impl fmt::Display for Condition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "outcome={:?} causes=", self.outcome)?;
        let mut first = true;
        for (present, name) in self.causes.iter().zip(ISSUES) {
            if *present {
                if !first {
                    f.write_str("|")?;
                }
                f.write_str(name)?;
                first = false;
            }
        }
        if first {
            f.write_str("none")?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Failure {
    Query,
    InvalidSnapshot,
    NoInventory,
}

#[derive(Debug, PartialEq, Eq)]
enum Transition {
    Stage {
        stage: &'static str,
        condition: Condition,
    },
    QueryRecovered,
}

#[derive(Default)]
pub(super) struct Telemetry {
    scope: Option<Scope>,
    generation: u64,
    conditions: [Option<Condition>; 8],
    failure: Option<Failure>,
}

impl Telemetry {
    fn transitions(&mut self, snapshot: &Snapshot) -> Vec<Transition> {
        if self.scope.as_ref() != Some(&snapshot.scope) || snapshot.generation <= self.generation {
            self.conditions = [None; 8];
        }
        self.scope = Some(snapshot.scope.clone());
        self.generation = snapshot.generation;
        let mut events = Vec::new();
        for (position, report) in reports(&snapshot.coverage).into_iter().enumerate() {
            // Skipping a stage does not establish recovery from its last failure.
            if report.outcome == Outcome::NotAttempted {
                continue;
            }
            let current = Condition::from_report(report);
            let previous = self.conditions[position].replace(current);
            if previous != Some(current)
                && (current.limited() || previous.is_some_and(Condition::limited))
            {
                events.push(Transition::Stage {
                    stage: STAGES[position],
                    condition: current,
                });
            }
        }
        events
    }

    pub fn scan(&mut self, server: uuid::Uuid, snapshot: &Snapshot, refresh: bool) {
        tracing::info!(target: "newport::discovery", %server, "event=discovery_scan {}", Summary::new(snapshot, refresh));
        for event in self.transitions(snapshot) {
            tracing::info!(target: "newport::discovery", %server, "event=discovery_transition {event}");
        }
    }

    pub fn succeeded(&mut self, server: uuid::Uuid) {
        if self.failure.take().is_some() {
            tracing::info!(target: "newport::discovery", %server, "event=discovery_transition {}", Transition::QueryRecovered);
        }
    }

    pub fn failed(&mut self, server: uuid::Uuid, failure: Failure) {
        if self.replace_failure(failure) {
            tracing::warn!(target: "newport::discovery", %server, "event=discovery_query_failed kind={failure:?}");
        }
    }

    fn replace_failure(&mut self, failure: Failure) -> bool {
        self.failure.replace(failure) != Some(failure)
    }
}

impl fmt::Display for Transition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stage { stage, condition } => {
                write!(f, "kind=stage_changed stage={stage} {condition}")
            }
            Self::QueryRecovered => f.write_str("kind=query_recovered"),
        }
    }
}

struct StageSummary {
    outcome: Outcome,
    counts: [u64; 8],
}

struct Summary {
    generation: u64,
    refresh: bool,
    elapsed_ms: u64,
    sockets: usize,
    processes: usize,
    holder_references: usize,
    unresolved: usize,
    omitted_details: usize,
    publications: usize,
    ipv4_source: Option<SocketSource>,
    ipv6_source: Option<SocketSource>,
    stages: [StageSummary; 8],
}

impl Summary {
    fn new(snapshot: &Snapshot, refresh: bool) -> Self {
        Self {
            generation: snapshot.generation,
            refresh,
            elapsed_ms: snapshot.elapsed_ms,
            sockets: snapshot.sockets.len(),
            processes: snapshot.processes.len(),
            holder_references: snapshot
                .sockets
                .iter()
                .map(|socket| socket.holders.len())
                .sum(),
            unresolved: snapshot
                .sockets
                .iter()
                .filter(|socket| socket.holders.is_empty())
                .count(),
            omitted_details: snapshot
                .processes
                .iter()
                .filter(|process| process.details.is_none())
                .count(),
            publications: snapshot.publications.len(),
            ipv4_source: snapshot.diagnostics.ipv4_source,
            ipv6_source: snapshot.diagnostics.ipv6_source,
            stages: reports(&snapshot.coverage).map(|report| {
                let mut counts = [0u64; 8];
                for issue in &report.issues {
                    let count = &mut counts[index(issue.kind)];
                    *count = count.saturating_add(issue.count);
                }
                StageSummary {
                    outcome: report.outcome,
                    counts,
                }
            }),
        }
    }
}

impl fmt::Display for Summary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "generation={} refresh={} elapsed_ms={} sockets={} processes={} holder_references={} unresolved_sockets={} omitted_details={} publications={} ipv4_source={:?} ipv6_source={:?}",
            self.generation,
            self.refresh,
            self.elapsed_ms,
            self.sockets,
            self.processes,
            self.holder_references,
            self.unresolved,
            self.omitted_details,
            self.publications,
            self.ipv4_source,
            self.ipv6_source
        )?;
        for (name, stage) in STAGES.into_iter().zip(&self.stages) {
            write!(f, " {name}_coverage={:?}", stage.outcome)?;
            for (kind, count) in ISSUES.into_iter().zip(stage.counts) {
                if count != 0 {
                    write!(f, " {name}_{kind}_issues={count}")?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
