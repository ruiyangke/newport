//! Preserve complete record relationships while fitting the bounded response.
use super::Failure;
use newport_protocol::monitor::ports::discovery::{
    Issue, IssueKind, MAX_ISSUES, Outcome, Report, Snapshot, wire,
};
use std::collections::BTreeSet;

pub(super) fn fit(snapshot: &mut Snapshot) -> Result<(), Failure> {
    snapshot.validate_records()?;
    prune(snapshot);
    if wire::fits_response(snapshot)? {
        return Ok(());
    }
    let mut details = 0;
    for process in &mut snapshot.processes {
        if process.details.take().is_some() {
            omitted(&mut process.metadata, 1);
            details += 1;
        }
    }
    for publication in &mut snapshot.publications {
        if publication
            .compose
            .as_mut()
            .is_some_and(|compose| compose.directory_project.take().is_some())
        {
            details += 1;
        }
    }
    if details > 0 {
        omitted(&mut snapshot.coverage.details, details);
    }
    while !wire::fits_response(snapshot)? {
        if !snapshot.sockets.is_empty() && snapshot.sockets.len() >= snapshot.publications.len() {
            let keep = snapshot.sockets.len() / 2;
            let dropped = &snapshot.sockets[keep..];
            let ipv4 = u64::try_from(
                dropped
                    .iter()
                    .filter(|s| s.endpoint.address.is_ipv4())
                    .count(),
            )
            .map_err(|_| Failure::Exhausted("Discovery omission count limit reached"))?;
            let ipv6 = u64::try_from(dropped.len())
                .map_err(|_| Failure::Exhausted("Discovery omission count limit reached"))?
                - ipv4;
            if ipv4 > 0 {
                omitted(&mut snapshot.coverage.ipv4, ipv4);
            }
            if ipv6 > 0 {
                omitted(&mut snapshot.coverage.ipv6, ipv6);
            }
            snapshot.sockets.truncate(keep);
        } else if !snapshot.publications.is_empty() {
            let keep = snapshot.publications.len() / 2;
            let dropped = u64::try_from(snapshot.publications.len() - keep)
                .map_err(|_| Failure::Exhausted("Discovery omission count limit reached"))?;
            snapshot.publications.truncate(keep);
            omitted(&mut snapshot.coverage.containers, dropped);
        } else {
            return Err(Failure::Invalid(
                "Discovery metadata exceeds the response budget",
            ));
        }
        prune(snapshot);
    }
    snapshot.validate()?;
    Ok(())
}

fn omitted(report: &mut Report, count: u64) {
    report.outcome = Outcome::Partial;
    if let Some(issue) = report
        .issues
        .iter_mut()
        .find(|issue| issue.kind == IssueKind::BudgetExceeded)
    {
        issue.count = issue.count.saturating_add(count);
    } else {
        if report.issues.len() == MAX_ISSUES {
            report.issues.pop();
        }
        report.issues.push(Issue {
            kind: IssueKind::BudgetExceeded,
            count,
            message: "Observations omitted to fit the discovery response budget.".into(),
        });
    }
}

fn prune(snapshot: &mut Snapshot) {
    let processes: BTreeSet<_> = snapshot
        .sockets
        .iter()
        .flat_map(|socket| &socket.holders)
        .copied()
        .collect();
    let services: BTreeSet<_> = snapshot
        .sockets
        .iter()
        .flat_map(|socket| &socket.services)
        .map(|a| a.service)
        .collect();
    snapshot
        .processes
        .retain(|process| processes.contains(&process.id));
    snapshot
        .services
        .retain(|service| services.contains(&service.id));
}

#[cfg(test)]
mod tests;
