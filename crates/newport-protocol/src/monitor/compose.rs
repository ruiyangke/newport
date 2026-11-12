//! Compose project discovery and bounded background command observations.
mod records;
mod validate;
use super::{Invalid, containers::valid_project};
pub use records::*;
use serde::{Deserialize, Serialize};

pub const MAX_SERVICES: usize = 50;
pub const MAX_SERVICE_NAME: usize = 64;
pub const MAX_REPLICAS: u32 = 20;
pub const MAX_JOB_ID: usize = 64;
pub const MAX_FILES: usize = 20;
pub const MAX_FILE_TEXT: usize = 256 * 1024;
pub const MAX_JOBS: usize = 64;
pub const MAX_OUTPUT: usize = 1024 * 1024;
pub const MAX_ITEMS: usize = 4096;
pub const MAX_TEXT: usize = 16 * 1024;
pub const MAX_REPLY_TEXT: usize = 3 * 1024 * 1024;

pub fn valid_service(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_SERVICE_NAME
        && name.as_bytes()[0].is_ascii_alphanumeric()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}
pub fn valid_job(job: &str) -> bool {
    !job.is_empty()
        && job.len() <= MAX_JOB_ID
        && job.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InvalidRequest {
    Project(String),
    Job(String),
    Service(String),
    Services,
    Replicas,
}
impl std::fmt::Display for InvalidRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Project(v) => write!(f, "Not a valid Compose project: {v}"),
            Self::Job(v) => write!(f, "Not a valid Compose command: {v}"),
            Self::Service(v) => write!(f, "Not a valid Compose service: {v}"),
            Self::Services => write!(f, "Choose at most {MAX_SERVICES} services."),
            Self::Replicas => write!(f, "Replicas must be 0 to {MAX_REPLICAS}."),
        }
    }
}
impl std::error::Error for InvalidRequest {}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum Action {
    Up {
        #[serde(default)]
        services: Vec<String>,
        #[serde(default)]
        build: bool,
        #[serde(default)]
        pull: bool,
        #[serde(default)]
        force_recreate: bool,
    },
    Down {
        #[serde(default)]
        volumes: bool,
        #[serde(default)]
        remove_orphans: bool,
    },
    Pull {
        #[serde(default)]
        services: Vec<String>,
    },
    Build {
        #[serde(default)]
        services: Vec<String>,
        #[serde(default)]
        no_cache: bool,
    },
    Scale {
        service: String,
        replicas: u32,
    },
}

impl Action {
    pub fn validate(&self) -> Result<(), InvalidRequest> {
        let service = |v: &String| {
            if valid_service(v) {
                Ok(())
            } else {
                Err(InvalidRequest::Service(v.clone()))
            }
        };
        match self {
            Self::Up { services, .. } | Self::Pull { services } | Self::Build { services, .. } => {
                if services.len() > MAX_SERVICES {
                    return Err(InvalidRequest::Services);
                }
                services.iter().try_for_each(service)
            }
            Self::Down { .. } => Ok(()),
            Self::Scale {
                service: v,
                replicas,
            } => {
                service(v)?;
                if *replicas > MAX_REPLICAS {
                    Err(InvalidRequest::Replicas)
                } else {
                    Ok(())
                }
            }
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "op")]
pub enum Request {
    #[serde(rename = "compose_project")]
    Project { project: String },
    #[serde(rename = "compose_run")]
    Run { project: String, action: Action },
    #[serde(rename = "compose_job")]
    Job {
        job: String,
        #[serde(default)]
        offset: u64,
    },
    #[serde(rename = "compose_cancel")]
    Cancel { job: String },
    #[serde(rename = "compose_jobs")]
    Jobs,
}
impl Request {
    pub fn validate(&self) -> Result<(), InvalidRequest> {
        match self {
            Self::Project { project } | Self::Run { project, .. } => {
                if !valid_project(project) {
                    return Err(InvalidRequest::Project(project.clone()));
                }
            }
            Self::Job { job, .. } | Self::Cancel { job } => {
                if !valid_job(job) {
                    return Err(InvalidRequest::Job(job.clone()));
                }
            }
            Self::Jobs => {}
        }
        if let Self::Run { action, .. } = self {
            action.validate()?;
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum Reply {
    Project(Project),
    Started(Started),
    Job(Job),
    Cancelled(Done),
    Jobs(Jobs),
}
impl Reply {
    pub fn validate_for(&self, request: &Request) -> Result<(), Invalid> {
        match (self, request) {
            (Self::Project(v), Request::Project { .. }) => v.validate(),
            (Self::Started(v), Request::Run { .. }) if valid_job(&v.job) => Ok(()),
            (Self::Job(v), Request::Job { job, .. }) if &v.job == job => v.validate(),
            (Self::Cancelled(v), Request::Cancel { .. }) if v.ok => Ok(()),
            (Self::Jobs(v), Request::Jobs) => v.validate(),
            _ => Err(Invalid("Compose result operation")),
        }
    }
}
#[cfg(test)]
mod tests;
