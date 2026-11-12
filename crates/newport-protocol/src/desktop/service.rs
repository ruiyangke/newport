//! Projection to existing service contracts without JSON or application dependencies.
use super::{containers, services};
use crate::monitor::{self, compose, containers as c, services as s};
impl services::Operation {
    pub fn service_request(&self) -> s::Request {
        match self {
            Self::Snapshot(_) => s::Request::Snapshot,
            Self::Logs(value) => s::Request::Logs(value.request.clone()),
            Self::UnitFile(value) => s::Request::Unit {
                scope: value.scope,
                unit: value.unit.clone(),
            },
            Self::Action(value) => s::Request::Action {
                scope: value.scope,
                unit: value.unit.clone(),
                verb: value.verb,
            },
        }
    }
}
impl services::Reply {
    pub fn service_reply(&self) -> s::Reply {
        match self {
            Self::Snapshot(value) => s::Reply::Snapshot(value.as_ref().clone()),
            Self::Logs(value) => s::Reply::Logs(value.as_ref().clone()),
            Self::UnitFile(value) => s::Reply::Unit(value.as_ref().clone()),
            Self::Action(value) => s::Reply::Action(value.as_ref().clone()),
        }
    }
}
impl containers::Operation {
    pub fn service_request(&self) -> monitor::Request {
        let request = match self {
            Self::Snapshot(_) => c::Request::Snapshot,
            Self::Inspect(value) => c::Request::Inspect {
                container: value.container.clone(),
            },
            Self::Logs(value) => c::Request::Logs(value.request.clone()),
            Self::Action(value) => c::Request::Action {
                container: value.container.clone(),
                verb: value.verb,
            },
            Self::ProjectAction(value) => c::Request::ProjectAction {
                project: value.project.clone(),
                verb: value.verb,
                expected_ids: value.expected_ids.clone(),
            },
            Self::Remove(value) => c::Request::Remove {
                containers: value.containers.clone(),
            },
            Self::Disk(value) => c::Request::Disk {
                part: match value.part {
                    containers::DiskPart::Full => None,
                    containers::DiskPart::Quick => Some(c::DiskPart::Quick),
                    containers::DiskPart::Volumes => Some(c::DiskPart::Volumes),
                },
            },
            Self::Prune(value) => c::Request::Prune { what: value.what },
            Self::ComposeProject(value) => {
                return monitor::Request::Compose(compose::Request::Project {
                    project: value.project.clone(),
                });
            }
            Self::ComposeRun(value) => {
                return monitor::Request::Compose(compose::Request::Run {
                    project: value.project.clone(),
                    action: value.action.clone(),
                });
            }
            Self::ComposeJob(value) => {
                return monitor::Request::Compose(compose::Request::Job {
                    job: value.job.clone(),
                    offset: value.offset,
                });
            }
            Self::ComposeCancel(value) => {
                return monitor::Request::Compose(compose::Request::Cancel {
                    job: value.job.clone(),
                });
            }
            Self::ComposeJobs(_) => return monitor::Request::Compose(compose::Request::Jobs),
        };
        monitor::Request::Containers(request)
    }
}
impl containers::Reply {
    pub fn service_reply(&self) -> monitor::Reply {
        let reply = match self {
            Self::Snapshot(value) => c::Reply::Snapshot(value.as_ref().clone()),
            Self::Inspect(value) => c::Reply::Inspect(value.clone()),
            Self::Logs(value) => c::Reply::Logs(value.as_ref().clone()),
            Self::Action(value) => c::Reply::Action(value.as_ref().clone()),
            Self::ProjectAction(value) => c::Reply::ProjectAction(value.as_ref().clone()),
            Self::Remove(value) => c::Reply::Removed(value.as_ref().clone()),
            Self::Disk(value) => c::Reply::Disk(value.as_ref().clone()),
            Self::Prune(value) => c::Reply::Pruned(value.as_ref().clone()),
            Self::ComposeProject(value) => {
                return monitor::Reply::Compose(compose::Reply::Project(value.as_ref().clone()));
            }
            Self::ComposeRun(value) => {
                return monitor::Reply::Compose(compose::Reply::Started(value.as_ref().clone()));
            }
            Self::ComposeJob(value) => {
                return monitor::Reply::Compose(compose::Reply::Job(value.as_ref().clone()));
            }
            Self::ComposeCancel(value) => {
                return monitor::Reply::Compose(compose::Reply::Cancelled(value.as_ref().clone()));
            }
            Self::ComposeJobs(value) => {
                return monitor::Reply::Compose(compose::Reply::Jobs(value.as_ref().clone()));
            }
        };
        monitor::Reply::Containers(reply)
    }
}
