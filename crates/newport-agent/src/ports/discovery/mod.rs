//! Bounded observations of sockets and their independently verified attribution.
mod budget;
mod cgroup;
mod diagnostic;
mod elevation;
mod fallback;
mod fit;
pub mod helper;
mod process;
mod projects;
mod scan;
mod session;
mod units;

pub use budget::Failure;
pub use scan::{Discovery, Mode, Scan};
pub(crate) use session::Session;

use newport_protocol::monitor::ports::discovery::{
    MAX_ISSUES, Outcome, Report, Socket, SocketSource,
};
use std::{
    net::IpAddr,
    path::Path,
    time::{Duration, Instant},
};

use budget::Budget;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    Ipv4,
    Ipv6,
}

impl Family {
    fn contains(self, address: IpAddr) -> bool {
        matches!(
            (self, address),
            (Self::Ipv4, IpAddr::V4(_)) | (Self::Ipv6, IpAddr::V6(_))
        )
    }

    fn number(self) -> u8 {
        match self {
            Self::Ipv4 => 2,
            Self::Ipv6 => 10,
        }
    }
}

pub struct Inventory {
    pub source: SocketSource,
    pub sockets: Vec<Socket>,
    pub report: Report,
}

/// Only the caller's current network namespace is queried.
pub fn sockets(family: Family, deadline: Instant, cancelled: &dyn Fn() -> bool) -> Inventory {
    let budget = Budget::new(deadline, cancelled);
    let budget = budget.limited(Duration::from_secs(2));
    let primary = diagnostic::collect(family, &budget);
    if primary.report.outcome == Outcome::Complete || budget.remaining().is_err() {
        return primary;
    }
    choose_inventory(primary, fallback::collect(Path::new("/"), family, &budget))
}

fn choose_inventory(primary: Inventory, fallback: Inventory) -> Inventory {
    let mut issues = primary.report.issues.clone();
    issues.extend(fallback.report.issues.iter().cloned());
    issues.truncate(MAX_ISSUES);
    let mut result = if fallback.report.outcome == Outcome::Complete || !fallback.sockets.is_empty()
    {
        fallback
    } else {
        primary
    };
    result.report.issues = issues;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use newport_protocol::monitor::ports::discovery::{Issue, IssueKind};

    #[test]
    fn successful_fallback_keeps_primary_diagnostics_without_claiming_incomplete_inventory() {
        let primary = Inventory {
            source: SocketSource::Diagnostic,
            sockets: Vec::new(),
            report: Report {
                outcome: Outcome::Unavailable,
                issues: vec![Issue {
                    kind: IssueKind::Unsupported,
                    count: 1,
                    message: "Socket diagnostics unsupported".into(),
                }],
            },
        };
        let fallback = Inventory {
            source: SocketSource::Proc,
            sockets: Vec::new(),
            report: Report::complete(),
        };
        let result = choose_inventory(primary, fallback);
        assert_eq!(result.report.outcome, Outcome::Complete);
        assert_eq!(result.source, SocketSource::Proc);
        assert!(result.sockets.is_empty());
        assert_eq!(result.report.issues[0].kind, IssueKind::Unsupported);
    }
}
