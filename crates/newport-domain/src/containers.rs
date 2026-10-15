//! Container contracts and domain rules shared by desktop clients.
//! Mirrors `src/domain/containers.ts`; no UI, transport, or runtime dependencies.
use serde::{Deserialize, Serialize};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, HashSet},
    fmt,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
};

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
    pub const fn label(self) -> &'static str {
        match self {
            Self::Start => "Start",
            Self::Stop => "Stop",
            Self::Restart => "Restart",
            Self::Pause => "Pause",
            Self::Unpause => "Resume",
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
    pub const fn label(self) -> &'static str {
        match self {
            Self::Start => "Start",
            Self::Stop => "Stop",
            Self::Restart => "Restart",
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
impl ContainerState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Restarting => "restarting",
            Self::Removing => "removing",
            Self::Exited => "exited",
            Self::Dead => "dead",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ContainerHealth {
    #[default]
    Healthy,
    Unhealthy,
    Starting,
}
impl ContainerHealth {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Unhealthy => "unhealthy",
            Self::Starting => "starting",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PortProtocol {
    #[default]
    Tcp,
    Udp,
    Sctp,
}
impl PortProtocol {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::Udp => "udp",
            Self::Sctp => "sctp",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LogStream {
    #[default]
    Stdout,
    Stderr,
}
impl LogStream {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
        }
    }
}

macro_rules! display_as_str {
    ($($kind:ty),+ $(,)?) => {$(
        impl fmt::Display for $kind {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }
    )+};
}
display_as_str!(
    ContainerVerb,
    ProjectVerb,
    PruneTarget,
    ContainerState,
    ContainerHealth,
    PortProtocol,
    LogStream
);

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
impl DockerTotals {
    pub fn size(&self) -> u64 {
        [self.images, self.containers, self.volumes, self.build_cache]
            .iter()
            .fold(0_u64, |total, category| total.saturating_add(category.size))
    }
    pub fn reclaimable(&self) -> u64 {
        [self.images, self.containers, self.volumes, self.build_cache]
            .iter()
            .fold(0_u64, |total, category| {
                total.saturating_add(category.reclaimable)
            })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct DockerDisk {
    pub images: Vec<DockerImage>,
    pub volumes: Vec<DockerVolume>,
    pub totals: DockerTotals,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct Pruned {
    pub removed: u64,
    pub reclaimed: u64,
}

/// A container id (12 to 64 hex digits) or a container name.
pub fn valid_container(target: &str) -> bool {
    let hex = (12..=64).contains(&target.len()) && target.bytes().all(|b| b.is_ascii_hexdigit());
    let name = !target.is_empty()
        && target.len() <= 128
        && target.as_bytes()[0].is_ascii_alphanumeric()
        && target
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b));
    hex || name
}

/// Full engine identity used for operations that must survive name reuse safely.
pub fn valid_container_id(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// A shell inside one immutable container: bash when installed, otherwise sh.
/// The fixed script and quoted full ID never interpolate a container's name.
pub fn container_shell_command(container_id: &str, podman: bool) -> Option<String> {
    valid_container_id(container_id).then(|| {
        let engine = if podman { "podman" } else { "docker" };
        format!("{engine} exec -it '{container_id}' sh -c 'command -v bash >/dev/null && exec bash || exec sh'")
    })
}

/// Compose's project-name rules, shared with backend request validation.
pub fn valid_project(project: &str) -> bool {
    !project.is_empty()
        && project.len() <= 256
        && project.as_bytes()[0].is_ascii_alphanumeric()
        && project
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-".contains(&b))
}

/// Environment values with these names require an individual reveal even when
/// ordinary values have been revealed. This is presentation policy, not storage encryption.
pub fn secret_env_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    [
        "pass",
        "secret",
        "token",
        "key",
        "credential",
        "auth",
        "private",
        "dsn",
        "cookie",
    ]
    .iter()
    .any(|needle| name.contains(needle))
}

pub const DISK: &str = "disk";

/// Names survive Compose recreating a container under a different engine id.
pub fn container_key(container: &Container) -> String {
    format!("c:{}", container.name)
}
pub fn project_key(project: &str) -> String {
    format!("p:{project}")
}
pub fn selected_container<'a>(containers: &'a [Container], key: &str) -> Option<&'a Container> {
    let name = key.strip_prefix("c:")?;
    containers.iter().find(|container| container.name == name)
}
pub fn display_name(container: &Container) -> &str {
    container
        .compose
        .as_ref()
        .filter(|compose| !compose.oneoff)
        .map(|compose| compose.service.as_str())
        .unwrap_or(&container.name)
}
pub fn is_running(container: &Container) -> bool {
    matches!(
        container.state,
        ContainerState::Running | ContainerState::Restarting | ContainerState::Paused
    )
}
pub fn is_job(container: &Container) -> bool {
    container
        .compose
        .as_ref()
        .is_some_and(|compose| compose.oneoff)
}
fn stopped(code: i64) -> bool {
    matches!(code, 0 | 130 | 137 | 143)
}
pub fn is_problem(container: &Container) -> bool {
    let failed =
        container.state == ContainerState::Exited && !stopped(container.exit_code.unwrap_or(0));
    if is_job(container) {
        return failed;
    }
    (container.state == ContainerState::Running
        && container.health == Some(ContainerHealth::Unhealthy))
        || matches!(
            container.state,
            ContainerState::Restarting | ContainerState::Dead
        )
        || container.oom_killed
        || failed
}

/// The actions offered by the existing Containers detail UI. Engine state can
/// change between this snapshot and execution; the engine remains authoritative.
pub fn available_container_verbs(container: &Container) -> &'static [ContainerVerb] {
    use ContainerVerb::*;
    match container.state {
        ContainerState::Running => &[Restart, Stop, Pause],
        ContainerState::Restarting => &[Restart, Stop],
        ContainerState::Paused => &[Restart, Stop, Unpause],
        _ => &[Start],
    }
}

