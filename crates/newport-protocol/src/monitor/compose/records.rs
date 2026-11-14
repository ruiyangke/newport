use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CliKind {
    Plugin,
    Standalone,
    Podman,
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Cli {
    pub kind: CliKind,
    pub version: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct File {
    pub path: String,
    pub text: Option<String>,
    pub error: Option<String>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct Service {
    pub name: String,
    pub image: Option<String>,
    pub build: bool,
    pub profiles: Vec<String>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub cli: Option<Cli>,
    pub cli_error: Option<String>,
    pub working_dir: Option<String>,
    pub config_files: Vec<String>,
    pub files: Vec<File>,
    pub services: Option<Vec<Service>>,
    pub config_error: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Started {
    pub job: String,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    #[default]
    Running,
    Succeeded,
    Failed,
    Cancelled,
}
impl State {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
    pub const fn is_running(self) -> bool {
        matches!(self, Self::Running)
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub job: String,
    pub project: String,
    pub command: String,
    pub state: State,
    pub exit_code: Option<i32>,
    /// Epoch milliseconds.
    pub started_at: i64,
    /// Epoch milliseconds.
    pub finished_at: Option<i64>,
    /// The output suffix after the requested offset. Job listings carry none.
    pub output: String,
    /// Cumulative UTF-8 byte position just after `output`. Listings use zero.
    pub offset: u64,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct Jobs {
    /// Agent insertion order, newest first; wall-clock times may tie or move
    /// backwards.
    pub jobs: Vec<Job>,
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Done {
    pub ok: bool,
}
