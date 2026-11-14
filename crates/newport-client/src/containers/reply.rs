//! Picks the reply each Containers read expects out of the agent's typed reply.
use newport_protocol::{
    desktop::containers::Reply,
    monitor::{compose, containers as wire},
};

const UNEXPECTED: &str = "Unexpected Containers response operation.";

pub(super) fn snapshot(reply: Reply) -> Result<wire::ContainersSnapshot, String> {
    match reply {
        Reply::Snapshot(value) => Ok(*value),
        _ => Err(UNEXPECTED.into()),
    }
}
pub(super) fn inspect(reply: Reply) -> Result<wire::ContainerInspect, String> {
    match reply {
        Reply::Inspect(value) => Ok(*value),
        _ => Err(UNEXPECTED.into()),
    }
}
pub(super) fn logs(reply: Reply) -> Result<wire::ContainerLogPage, String> {
    match reply {
        Reply::Logs(value) => Ok(*value),
        _ => Err(UNEXPECTED.into()),
    }
}
pub(super) fn disk(reply: Reply) -> Result<wire::DockerDisk, String> {
    match reply {
        Reply::Disk(value) => Ok(*value),
        _ => Err(UNEXPECTED.into()),
    }
}
pub(super) fn compose_project(reply: Reply) -> Result<compose::Project, String> {
    match reply {
        Reply::ComposeProject(value) => Ok(*value),
        _ => Err(UNEXPECTED.into()),
    }
}
pub(super) fn compose_jobs(reply: Reply) -> Result<compose::Jobs, String> {
    match reply {
        Reply::ComposeJobs(value) => Ok(*value),
        _ => Err(UNEXPECTED.into()),
    }
}
pub(super) fn compose_job(reply: Reply) -> Result<compose::Job, String> {
    match reply {
        Reply::ComposeJob(value) => Ok(*value),
        _ => Err(UNEXPECTED.into()),
    }
}
pub(super) fn compose_started(reply: Reply) -> Result<compose::Started, String> {
    match reply {
        Reply::ComposeRun(value) => Ok(*value),
        _ => Err(UNEXPECTED.into()),
    }
}
pub(super) fn compose_done(reply: Reply) -> Result<compose::Done, String> {
    match reply {
        Reply::ComposeCancel(value) => Ok(*value),
        _ => Err(UNEXPECTED.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_rejects_another_operations_reply() {
        let pruned = || {
            Reply::Prune(Box::new(wire::Pruned {
                removed: 1,
                reclaimed: u64::MAX,
            }))
        };
        assert_eq!(snapshot(pruned()).unwrap_err(), UNEXPECTED);
        assert_eq!(compose_job(pruned()).unwrap_err(), UNEXPECTED);
        let page = wire::ContainerLogPage {
            cursor: Some("cursor".into()),
            ..Default::default()
        };
        assert_eq!(logs(Reply::Logs(Box::new(page.clone()))).unwrap(), page);
    }
}
