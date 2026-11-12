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
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct File {
    pub path: String,
    pub text: Option<String>,
    pub error: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Service {
    pub name: String,
    pub image: Option<String>,
    pub build: bool,
    pub profiles: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Running,
    Succeeded,
    Failed,
    Cancelled,
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub job: String,
    pub project: String,
    pub command: String,
    pub state: State,
    pub exit_code: Option<i32>,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub output: String,
    pub offset: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Jobs {
    pub jobs: Vec<Job>,
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Done {
    pub ok: bool,
}