/// Removal never forces a running, paused, restarting, or already removing
/// container. The engine rechecks the state when the request arrives.
pub fn can_remove_container(container: &Container) -> bool {
    matches!(
        container.state,
        ContainerState::Created | ContainerState::Exited | ContainerState::Dead
    )
}

/// One displayed binding per published host port and protocol. Prefer a
/// reachable loopback/wildcard binding before choosing IPv4 over IPv6.
pub fn published_ports(container: &Container) -> Vec<&ContainerPort> {
    let mut published = BTreeMap::<(u16, &str), &ContainerPort>::new();
    for port in &container.ports {
        let Some(host_port) = port.host_port else {
            continue;
        };
        let key = (host_port, port.protocol.as_str());
        match published.get(&key) {
            Some(previous) if binding_rank(previous) <= binding_rank(port) => {}
            _ => {
                published.insert(key, port);
            }
        }
    }
    published.into_values().collect()
}

/// Internal-only ports, deduplicated by both port number and protocol. Every
/// published binding counts, including one hidden by the IPv4/IPv6 projection.
pub fn exposed_ports(container: &Container) -> Vec<&ContainerPort> {
    let published: HashSet<_> = container
        .ports
        .iter()
        .filter(|port| port.host_port.is_some())
        .map(|port| (port.container_port, port.protocol))
        .collect();
    let mut seen = HashSet::new();
    container
        .ports
        .iter()
        .filter(|port| {
            let key = (port.container_port, port.protocol);
            port.host_port.is_none() && !published.contains(&key) && seen.insert(key)
        })
        .collect()
}

/// SSH forwards carry TCP to a server's canonical loopback or wildcard bind.
/// Expanded IPv6 spellings have the same meaning as their canonical form.
pub fn forwardable(port: &ContainerPort) -> bool {
    port.protocol == PortProtocol::Tcp
        && port.host_port.is_some_and(|number| number != 0)
        && port.container_port != 0
        && loopback_binding(port.ip.as_deref())
}

pub fn port_label(port: &ContainerPort) -> String {
    let mapping = match port.host_port {
        Some(host) if host != port.container_port => format!("{host}→{}", port.container_port),
        Some(host) => host.to_string(),
        None => port.container_port.to_string(),
    };
    if port.protocol == PortProtocol::Tcp {
        mapping
    } else {
        format!("{mapping}/{}", port.protocol)
    }
}

