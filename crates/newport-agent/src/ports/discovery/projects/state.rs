use super::super::helper;
use newport_protocol::monitor::ports::discovery::{Outcome, Report, Snapshot};
use std::time::Instant;

#[derive(Default)]
pub struct State {
    client: helper::Client,
}

impl State {
    pub fn enrich(
        &mut self,
        snapshot: &mut Snapshot,
        deadline: Instant,
        cancelled: &dyn Fn() -> bool,
    ) {
        self.client.poll_cleanup();
        let process_paths = snapshot.processes.iter().any(|process| {
            process
                .details
                .as_ref()
                .and_then(|details| details.directory.as_ref())
                .is_some_and(|path| !path.deleted && !path.lossy)
        });
        let compose_paths = snapshot.publications.iter().any(|publication| {
            publication
                .compose
                .as_ref()
                .is_some_and(|compose| compose.directory.is_some())
        });
        if !process_paths && !compose_paths {
            return;
        }
        let deadline = deadline.min(Instant::now() + crate::ports::project::TIME_LIMIT);
        let result = std::env::current_exe()
            .map_err(helper::Error::from)
            .and_then(|agent| self.client.projects(&agent, snapshot, deadline, cancelled));
        match result {
            Ok(enriched) => *snapshot = enriched,
            Err(error) => {
                let report = helper::report(&error);
                incomplete(&mut snapshot.coverage.details, &report);
                for process in &mut snapshot.processes {
                    if process
                        .details
                        .as_ref()
                        .and_then(|details| details.directory.as_ref())
                        .is_some_and(|path| !path.deleted && !path.lossy)
                    {
                        incomplete(&mut process.metadata, &report);
                    }
                }
            }
        }
    }
}

fn incomplete(target: &mut Report, failure: &Report) {
    target.outcome = Outcome::Partial;
    for issue in &failure.issues {
        if let Some(previous) = target
            .issues
            .iter_mut()
            .find(|value| value.kind == issue.kind)
        {
            previous.count = previous.count.saturating_add(issue.count);
        } else if target.issues.len() < newport_protocol::monitor::ports::discovery::MAX_ISSUES {
            target.issues.push(issue.clone());
        }
    }
}
