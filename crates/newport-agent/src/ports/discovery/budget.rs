use newport_protocol::monitor::ports::discovery::{Issue, IssueKind, Outcome, Report};
use std::{
    fmt, io,
    time::{Duration, Instant},
};

pub struct Budget<'a> {
    deadline: Instant,
    cancelled: &'a dyn Fn() -> bool,
}

impl<'a> Budget<'a> {
    pub fn new(deadline: Instant, cancelled: &'a dyn Fn() -> bool) -> Self {
        Self {
            deadline,
            cancelled,
        }
    }

    pub fn remaining(&self) -> Result<Duration, Failure> {
        if (self.cancelled)() {
            return Err(Failure::Cancelled);
        }
        self.deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or(Failure::TimedOut)
    }

    pub fn limited(&self, limit: Duration) -> Self {
        Self {
            deadline: self.deadline.min(Instant::now() + limit),
            cancelled: self.cancelled,
        }
    }

    pub fn is_cancelled(&self) -> bool {
        (self.cancelled)()
    }
}

#[derive(Debug)]
pub enum Failure {
    Io(io::Error),
    Decode(netlink_packet_core::DecodeError),
    Contract(newport_protocol::monitor::Invalid),
    Wire(newport_protocol::monitor::wire::Error),
    Invalid(&'static str),
    Unsupported(&'static str),
    Unavailable(&'static str),
    Exhausted(&'static str),
    Interrupted,
    TimedOut,
    Cancelled,
}

impl Failure {
    pub fn kind(&self) -> IssueKind {
        match self {
            Self::Io(error) => match error.raw_os_error() {
                Some(libc::EACCES | libc::EPERM) => IssueKind::PermissionDenied,
                Some(libc::EAFNOSUPPORT | libc::EPROTONOSUPPORT | libc::EOPNOTSUPP) => {
                    IssueKind::Unsupported
                }
                Some(libc::ENOBUFS | libc::EINTR) => IssueKind::Interrupted,
                _ => match error.kind() {
                    io::ErrorKind::TimedOut => IssueKind::TimedOut,
                    io::ErrorKind::Interrupted => IssueKind::Interrupted,
                    io::ErrorKind::PermissionDenied => IssueKind::PermissionDenied,
                    io::ErrorKind::Unsupported => IssueKind::Unsupported,
                    io::ErrorKind::FileTooLarge => IssueKind::BudgetExceeded,
                    io::ErrorKind::InvalidData => IssueKind::InvalidResponse,
                    _ => IssueKind::Io,
                },
            },
            Self::Decode(_) | Self::Invalid(_) | Self::Contract(_) | Self::Wire(_) => {
                IssueKind::InvalidResponse
            }
            Self::Exhausted(_) => IssueKind::BudgetExceeded,
            Self::Unsupported(_) => IssueKind::Unsupported,
            Self::Unavailable(_) => IssueKind::Unavailable,
            Self::Interrupted | Self::Cancelled => IssueKind::Interrupted,
            Self::TimedOut => IssueKind::TimedOut,
        }
    }

    pub fn report(&self, observed: bool) -> Report {
        let mut message = self.to_string();
        super::super::truncate(
            &mut message,
            newport_protocol::monitor::ports::discovery::MAX_DIAGNOSTIC,
        );
        Report {
            outcome: if observed {
                Outcome::Partial
            } else {
                Outcome::Unavailable
            },
            issues: vec![Issue {
                kind: self.kind(),
                count: 1,
                message,
            }],
        }
    }

    pub fn retryable(&self) -> bool {
        matches!(self, Self::Interrupted)
            || matches!(self, Self::Io(error) if error.raw_os_error() == Some(libc::ENOBUFS))
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "Port discovery: {error}"),
            Self::Decode(error) => write!(f, "Invalid socket diagnostic response: {error}"),
            Self::Contract(error) => error.fmt(f),
            Self::Wire(error) => error.fmt(f),
            Self::Invalid(reason)
            | Self::Exhausted(reason)
            | Self::Unsupported(reason)
            | Self::Unavailable(reason) => f.write_str(reason),
            Self::Interrupted => {
                f.write_str("Socket inventory changed or lost records during enumeration")
            }
            Self::TimedOut => f.write_str("Discovery time limit reached"),
            Self::Cancelled => f.write_str("Discovery was cancelled"),
        }
    }
}

impl std::error::Error for Failure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Decode(error) => Some(error),
            Self::Contract(error) => Some(error),
            Self::Wire(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for Failure {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<netlink_packet_core::DecodeError> for Failure {
    fn from(value: netlink_packet_core::DecodeError) -> Self {
        Self::Decode(value)
    }
}

impl From<newport_protocol::monitor::Invalid> for Failure {
    fn from(value: newport_protocol::monitor::Invalid) -> Self {
        Self::Contract(value)
    }
}

impl From<newport_protocol::monitor::wire::Error> for Failure {
    fn from(value: newport_protocol::monitor::wire::Error) -> Self {
        Self::Wire(value)
    }
}

pub(super) fn record(report: &mut Report, failure: &Failure) {
    report.outcome = Outcome::Partial;
    let kind = failure.kind();
    if let Some(issue) = report.issues.iter_mut().find(|issue| issue.kind == kind) {
        issue.count = issue.count.saturating_add(1);
    } else if report.issues.len() < newport_protocol::monitor::ports::discovery::MAX_ISSUES {
        report.issues.extend(failure.report(true).issues);
    }
}