fn loopback_binding(address: Option<&str>) -> bool {
    match address {
        None | Some("") => true,
        Some(address) => match address.parse::<IpAddr>() {
            Ok(IpAddr::V4(ip)) => ip == Ipv4Addr::UNSPECIFIED || ip == Ipv4Addr::LOCALHOST,
            Ok(IpAddr::V6(ip)) => ip == Ipv6Addr::UNSPECIFIED || ip == Ipv6Addr::LOCALHOST,
            Err(_) => false,
        },
    }
}
fn binding_rank(port: &ContainerPort) -> (bool, bool, String) {
    let address = port
        .ip
        .as_deref()
        .filter(|address| !address.is_empty())
        .unwrap_or("0.0.0.0");
    let parsed = address.parse::<IpAddr>();
    (
        !loopback_binding(port.ip.as_deref()),
        matches!(parsed, Ok(IpAddr::V6(_))),
        parsed
            .map(|ip| ip.to_string())
            .unwrap_or_else(|_| address.to_owned()),
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Success,
    Warning,
    Danger,
    Muted,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContainerStatus {
    pub label: &'static str,
    pub tone: Tone,
}
pub fn state_of(container: &Container) -> ContainerStatus {
    use ContainerState::*;
    use Tone::*;
    let (label, tone) = match container.state {
        Restarting => ("Restarting", Warning),
        Dead => ("Dead", Danger),
        Removing => ("Removing", Warning),
        Paused => ("Paused", Muted),
        Created => ("Created", Muted),
        Running => match container.health {
            Some(ContainerHealth::Unhealthy) => ("Unhealthy", Danger),
            Some(ContainerHealth::Starting) => ("Starting", Warning),
            _ => ("Running", Success),
        },
        Exited if container.oom_killed => ("Out of memory", Danger),
        Exited if is_problem(container) => ("Crashed", Danger),
        Exited if is_job(container) && container.exit_code.unwrap_or(0) == 0 => {
            ("Completed", Muted)
        }
        Exited => ("Stopped", Muted),
    };
    ContainerStatus { label, tone }
}
pub fn problem_reason(container: &Container) -> Option<String> {
    if container.state == ContainerState::Running
        && container.health == Some(ContainerHealth::Unhealthy)
    {
        return Some("Failing its health check".into());
    }
    if container.state == ContainerState::Restarting {
        return Some(if container.restart_count > 0 {
            format!("Keeps exiting; restarted {}×", container.restart_count)
        } else {
            "Keeps exiting and restarting".into()
        });
    }
    let error = container.error.as_deref().filter(|error| !error.is_empty());
    if container.state == ContainerState::Dead {
        return Some(
            error
                .map(|error| format!("Docker couldn't stop or remove it: {error}"))
                .unwrap_or_else(|| "Dead".into()),
        );
    }
    if container.oom_killed {
        return Some("Killed for running out of memory".into());
    }
    if container.state != ContainerState::Exited {
        return None;
    }
    if let Some(error) = error {
        return Some(error.into());
    }
    let code = container.exit_code.unwrap_or(0);
    (!stopped(code)).then(|| exit_meaning(code))
}
pub fn exit_meaning(code: i64) -> String {
    match code {
        0 => "Exited normally".into(),
        1 => "Exited with an error (code 1)".into(),
        125 => "Docker couldn't run it (code 125)".into(),
        126 => "Its command couldn't be executed (code 126)".into(),
        127 => "Its command wasn't found (code 127)".into(),
        134 => "Aborted (code 134, SIGABRT)".into(),
        137 => "Killed (code 137, SIGKILL)".into(),
        139 => "Crashed with a segmentation fault (code 139)".into(),
        143 => "Stopped (code 143, SIGTERM)".into(),
        129..=159 => format!("Killed by signal {} (code {code})", code - 128),
        _ => format!("Exited with code {code}"),
    }
}
pub fn format_cpu(cpu: f64) -> String {
    if cpu >= 10.0 {
        format!("{}%", cpu.round())
    } else {
        format!("{cpu:.1}%")
    }
}
pub fn list_note(container: &Container, now: i64) -> String {
    match container.state {
        ContainerState::Running if container.health == Some(ContainerHealth::Unhealthy) => {
            "unhealthy".into()
        }
        ContainerState::Running => container
            .cpu
            .map(format_cpu)
            .unwrap_or_else(|| since(container.started_at, now, "")),
        ContainerState::Restarting => format!("restart ×{}", container.restart_count),
        ContainerState::Exited if container.oom_killed => "OOM".into(),
        ContainerState::Exited if is_problem(container) => {
            format!("exit {}", container.exit_code.unwrap_or(0))
        }
        ContainerState::Exited => since(container.finished_at, now, " ago"),
        _ => state_of(container).label.to_ascii_lowercase(),
    }
}
fn since(at: Option<i64>, now: i64, suffix: &str) -> String {
    at.map(|at| {
        format!(
            "{}{suffix}",
            crate::services::duration(now.saturating_sub(at))
        )
    })
    .unwrap_or_default()
}

pub fn matches_query(container: &Container, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    matches_needle(container, &query)
}
fn matches_needle(container: &Container, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let (project, service) = container
        .compose
        .as_ref()
        .map(|compose| (compose.project.as_str(), compose.service.as_str()))
        .unwrap_or_default();
    format!(
        "{} {} {project} {service} {} {}",
        container.name,
        container.image,
        container.state,
        container
            .health
            .map(|health| health.as_str())
            .unwrap_or_default()
    )
    .to_lowercase()
    .contains(needle)
}

#[derive(Debug, Default)]
pub struct ProjectGroup<'a> {
    pub name: &'a str,
    pub services: Vec<&'a Container>,
    pub jobs: Vec<&'a Container>,
    pub running: usize,
    pub working_dir: Option<&'a str>,
    pub problems: usize,
}
#[derive(Debug, Default)]
pub struct ContainerGroups<'a> {
    pub problems: Vec<&'a Container>,
    pub projects: Vec<ProjectGroup<'a>>,
    pub standalone: Vec<&'a Container>,
}
pub fn group_containers(containers: &[Container]) -> ContainerGroups<'_> {
    group_rows(containers.iter())
}
pub fn group_matching_containers<'a>(
    containers: &'a [Container],
    query: &str,
) -> ContainerGroups<'a> {
    let needle = query.trim().to_lowercase();
    group_rows(
        containers
            .iter()
            .filter(|container| matches_needle(container, &needle)),
    )
}
// Browser localeCompare is locale-dependent. Use deterministic case-insensitive
// ordering with an exact tie-break so snapshots don't reorder equal labels.
fn compare_text(a: &str, b: &str) -> Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| a.cmp(b))
}
fn compare_containers(a: &&Container, b: &&Container) -> Ordering {
    compare_text(display_name(a), display_name(b))
        .then_with(|| {
            a.compose
                .as_ref()
                .and_then(|c| c.number)
                .unwrap_or(0)
                .cmp(&b.compose.as_ref().and_then(|c| c.number).unwrap_or(0))
        })
        .then_with(|| compare_text(&a.name, &b.name))
}
fn group_rows<'a>(containers: impl Iterator<Item = &'a Container>) -> ContainerGroups<'a> {
    let mut groups = ContainerGroups::default();
    let mut projects = BTreeMap::<&str, ProjectGroup<'a>>::new();
    for container in containers {
        if is_problem(container) {
            groups.problems.push(container);
        }
        let Some(compose) = &container.compose else {
            groups.standalone.push(container);
            continue;
        };
        let project = projects
            .entry(&compose.project)
            .or_insert_with(|| ProjectGroup {
                name: &compose.project,
                ..Default::default()
            });
        if project.working_dir.is_none() {
            project.working_dir = compose.working_dir.as_deref().filter(|dir| !dir.is_empty());
        }
        project.problems += usize::from(is_problem(container));
        if compose.oneoff {
            project.jobs.push(container);
        } else {
            project.running += usize::from(is_running(container));
            project.services.push(container);
        }
    }
    groups.problems.sort_by(compare_containers);
    groups.standalone.sort_by(compare_containers);
    groups.projects = projects
        .into_values()
        .map(|mut project| {
            project.services.sort_by(compare_containers);
            project
                .jobs
                .sort_by_key(|container| std::cmp::Reverse(container.created));
            project
        })
        .collect();
    groups.projects.sort_by(|a, b| {
        (b.problems > 0)
            .cmp(&(a.problems > 0))
            .then_with(|| compare_text(a.name, b.name))
    });
    groups
}
pub fn default_selection(groups: &ContainerGroups<'_>) -> String {
    groups
        .problems
        .first()
        .map(|container| container_key(container))
        .or_else(|| {
            groups
                .projects
                .first()
                .map(|project| project_key(project.name))
        })
        .or_else(|| {
            groups
                .standalone
                .first()
                .map(|container| container_key(container))
        })
        .unwrap_or_else(|| DISK.into())
}
pub fn project_state(project: &ProjectGroup<'_>) -> ContainerStatus {
    let (label, tone) = if project.services.is_empty() {
        ("Jobs only", Tone::Muted)
    } else if project
        .services
        .iter()
        .any(|container| is_problem(container))
    {
        ("Needs attention", Tone::Danger)
    } else if project.running == project.services.len() {
        ("Running", Tone::Success)
    } else if project.running == 0 {
        ("Stopped", Tone::Muted)
    } else {
        ("Partly running", Tone::Warning)
    };
    ContainerStatus { label, tone }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub running: usize,
    pub stopped: usize,
    pub problems: usize,
}
pub fn summary(containers: &[Container]) -> Summary {
    let mut summary = Summary::default();
    for container in containers {
        if !is_job(container) {
            if is_running(container) {
                summary.running += 1;
            } else {
                summary.stopped += 1;
            }
        }
        summary.problems += usize::from(is_problem(container));
    }
    summary
}
impl fmt::Display for Summary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} running", self.running)?;
        if self.stopped > 0 {
            write!(f, " · {} stopped", self.stopped)?;
        }
        if self.problems > 0 {
            write!(
                f,
                " · {} {} attention",
                self.problems,
                if self.problems == 1 { "needs" } else { "need" }
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn container(name: &str) -> Container {
        Container {
            id: format!("{name:0<64}"),
            name: name.into(),
            image: "example/app:1".into(),
            state: ContainerState::Running,
            cpu: Some(1.5),
            ..Default::default()
        }
    }
    fn service(project: &str, name: &str, replica: u64) -> Container {
        Container {
            compose: Some(ComposeLabels {
                project: project.into(),
                service: name.into(),
                number: Some(replica),
                working_dir: Some(format!("/srv/{project}")),
                ..Default::default()
            }),
            ..container(&format!("{project}-{name}-{replica}"))
        }
    }
    fn exited(mut container: Container, code: i64) -> Container {
        container.state = ContainerState::Exited;
        container.exit_code = Some(code);
        container.cpu = None;
        container.finished_at = Some(0);
        container
    }
    fn job(project: &str, name: &str, created: i64, code: i64) -> Container {
        let mut container = exited(service(project, name, 1), code);
        container.compose.as_mut().unwrap().oneoff = true;
        container.created = created;
        container
    }

    #[test]
    fn wire_contract_preserves_names_required_fields_and_positional_order() {
        let value = json!({
            "engine": {"version":"29.4", "apiVersion":"1.47", "os":"Linux", "rootless":true, "podman":false},
            "containers": [{
                "id":"a".repeat(64), "name":"web", "image":"nginx", "imageId":"sha256:1", "command":"nginx",
                "state":"running", "status":"Up", "health":"healthy", "exitCode":null,
                "oomKilled":false, "error":null, "restartCount":2, "restartPolicy":"always",
                "created":11, "startedAt":12, "finishedAt":null,
                "ports":[{"ip":"0.0.0.0", "hostPort":8080, "containerPort":80, "protocol":"tcp"}],
                "compose":{"project":"app", "service":"web", "oneoff":false, "number":1, "workingDir":"/srv/app", "configFiles":null},
                "cpu":1.5, "memory":9000, "memoryLimit":null
            }],
            "truncated":false
        });
        let snapshot: ContainersSnapshot = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&snapshot).unwrap(), value);
        let named = rmp_serde::to_vec_named(&snapshot).unwrap();
        assert_eq!(
            rmp_serde::from_slice::<ContainersSnapshot>(&named).unwrap(),
            snapshot
        );
        let positional = rmp_serde::to_vec(&snapshot).unwrap();
        assert_eq!(
            rmp_serde::from_slice::<ContainersSnapshot>(&positional).unwrap(),
            snapshot
        );
        let positional_value: serde_json::Value = rmp_serde::from_slice(&positional).unwrap();
        assert_eq!(
            positional_value,
            json!([
                ["29.4", "1.47", "Linux", true, false],
                [[
                    "a".repeat(64),
                    "web",
                    "nginx",
                    "sha256:1",
                    "nginx",
                    "running",
                    "Up",
                    "healthy",
                    null,
                    false,
                    null,
                    2,
                    "always",
                    11,
                    12,
                    null,
                    [["0.0.0.0", 8080, 80, "tcp"]],
                    ["app", "web", false, 1, "/srv/app", null],
                    1.5,
                    9000,
                    null
                ]],
                false
            ])
        );
        for (field, invalid) in [
            ("state", json!("zombie")),
            ("health", json!("none")),
            ("created", json!("yesterday")),
        ] {
            let mut invalid_value = value.clone();
            invalid_value["containers"][0][field] = invalid;
            assert!(serde_json::from_value::<ContainersSnapshot>(invalid_value).is_err());
        }
        let mut missing = value;
        missing["containers"][0]
            .as_object_mut()
            .unwrap()
            .remove("oomKilled");
        assert!(serde_json::from_value::<ContainersSnapshot>(missing).is_err());
    }

    #[test]
    fn inspect_and_logs_keep_the_backend_contract() {
        let inspect = json!({
            "id":"id", "name":"web", "image":"nginx", "created":11,
            "entrypoint":["nginx"], "cmd":null, "workingDir":"/srv", "user":"1000", "hostname":"web",
            "env":[{"name":"TOKEN", "value":"secret"}],
            "mounts":[{"type":"volume", "source":null, "destination":"/data", "mode":"rw", "rw":true, "name":"data"}],
            "networks":[{"name":"app", "ipAddress":"172.17.0.2", "aliases":["web"]}],
            "restartPolicy":"always", "labels":{"owner":"newport"},
            "healthcheck":{"test":["CMD", "true"], "interval":10},
            "lastHealth":[{"at":12, "exitCode":0, "output":"ok"}]
        });
        let typed: ContainerInspect = serde_json::from_value(inspect.clone()).unwrap();
        assert_eq!(serde_json::to_value(&typed).unwrap(), inspect);
        assert_eq!(
            rmp_serde::from_slice::<ContainerInspect>(&rmp_serde::to_vec(&typed).unwrap()).unwrap(),
            typed
        );
        let logs = json!({"entries":[{"at":12, "stream":"stderr", "message":"failed", "container":"web"}], "cursor":"12.0"});
        let typed: ContainerLogPage = serde_json::from_value(logs.clone()).unwrap();
        assert_eq!(serde_json::to_value(&typed).unwrap(), logs);
        assert_eq!(
            rmp_serde::from_slice::<ContainerLogPage>(&rmp_serde::to_vec(&typed).unwrap()).unwrap(),
            typed
        );
        let mut invalid = logs;
        invalid["entries"][0]["stream"] = json!("stdin");
        assert!(serde_json::from_value::<ContainerLogPage>(invalid).is_err());
    }

    #[test]
    fn disk_contract_preserves_json_names_and_messagepack_field_order() {
        let total = json!({"count":1,"size":20,"reclaimable":10});
        let value = json!({
            "images":[{"id":"sha256:1","tags":["web:latest"],"size":20,"sharedSize":3,"created":11,"containers":2,"dangling":false}],
            "volumes":[{"name":"app_data","driver":"local","size":null,"refCount":null,"anonymous":false,"compose":"app"}],
            "totals":{"images":total,"containers":total,"volumes":total,"buildCache":total}
        });
        let disk: DockerDisk = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&disk).unwrap(), value);
        assert_eq!(disk.volumes[0].size, None);
        assert_eq!(disk.volumes[0].ref_count, None);
        for encoded in [
            rmp_serde::to_vec_named(&disk).unwrap(),
            rmp_serde::to_vec(&disk).unwrap(),
        ] {
            assert_eq!(rmp_serde::from_slice::<DockerDisk>(&encoded).unwrap(), disk);
        }
        let positional: serde_json::Value =
            rmp_serde::from_slice(&rmp_serde::to_vec(&disk).unwrap()).unwrap();
        assert_eq!(
            positional,
            json!([
                [["sha256:1", ["web:latest"], 20, 3, 11, 2, false]],
                [["app_data", "local", null, null, false, "app"]],
                [[1, 20, 10], [1, 20, 10], [1, 20, 10], [1, 20, 10]]
            ])
        );
        for pointer in [
            "/images/0/size",
            "/images/0/containers",
            "/volumes/0/refCount",
            "/totals/images/reclaimable",
        ] {
            let mut invalid = value.clone();
            *invalid.pointer_mut(pointer).unwrap() = json!(-1);
            assert!(
                serde_json::from_value::<DockerDisk>(invalid).is_err(),
                "{pointer}"
            );
        }
        let mut missing = value.clone();
        missing["totals"]
            .as_object_mut()
            .unwrap()
            .remove("buildCache");
        assert!(serde_json::from_value::<DockerDisk>(missing).is_err());
        let mut missing = value;
        missing["images"][0]
            .as_object_mut()
            .unwrap()
            .remove("dangling");
        assert!(serde_json::from_value::<DockerDisk>(missing).is_err());
    }

    #[test]
    fn cleanup_result_and_disk_totals_preserve_unsigned_byte_counts() {
        let pruned = Pruned {
            removed: 2,
            reclaimed: 4096,
        };
        assert_eq!(
            serde_json::to_value(pruned).unwrap(),
            json!({"removed":2,"reclaimed":4096})
        );
        let positional: serde_json::Value =
            rmp_serde::from_slice(&rmp_serde::to_vec(&pruned).unwrap()).unwrap();
        assert_eq!(positional, json!([2, 4096]));
        for invalid in [
            json!({"removed":-1,"reclaimed":0}),
            json!({"removed":0}),
            json!({"removed":0,"reclaimed":"4096"}),
        ] {
            assert!(serde_json::from_value::<Pruned>(invalid).is_err());
        }
        let mut totals = DockerTotals {
            images: DiskTotals {
                count: 1,
                size: 100,
                reclaimable: 80,
            },
            containers: DiskTotals {
                count: 2,
                size: 10,
                reclaimable: 1,
            },
            volumes: DiskTotals {
                count: 3,
                size: 50,
                reclaimable: 20,
            },
            build_cache: DiskTotals {
                count: 4,
                size: 30,
                reclaimable: 30,
            },
        };
        assert_eq!(totals.size(), 190);
        assert_eq!(totals.reclaimable(), 131);
        totals.images.size = u64::MAX;
        totals.images.reclaimable = u64::MAX;
        assert_eq!(totals.size(), u64::MAX);
        assert_eq!(totals.reclaimable(), u64::MAX);
    }

    #[test]
    fn container_shell_uses_only_a_quoted_full_engine_id_and_fixed_fallback_script() {
        let id = "0123456789abcdef".repeat(4);
        assert_eq!(
            container_shell_command(&id, false).unwrap(),
            format!(
                "docker exec -it '{id}' sh -c 'command -v bash >/dev/null && exec bash || exec sh'"
            )
        );
        assert_eq!(
            container_shell_command(&id, true).unwrap(),
            format!(
                "podman exec -it '{id}' sh -c 'command -v bash >/dev/null && exec bash || exec sh'"
            )
        );
        assert!(valid_container_id(&id.to_ascii_uppercase()));
        for invalid in [
            "".to_owned(),
            "web".to_owned(),
            id[..12].to_owned(),
            format!("{}'", "a".repeat(63)),
            format!("{};", "a".repeat(63)),
            format!("{}\n", "a".repeat(63)),
            format!("{}\0", "a".repeat(63)),
            "$(touch /tmp/unexpected)".to_owned(),
            "a".repeat(65),
            "é".repeat(32),
        ] {
            assert!(!valid_container_id(&invalid), "{invalid:?}");
            assert!(
                container_shell_command(&invalid, false).is_none(),
                "{invalid:?}"
            );
        }
    }

    #[test]
    fn individual_removal_is_limited_to_stopped_engine_states() {
        for state in [
            ContainerState::Created,
            ContainerState::Exited,
            ContainerState::Dead,
        ] {
            assert!(can_remove_container(&Container {
                state,
                ..Default::default()
            }));
        }
        for state in [
            ContainerState::Running,
            ContainerState::Restarting,
            ContainerState::Paused,
            ContainerState::Removing,
        ] {
            assert!(!can_remove_container(&Container {
                state,
                ..Default::default()
            }));
        }
    }

    fn port(
        ip: Option<&str>,
        host_port: Option<u16>,
        container_port: u16,
        protocol: PortProtocol,
    ) -> ContainerPort {
        ContainerPort {
            ip: ip.map(str::to_owned),
            host_port,
            container_port,
            protocol,
        }
    }

    #[test]
    fn published_ports_prefer_reachable_bindings_then_ipv4_without_merging_protocols() {
        use PortProtocol::*;
        let mut row = Container {
            ports: vec![
                port(Some("10.0.0.2"), Some(8080), 80, Tcp),
                port(Some("0:0:0:0:0:0:0:0"), Some(8080), 80, Tcp),
                port(Some("::1"), Some(9090), 90, Tcp),
                port(Some("10.0.0.3"), Some(9090), 90, Tcp),
                port(Some("0.0.0.0"), Some(8080), 80, Tcp),
                port(Some("0.0.0.0"), Some(8080), 80, Udp),
                port(None, None, 9000, Tcp),
            ],
            ..Default::default()
        };
        for _ in 0..2 {
            let published = published_ports(&row);
            assert_eq!(published.len(), 3);
            assert_eq!(
                published
                    .iter()
                    .map(|p| (p.host_port, p.protocol, p.ip.as_deref()))
                    .collect::<Vec<_>>(),
                vec![
                    (Some(8080), Tcp, Some("0.0.0.0")),
                    (Some(8080), Udp, Some("0.0.0.0")),
                    (Some(9090), Tcp, Some("::1"))
                ]
            );
            row.ports.reverse();
        }
    }

    #[test]
    fn exposed_ports_distinguish_protocols_and_consider_every_published_mapping() {
        use PortProtocol::*;
        let row = Container {
            ports: vec![
                port(Some("0.0.0.0"), Some(8080), 80, Tcp),
                port(Some("::"), Some(8080), 81, Tcp),
                port(None, None, 80, Tcp),
                port(None, None, 81, Tcp),
                port(None, None, 80, Udp),
                port(None, None, 80, Udp),
                port(None, None, 80, Sctp),
                port(None, None, 9000, Tcp),
            ],
            ..Default::default()
        };
        assert_eq!(published_ports(&row).len(), 1);
        assert_eq!(
            exposed_ports(&row)
                .iter()
                .map(|port| (port.container_port, port.protocol))
                .collect::<Vec<_>>(),
            vec![(80, Udp), (80, Sctp), (9000, Tcp)]
        );
    }

    #[test]
    fn forwarding_allows_only_valid_tcp_ports_on_canonical_loopback_or_wildcard() {
        for address in [
            None,
            Some(""),
            Some("0.0.0.0"),
            Some("127.0.0.1"),
            Some("::"),
            Some("::1"),
            Some("0:0:0:0:0:0:0:0"),
            Some("0:0:0:0:0:0:0:1"),
        ] {
            assert!(
                forwardable(&port(address, Some(8080), 80, PortProtocol::Tcp)),
                "{address:?}"
            );
        }
        for address in [
            "10.0.0.2",
            "2001:db8::1",
            "127.0.0.2",
            "localhost",
            "[::1]",
            "::ffff:127.0.0.1",
            "bad\nhost",
        ] {
            assert!(
                !forwardable(&port(Some(address), Some(8080), 80, PortProtocol::Tcp)),
                "{address}"
            );
        }
        for invalid in [
            port(None, None, 80, PortProtocol::Tcp),
            port(None, Some(0), 80, PortProtocol::Tcp),
            port(None, Some(8080), 0, PortProtocol::Tcp),
            port(None, Some(8080), 80, PortProtocol::Udp),
            port(None, Some(8080), 80, PortProtocol::Sctp),
        ] {
            assert!(!forwardable(&invalid));
        }
        assert_eq!(
            port_label(&port(None, Some(8080), 80, PortProtocol::Tcp)),
            "8080→80"
        );
        assert_eq!(
            port_label(&port(None, Some(80), 80, PortProtocol::Tcp)),
            "80"
        );
        assert_eq!(
            port_label(&port(None, Some(53), 53, PortProtocol::Udp)),
            "53/udp"
        );
        assert_eq!(
            port_label(&port(None, Some(9000), 9001, PortProtocol::Sctp)),
            "9000→9001/sctp"
        );
        assert_eq!(
            port_label(&port(None, None, 9001, PortProtocol::Udp)),
            "9001/udp"
        );
    }

    #[test]
    fn problems_distinguish_intentional_stops_from_failures_and_stale_health() {
        for code in [0, 130, 137, 143] {
            assert!(!is_problem(&exited(container("a"), code)));
        }
        for code in [1, 127, 139] {
            assert!(is_problem(&exited(container("a"), code)));
        }
        let mut row = exited(container("a"), 0);
        row.health = Some(ContainerHealth::Unhealthy);
        assert!(!is_problem(&row), "Docker retains old health after a stop");
        row.state = ContainerState::Running;
        assert!(is_problem(&row));
        row = exited(container("a"), 137);
        row.oom_killed = true;
        assert!(is_problem(&row));
        assert_eq!(state_of(&row).label, "Out of memory");
        assert_eq!(
            problem_reason(&row).as_deref(),
            Some("Killed for running out of memory")
        );
        row = container("a");
        row.state = ContainerState::Restarting;
        row.restart_count = 6;
        assert!(is_problem(&row));
        assert_eq!(
            problem_reason(&row).as_deref(),
            Some("Keeps exiting; restarted 6×")
        );
        assert_eq!(exit_meaning(132), "Killed by signal 4 (code 132)");
        assert_eq!(
            problem_reason(&exited(container("a"), 127)).as_deref(),
            Some("Its command wasn't found (code 127)")
        );
    }

    #[test]
    fn jobs_follow_exit_results_without_counting_as_running_services() {
        let completed = job("app", "migrate", 1, 0);
        assert!(!is_problem(&completed));
        assert_eq!(state_of(&completed).label, "Completed");
        let mut stopped_job = job("app", "stopped", 2, 137);
        stopped_job.oom_killed = true;
        assert!(
            !is_problem(&stopped_job),
            "Match the current one-off job exit-code policy"
        );
        let failed = job("app", "bad", 3, 1);
        assert!(is_problem(&failed));
        let rows = [
            service("app", "api", 1),
            exited(service("app", "db", 1), 0),
            completed,
            failed,
        ];
        assert_eq!(
            summary(&rows),
            Summary {
                running: 1,
                stopped: 1,
                problems: 1
            }
        );
        assert_eq!(
            summary(&rows).to_string(),
            "1 running · 1 stopped · 1 needs attention"
        );
        assert_eq!(summary(&[]).to_string(), "0 running");
        let groups = group_containers(&rows);
        assert_eq!(project_state(&groups.projects[0]).label, "Partly running");
    }

    #[test]
    fn groups_projects_by_attention_services_by_replica_and_jobs_by_recency() {
        let mut unhealthy = service("atlas", "worker", 1);
        unhealthy.health = Some(ContainerHealth::Unhealthy);
        let mut early_job = job("atlas", "migrate", 10, 0);
        early_job.compose.as_mut().unwrap().working_dir = None;
        let rows = [
            container("portainer"),
            service("aardvark", "web", 1),
            early_job,
            unhealthy,
            service("atlas", "api", 10),
            service("atlas", "api", 2),
            exited(service("ledger", "web", 1), 1),
            job("atlas", "seed", 20, 0),
        ];
        let groups = group_containers(&rows);
        assert_eq!(
            groups.projects.iter().map(|p| p.name).collect::<Vec<_>>(),
            ["atlas", "ledger", "aardvark"]
        );
        assert_eq!(
            groups
                .problems
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            ["ledger-web-1", "atlas-worker-1"]
        );
        let atlas = &groups.projects[0];
        assert_eq!(
            atlas
                .services
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            ["atlas-api-2", "atlas-api-10", "atlas-worker-1"]
        );
        assert_eq!(
            atlas.jobs.iter().map(|c| c.created).collect::<Vec<_>>(),
            [20, 10]
        );
        assert_eq!(atlas.working_dir, Some("/srv/atlas"));
        assert_eq!((atlas.running, atlas.problems), (3, 1));
        assert_eq!(project_state(atlas).label, "Needs attention");
        assert_eq!(groups.standalone[0].name, "portainer");
    }

    #[test]
    fn default_selection_and_name_identity_survive_recreation() {
        let initial = service("app", "api", 1);
        let key = container_key(&initial);
        let mut replacement = initial.clone();
        replacement.id = "new-id".into();
        let rows = [replacement];
        assert_eq!(selected_container(&rows, &key).unwrap().id, "new-id");
        assert!(selected_container(&rows, "c:missing").is_none());
        assert!(selected_container(&rows, "p:app").is_none());
        assert_eq!(default_selection(&group_containers(&rows)), "p:app");
        assert_eq!(
            default_selection(&group_containers(&[container("web")])),
            "c:web"
        );
        let bad = [exited(container("bad"), 1), initial];
        assert_eq!(default_selection(&group_containers(&bad)), "c:bad");
        assert_eq!(default_selection(&group_containers(&[])), DISK);
    }

    #[test]
    fn query_covers_all_tauri_search_fields_without_mutating_the_snapshot() {
        let mut web = service("Atlas", "web", 1);
        web.health = Some(ContainerHealth::Unhealthy);
        for query in [
            "  ATLAS  ",
            "web",
            "example/app",
            "RUNNING",
            "unhealthy",
            "",
            "  ",
        ] {
            assert!(matches_query(&web, query), "{query}");
        }
        assert!(!matches_query(&web, "missing"));
        let rows = [web, container("portainer")];
        let groups = group_matching_containers(&rows, "atlas");
        assert_eq!(groups.projects.len(), 1);
        assert!(groups.standalone.is_empty());
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn available_actions_match_source_state_transitions() {
        use ContainerVerb::*;
        let mut row = container("web");
        for (state, expected) in [
            (ContainerState::Running, vec![Restart, Stop, Pause]),
            (ContainerState::Restarting, vec![Restart, Stop]),
            (ContainerState::Paused, vec![Restart, Stop, Unpause]),
            (ContainerState::Created, vec![Start]),
            (ContainerState::Exited, vec![Start]),
            (ContainerState::Dead, vec![Start]),
            (ContainerState::Removing, vec![Start]),
        ] {
            row.state = state;
            assert_eq!(available_container_verbs(&row), expected);
        }
        assert_eq!(Unpause.label(), "Resume");
        assert_eq!(serde_json::to_value(Unpause).unwrap(), json!("unpause"));
        assert_eq!(
            serde_json::to_value(PruneTarget::DanglingImages).unwrap(),
            json!("dangling-images")
        );
    }

    #[test]
    fn validates_identifiers_without_accepting_options_or_shell_fragments() {
        for name in ["a", "Atlas-web_1.2", &"a".repeat(128)] {
            assert!(valid_container(name));
        }
        for name in ["", "-all", "a b", "a;id", "$(id)", "a/b", &"a".repeat(129)] {
            assert!(!valid_container(name));
        }
        for project in ["a", "atlas_web-2", &"a".repeat(256)] {
            assert!(valid_project(project));
        }
        for project in ["", "Atlas", "-app", "app.web", "app/x", &"a".repeat(257)] {
            assert!(!valid_project(project));
        }
    }

    #[test]
    fn secret_names_require_individual_reveal() {
        for name in [
            "PASSWORD",
            "clientSecret",
            "API_TOKEN",
            "public_key",
            "credential",
            "AUTH_URL",
            "private",
            "database_dsn",
            "cookie",
        ] {
            assert!(secret_env_name(name), "{name}");
        }
        for name in ["PATH", "PORT", "HOME", "DATABASE_URL", ""] {
            assert!(!secret_env_name(name));
        }
    }

    #[test]
    fn list_notes_keep_stale_health_and_clock_skew_out_of_the_uptime() {
        let mut row = container("web");
        row.cpu = Some(12.4);
        assert_eq!(list_note(&row, 0), "12%");
        row.cpu = Some(0.42);
        assert_eq!(list_note(&row, 0), "0.4%");
        row.cpu = None;
        row.started_at = Some(10_000);
        assert_eq!(list_note(&row, 0), "0s");
        row = exited(row, 0);
        assert_eq!(list_note(&row, 3_600_000), "1h ago");
        row.exit_code = Some(1);
        assert_eq!(list_note(&row, 0), "exit 1");
    }
}
