//! Finite identity scanning and isolated, unprivileged project enrichment.
use super::{Outcome, Snapshot};
use crate::{common, monitor::Invalid};

#[cfg(feature = "protobuf")]
pub mod wire;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub contract_id: Option<String>,
    pub generation: u64,
    pub projects: Option<Box<Snapshot>>,
}

impl Request {
    pub fn validate(&self) -> Result<(), Invalid> {
        contract_id(self.contract_id.as_deref())?;
        if self.generation == 0 {
            return Err(Invalid("discovery helper generation"));
        }
        if let Some(snapshot) = &self.projects {
            snapshot.validate()?;
            if snapshot.generation != self.generation
                || snapshot.processes.iter().any(|process| {
                    process
                        .details
                        .as_ref()
                        .is_some_and(|details| details.project.is_some())
                })
                || snapshot.publications.iter().any(|publication| {
                    publication
                        .compose
                        .as_ref()
                        .is_some_and(|compose| compose.directory_project.is_some())
                })
            {
                return Err(Invalid("project helper input"));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    pub contract_id: Option<String>,
    pub generation: u64,
    pub result: Result<Box<Snapshot>, common::Error>,
}

impl Reply {
    pub fn validate_for(&self, request: &Request) -> Result<(), Invalid> {
        request.validate()?;
        contract_id(self.contract_id.as_deref())?;
        if self.generation != request.generation {
            return Err(Invalid("discovery helper correlation"));
        }
        match &self.result {
            Ok(snapshot) => {
                if self.contract_id.is_none() || self.contract_id != request.contract_id {
                    return Err(Invalid("discovery helper contract"));
                }
                if snapshot.generation != request.generation {
                    return Err(Invalid("discovery helper snapshot generation"));
                }
                match &request.projects {
                    Some(input) => projects(input, snapshot),
                    None => identity(snapshot),
                }
            }
            Err(error)
                if !error.code.is_empty()
                    && error.code.len() <= 64
                    && !error.code.contains('\0')
                    && !error.message.is_empty()
                    && error.message.len() <= crate::monitor::MAX_ERROR_MESSAGE
                    && !error.message.contains('\0') =>
            {
                Ok(())
            }
            Err(_) => Err(Invalid("discovery helper error")),
        }
    }
}

fn projects(input: &Snapshot, output: &Snapshot) -> Result<(), Invalid> {
    output.validate()?;
    let mut unchanged = output.clone();
    if unchanged.processes.len() != input.processes.len()
        || unchanged.publications.len() != input.publications.len()
    {
        return Err(Invalid("project helper observation count"));
    }
    preserved_report(&input.coverage.details, &output.coverage.details)?;
    unchanged.coverage.details = input.coverage.details.clone();
    for (process, before) in unchanged.processes.iter_mut().zip(&input.processes) {
        preserved_report(&before.metadata, &process.metadata)?;
        process.metadata = before.metadata.clone();
        if let Some(details) = &mut process.details {
            details.project = None;
        }
    }
    for publication in &mut unchanged.publications {
        if let Some(compose) = &mut publication.compose {
            compose.directory_project = None;
        }
    }
    if &unchanged != input {
        return Err(Invalid("project helper changed observed facts"));
    }
    Ok(())
}

fn preserved_report(before: &super::Report, after: &super::Report) -> Result<(), Invalid> {
    if (before.outcome != after.outcome
        && before.outcome != Outcome::NotAttempted
        && after.outcome != Outcome::Partial)
        || before.issues.iter().any(|old| {
            !after.issues.iter().any(|issue| {
                issue.kind == old.kind && issue.message == old.message && issue.count >= old.count
            })
        })
    {
        return Err(Invalid("project helper discarded coverage evidence"));
    }
    Ok(())
}

fn contract_id(value: Option<&str>) -> Result<(), Invalid> {
    if value.is_some_and(|value| !common::valid_contract_id(value)) {
        return Err(Invalid("discovery helper contract identifier"));
    }
    Ok(())
}

fn identity(snapshot: &Snapshot) -> Result<(), Invalid> {
    snapshot.validate()?;
    let coverage = &snapshot.coverage;
    if !snapshot.services.is_empty()
        || !snapshot.publications.is_empty()
        || snapshot.container_engine.is_some()
        || snapshot.sockets.iter().any(|socket| {
            socket.account.is_some()
                || !socket.services.is_empty()
                || socket
                    .cgroup
                    .as_ref()
                    .is_some_and(|group| group.path.is_some())
        })
        || snapshot
            .processes
            .iter()
            .any(|process| process.account.is_some() || process.details.is_some())
        || [
            &coverage.cgroups,
            &coverage.services,
            &coverage.containers,
            &coverage.elevation,
            &coverage.details,
        ]
        .iter()
        .any(|report| {
            !matches!(
                report.outcome,
                Outcome::NotAttempted | Outcome::NotApplicable
            ) || !report.issues.is_empty()
        })
    {
        return Err(Invalid("discovery helper identity scope"));
    }
    Ok(())
}
