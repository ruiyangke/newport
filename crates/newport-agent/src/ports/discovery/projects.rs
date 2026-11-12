use super::{
    budget::{Budget, Failure, record},
    process,
};
use crate::ports::project::{Resolved, Resolver};
use newport_protocol::monitor::ports::discovery::{Outcome, Snapshot};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    time::Instant,
};

const COMPOSE_DIRECTORIES: usize = 64;

mod state;
pub(super) use state::State;

pub(super) fn enrich(
    root: &Path,
    snapshot: &mut Snapshot,
    deadline: Instant,
    cancelled: &dyn Fn() -> bool,
) {
    let budget = Budget::new(deadline, cancelled);
    let mut resolver = Resolver::new(root, deadline, cancelled);
    for process in &mut snapshot.processes {
        let Some(details) = &mut process.details else {
            continue;
        };
        let Some(directory) = &details.directory else {
            continue;
        };
        if directory.deleted || directory.lossy {
            continue;
        }
        let result = verified(root, process.identity, directory, &mut resolver, &budget);
        match result {
            Ok(resolved) => {
                details.project = resolved.project;
                if let Some(error) = resolved.limitation {
                    let failure = Failure::Io(error);
                    record(&mut process.metadata, &failure);
                    record(&mut snapshot.coverage.details, &failure);
                }
            }
            Err(error) => {
                record(&mut process.metadata, &error);
                record(&mut snapshot.coverage.details, &error);
            }
        }
    }
    let directories: BTreeSet<_> = snapshot
        .publications
        .iter()
        .filter_map(|publication| publication.compose.as_ref()?.directory.as_deref())
        .collect();
    let mut found = BTreeMap::new();
    for (index, dir) in directories.into_iter().enumerate() {
        if index == COMPOSE_DIRECTORIES {
            record(
                &mut snapshot.coverage.details,
                &Failure::Exhausted("Compose project directory limit reached"),
            );
            break;
        }
        match resolver.resolve(dir) {
            Ok(resolved) => {
                if let Some(error) = resolved.limitation {
                    record(&mut snapshot.coverage.details, &error.into());
                }
                found.insert(dir.to_owned(), resolved.project);
            }
            Err(error) => record(&mut snapshot.coverage.details, &error.into()),
        }
    }
    for compose in snapshot
        .publications
        .iter_mut()
        .filter_map(|publication| publication.compose.as_mut())
    {
        compose.directory_project = compose
            .directory
            .as_ref()
            .and_then(|dir| found.get(dir))
            .cloned()
            .flatten();
    }
    if snapshot.coverage.details.outcome == Outcome::NotAttempted {
        snapshot.coverage.details = newport_protocol::monitor::ports::discovery::Report::complete();
    }
}

fn verified(
    root: &Path,
    identity: newport_protocol::monitor::ports::discovery::ProcessIdentity,
    directory: &newport_protocol::monitor::ports::discovery::ObservedPath,
    resolver: &mut Resolver<'_>,
    budget: &Budget<'_>,
) -> Result<Resolved, Failure> {
    if !process::verify_directory(root, identity, directory, budget)? {
        return Err(Failure::Unavailable(
            "Process directory changed or uses another filesystem view",
        ));
    }
    let result = resolver.resolve(&directory.value)?;
    if !process::verify_directory(root, identity, directory, budget)? {
        return Err(Failure::Unavailable(
            "Process directory changed during project inspection",
        ));
    }
    Ok(result)
}

#[cfg(test)]
mod tests;
