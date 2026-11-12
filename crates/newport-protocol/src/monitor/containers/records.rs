use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ContainerVerb {
    Start,
    Stop,
    Restart,
    Pause,
    Unpause,
}
impl ContainerVerb {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Restart => "restart",
            Self::Pause => "pause",
            Self::Unpause => "unpause",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProjectVerb {
    Start,
    Stop,
    Restart,
}
impl ProjectVerb {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Restart => "restart",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PruneTarget {
    Containers,
    DanglingImages,
    UnusedImages,
    BuildCache,
    Volumes,
}
impl PruneTarget {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Containers => "containers",
            Self::DanglingImages => "dangling-images",
            Self::UnusedImages => "unused-images",
            Self::BuildCache => "build-cache",
            Self::Volumes => "volumes",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ContainerState {
    #[default]
    Created,
    Running,
    Paused,
    Restarting,
    Removing,
    Exited,
    Dead,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ContainerHealth {
    #[default]
    Healthy,
    Unhealthy,
    Starting,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PortProtocol {
    #[default]
    Tcp,
    Udp,
    Sctp,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LogStream {
    #[default]
    Stdout,
    Stderr,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineInfo {
    pub version: String,
    pub api_version: String,
    pub os: String,
    pub rootless: bool,
    pub podman: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerPort {
    pub ip: Option<String>,
    pub host_port: Option<u16>,
    pub container_port: u16,
    pub protocol: PortProtocol,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposeLabels {
    pub project: String,
    pub service: String,
    pub oneoff: bool,
    pub number: Option<u64>,
    pub working_dir: Option<String>,
    pub config_files: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Container {
    pub id: String,
    pub name: String,
    pub image: String,
    pub image_id: String,
    pub command: String,
    pub state: ContainerState,
    pub status: String,
    pub health: Option<ContainerHealth>,
    pub exit_code: Option<i64>,
    pub oom_killed: bool,
    pub error: Option<String>,
    pub restart_count: u64,
    pub restart_policy: Option<String>,
    /// Epoch milliseconds.
    pub created: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub ports: Vec<ContainerPort>,
    pub compose: Option<ComposeLabels>,
    /// Percent of one core since the agent's previous reading.
    pub cpu: Option<f64>,
    pub memory: Option<u64>,
    pub memory_limit: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
pub struct ContainersSnapshot {
    pub engine: EngineInfo,
    pub containers: Vec<Container>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct EnvVar {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct Mount {
    #[serde(rename = "type")]
    pub kind: String,
    pub source: Option<String>,
    pub destination: String,
    pub mode: Option<String>,
    pub rw: bool,
    pub name: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Network {
    pub name: String,
    pub ip_address: Option<String>,
    pub aliases: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct Healthcheck {
    pub test: Vec<String>,
    pub interval: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthResult {
    pub at: i64,
    pub exit_code: i64,
    pub output: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerInspect {
    pub id: String,
    pub name: String,
    pub image: String,
    pub created: i64,
    pub entrypoint: Option<Vec<String>>,
    pub cmd: Option<Vec<String>>,
    pub working_dir: Option<String>,
    pub user: Option<String>,
    pub hostname: Option<String>,
    pub env: Vec<EnvVar>,
    pub mounts: Vec<Mount>,
    pub networks: Vec<Network>,
    pub restart_policy: Option<String>,
    pub labels: BTreeMap<String, String>,
    pub healthcheck: Option<Healthcheck>,
    pub last_health: Vec<HealthResult>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct ContainerLogEntry {
    pub at: Option<i64>,
    pub stream: LogStream,
    pub message: String,
    pub container: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct ContainerLogPage {
    pub entries: Vec<ContainerLogEntry>,
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerImage {
    pub id: String,
    pub tags: Vec<String>,
    pub size: u64,
    pub shared_size: Option<u64>,
    /// Epoch milliseconds.
    pub created: i64,
    pub containers: u64,
    pub dangling: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerVolume {
    pub name: String,
    pub driver: String,
    /// Missing when the engine cannot report this volume's usage.
    pub size: Option<u64>,
    pub ref_count: Option<u64>,
    pub anonymous: bool,
    pub compose: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct DiskTotals {
    pub count: u64,
    pub size: u64,
    pub reclaimable: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerTotals {
    pub images: DiskTotals,
    pub containers: DiskTotals,
    pub volumes: DiskTotals,
    pub build_cache: DiskTotals,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerDisk {
    pub images: Vec<DockerImage>,
    pub volumes: Vec<DockerVolume>,
    pub totals: DockerTotals,
    /// Whether volume sizes have been measured.
    pub volumes_measured: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pruned {
    pub removed: u64,
    pub reclaimed: u64,
}
