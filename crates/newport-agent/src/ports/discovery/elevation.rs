//! Optional process enrichment preserves the base socket inventory. One root
//! helper serves a monitor session's scans; it starts once, and after sudo
//! refuses it, again only when the user asks to check again.
mod merge;

use super::{Failure, Family, helper, scan, sockets};
use newport_protocol::monitor::ports::discovery::{HolderState, Outcome, Report, Scope, Snapshot};
use std::time::Instant;

#[derive(Default)]
enum Helper {
    #[default]
    Idle,
    Running(helper::Elevated),
    /// A helper that worked stopped; the next scan starts another.
    Lost,
    /// sudo refused it, or a restart failed: tried again only on request.
    Stopped(Report),
}

#[derive(Default)]
pub(super) struct State {
    helper: Helper,
    manual: bool,
    scope: Option<Scope>,
}

impl State {
    pub fn refresh(&mut self) {
        self.manual = true;
    }

    pub fn enrich(
        &mut self,
        snapshot: &mut Snapshot,
        deadline: Instant,
        cancelled: &dyn Fn() -> bool,
    ) {
        if self.scope.as_ref() != Some(&snapshot.scope) {
            self.scope = Some(snapshot.scope.clone());
            self.helper = Helper::Idle;
        }
        // Root has already inspected descriptors directly; it never starts sudo.
        if crate::paths::effective_uid() == 0 || !needed(snapshot) {
            snapshot.coverage.elevation = Report::not_applicable();
            return;
        }
        if snapshot.scope.network.is_none()
            || snapshot.scope.processes.is_none()
            || snapshot.scope.boot_id.is_none()
        {
            snapshot.coverage.elevation =
                Failure::Unavailable("Discovery namespace identity is unavailable").report(false);
            return;
        }
        if cancelled() || Instant::now() >= deadline {
            return;
        }
        let (result, answered) = match self.running() {
            Ok(elevated) => {
                let result = elevated.scan(snapshot.generation, deadline, cancelled);
                (result, elevated.answered())
            }
            Err(report) => {
                snapshot.coverage.elevation = report;
                return;
            }
        };
        match result {
            Ok(extra) => {
                let fresh = merge::needs_revalidation(snapshot, &extra)
                    .then(|| refresh(snapshot, deadline, cancelled));
                if let Err(error) = merge::apply(snapshot, &extra, fresh.as_ref()) {
                    snapshot.coverage.elevation = error.report(false);
                }
            }
            Err(error) => {
                let report = helper::report(&error);
                snapshot.coverage.elevation = report.clone();
                self.helper = if answered {
                    Helper::Lost
                } else {
                    Helper::Stopped(report)
                };
            }
        }
    }

    /// The running helper, starting one unless it was refused and the user
    /// hasn't asked to check again.
    fn running(&mut self) -> Result<&mut helper::Elevated, Report> {
        let manual = std::mem::take(&mut self.manual);
        if let Helper::Stopped(report) = &self.helper
            && !manual
        {
            return Err(report.clone());
        }
        if !matches!(self.helper, Helper::Running(_)) {
            let started = std::env::current_exe()
                .map_err(helper::Error::from)
                .and_then(|agent| helper::Elevated::start(&agent));
            self.helper = match started {
                Ok(elevated) => Helper::Running(elevated),
                Err(error) => Helper::Stopped(helper::report(&error)),
            };
        }
        match &mut self.helper {
            Helper::Running(elevated) => Ok(elevated),
            Helper::Stopped(report) => Err(report.clone()),
            Helper::Idle | Helper::Lost => unreachable!("a helper was just started"),
        }
    }
}

fn needed(snapshot: &Snapshot) -> bool {
    !snapshot.sockets.is_empty()
        && (snapshot.coverage.processes.outcome != Outcome::Complete
            || snapshot.sockets.iter().any(|socket| {
                socket.holder_state != HolderState::Observed || socket.holders_truncated
            }))
}

fn refresh(base: &Snapshot, deadline: Instant, cancelled: &dyn Fn() -> bool) -> Snapshot {
    let mut fresh = Snapshot {
        generation: base.generation,
        scope: scan::scope(std::path::Path::new("/")),
        ..Default::default()
    };
    for family in [Family::Ipv4, Family::Ipv6] {
        let mut current = sockets(family, deadline, cancelled);
        let available =
            newport_protocol::monitor::ports::MAX_LISTENERS.saturating_sub(fresh.sockets.len());
        if current.sockets.len() > available {
            current.sockets.truncate(available);
            super::budget::record(
                &mut current.report,
                &Failure::Exhausted("Socket revalidation inventory limit reached"),
            );
        }
        match family {
            Family::Ipv4 => fresh.coverage.ipv4 = current.report,
            Family::Ipv6 => fresh.coverage.ipv6 = current.report,
        }
        for mut socket in current.sockets {
            socket.id = u32::try_from(fresh.sockets.len() + 1).unwrap_or(u32::MAX);
            fresh.sockets.push(socket);
        }
    }
    if fresh.scope != scan::scope(std::path::Path::new("/")) {
        fresh.scope = Scope::default();
    }
    fresh
}
