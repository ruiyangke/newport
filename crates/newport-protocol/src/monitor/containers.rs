//! Container inventory, inspection, logs and engine lifecycle operations.
mod records;
mod validate;
pub use records::*;

use super::Invalid;
use serde::{Deserialize, Serialize};

pub const MAX_CONTAINERS: usize = 500;
pub const MAX_IDS: usize = 500;
pub const MAX_LOG_ENTRIES: usize = 2000;
pub const MAX_CURSOR: usize = 4096;
pub const MAX_DISK_ROWS: usize = 2000;
pub const MAX_ITEMS: usize = 4096;
pub const MAX_TEXT: usize = 64 * 1024;
pub const MAX_REQUEST_TEXT: usize = 48 * 1024;
pub const MAX_REPLY_TEXT: usize = 3 * 1024 * 1024;

pub fn valid_id(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}
pub fn valid_container(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}
pub fn valid_project(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-".contains(&b))
}
pub fn valid_cursor(value: &str) -> bool {
    value.len() <= MAX_CURSOR
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".,_-".contains(&b))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InvalidRequest {
    Container(String),
    Project(String),
    Id(String),
    Cursor,
    Lines,
    Count,
    Text,
}
impl std::fmt::Display for InvalidRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Container(value) => write!(f, "Not a valid container: {value}"),
            Self::Project(value) => write!(f, "Not a valid Compose project: {value}"),
            Self::Id(value) => write!(f, "Not a valid container id: {value}"),
            Self::Cursor => f.write_str("Invalid log cursor."),
            Self::Lines => f.write_str("Container log line count is outside the supported range."),
            Self::Count => write!(f, "Choose 1 to {MAX_IDS} containers."),
            Self::Text => f.write_str("Container request exceeds the text limit."),
        }
    }
}
impl std::error::Error for InvalidRequest {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogTarget {
    Container(String),
    Project(String),
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogsRequest {
    pub target: LogTarget,
    pub cursor: Option<String>,
    pub lines: Option<u32>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiskPart {
    Quick,
    Volumes,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op")]
pub enum Request {
    #[serde(rename = "containers")]
    Snapshot,
    #[serde(rename = "container_inspect")]
    Inspect { container: String },
    #[serde(rename = "container_logs")]
    Logs(LogsRequest),
    #[serde(rename = "container_action")]
    Action {
        container: String,
        verb: ContainerVerb,
    },
    #[serde(rename = "project_action")]
    ProjectAction {
        project: String,
        verb: ProjectVerb,
        #[serde(rename = "expectedIds")]
        expected_ids: Vec<String>,
    },
    #[serde(rename = "containers_remove")]
    Remove { containers: Vec<String> },
    #[serde(rename = "docker_disk")]
    Disk { part: Option<DiskPart> },
    #[serde(rename = "docker_prune")]
    Prune { what: PruneTarget },
}

impl Request {
    pub fn validate(&self) -> Result<(), InvalidRequest> {
        let container = |v: &String| {
            if valid_container(v) {
                Ok(())
            } else {
                Err(InvalidRequest::Container(v.clone()))
            }
        };
        let project = |v: &String| {
            if valid_project(v) {
                Ok(())
            } else {
                Err(InvalidRequest::Project(v.clone()))
            }
        };
        let list = |items: &[String]| {
            if items.is_empty() || items.len() > MAX_IDS {
                return Err(InvalidRequest::Count);
            }
            if items
                .iter()
                .fold(0usize, |n, item| n.saturating_add(item.len()))
                > MAX_REQUEST_TEXT
            {
                return Err(InvalidRequest::Text);
            }
            Ok(())
        };
        match self {
            Self::Snapshot | Self::Disk { .. } | Self::Prune { .. } => Ok(()),
            Self::Inspect { container: v } | Self::Action { container: v, .. } => container(v),
            Self::Logs(request) => {
                match &request.target {
                    LogTarget::Container(v) => container(v)?,
                    LogTarget::Project(v) => project(v)?,
                }
                if request.cursor.as_ref().is_some_and(|v| !valid_cursor(v)) {
                    return Err(InvalidRequest::Cursor);
                }
                if request
                    .lines
                    .is_some_and(|n| n == 0 || n > MAX_LOG_ENTRIES as u32)
                {
                    return Err(InvalidRequest::Lines);
                }
                Ok(())
            }
            Self::ProjectAction {
                project: v,
                expected_ids,
                ..
            } => {
                project(v)?;
                list(expected_ids)?;
                if let Some(id) = expected_ids.iter().find(|id| !valid_id(id)) {
                    return Err(InvalidRequest::Id(id.clone()));
                }
                Ok(())
            }
            Self::Remove { containers } => {
                list(containers)?;
                containers.iter().try_for_each(container)
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Done {
    pub ok: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Removed {
    pub removed: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Reply {
    Snapshot(ContainersSnapshot),
    Inspect(Box<ContainerInspect>),
    Logs(ContainerLogPage),
    Action(Done),
    ProjectAction(Done),
    Removed(Removed),
    Disk(DockerDisk),
    Pruned(Pruned),
}
impl Reply {
    pub fn validate_for(&self, request: &Request) -> Result<(), Invalid> {
        match (self, request) {
            (Self::Snapshot(v), Request::Snapshot) => v.validate(),
            (Self::Inspect(v), Request::Inspect { .. }) => v.validate(),
            (Self::Logs(v), Request::Logs(_)) => v.validate(),
            (Self::Action(v), Request::Action { .. })
            | (Self::ProjectAction(v), Request::ProjectAction { .. })
                if v.ok =>
            {
                Ok(())
            }
            (Self::Removed(v), Request::Remove { containers })
                if v.removed <= containers.len() as u64 =>
            {
                Ok(())
            }
            (Self::Disk(v), Request::Disk { .. }) => v.validate(),
            (Self::Pruned(_), Request::Prune { .. }) => Ok(()),
            _ => Err(Invalid("container result operation")),
        }
    }
}

#[cfg(test)]
mod tests;
