//! Explicit wire-to-domain ownership transfer; no JSON or domain dependency in protocol.
use newport_domain::containers as domain;
use newport_protocol::{
    desktop::{self, containers::Reply},
    monitor::containers as wire,
};
use serde_json::{Value, json};

#[derive(Clone, Debug)]
pub(super) enum Response {
    Snapshot(Box<domain::ContainersSnapshot>),
    Inspect(Box<domain::ContainerInspect>),
    Logs(Box<domain::ContainerLogPage>),
    Disk(Box<domain::DockerDisk>),
    Pruned(domain::Pruned),
    Value(Value),
}

impl Response {
    pub(super) fn snapshot(self) -> Result<domain::ContainersSnapshot, String> {
        match self {
            Self::Snapshot(value) => Ok(*value),
            _ => Err("Unexpected Containers response operation.".into()),
        }
    }
    pub(super) fn inspect(self) -> Result<domain::ContainerInspect, String> {
        match self {
            Self::Inspect(value) => Ok(*value),
            _ => Err("Unexpected Containers response operation.".into()),
        }
    }
    pub(super) fn logs(self) -> Result<domain::ContainerLogPage, String> {
        match self {
            Self::Logs(value) => Ok(*value),
            _ => Err("Unexpected Containers response operation.".into()),
        }
    }
    pub(super) fn disk(self) -> Result<domain::DockerDisk, String> {
        match self {
            Self::Disk(value) => Ok(*value),
            _ => Err("Unexpected Containers response operation.".into()),
        }
    }
    pub(super) fn pruned(&self) -> Option<&domain::Pruned> {
        if let Self::Pruned(value) = self {
            Some(value)
        } else {
            None
        }
    }
    pub(super) fn value(self) -> Result<Value, String> {
        match self {
            Self::Value(value) => Ok(value),
            Self::Pruned(value) => Ok(json!({"removed":value.removed,"reclaimed":value.reclaimed})),
            _ => Err("Unexpected Containers response operation.".into()),
        }
    }
}

pub(super) fn convert(value: Reply) -> Result<Response, String> {
    Ok(match value {
        Reply::Snapshot(value) => Response::Snapshot(Box::new(containers_snapshot(*value))),
        Reply::Inspect(value) => Response::Inspect(Box::new(container_inspect(*value))),
        Reply::Logs(value) => Response::Logs(Box::new(container_log_page(*value))),
        Reply::Disk(value) => Response::Disk(Box::new(docker_disk(*value))),
        Reply::Prune(value) => Response::Pruned(pruned(*value)),
        value => Response::Value(
            crate::desktop_result::project(desktop::Reply::Containers(Box::new(value)))
                .map_err(|error| error.to_string())?,
        ),
    })
}

fn container_state(value: wire::ContainerState) -> domain::ContainerState {
    match value {
        wire::ContainerState::Created => domain::ContainerState::Created,
        wire::ContainerState::Running => domain::ContainerState::Running,
        wire::ContainerState::Paused => domain::ContainerState::Paused,
        wire::ContainerState::Restarting => domain::ContainerState::Restarting,
        wire::ContainerState::Removing => domain::ContainerState::Removing,
        wire::ContainerState::Exited => domain::ContainerState::Exited,
        wire::ContainerState::Dead => domain::ContainerState::Dead,
    }
}

fn container_health(value: wire::ContainerHealth) -> domain::ContainerHealth {
    match value {
        wire::ContainerHealth::Healthy => domain::ContainerHealth::Healthy,
        wire::ContainerHealth::Unhealthy => domain::ContainerHealth::Unhealthy,
        wire::ContainerHealth::Starting => domain::ContainerHealth::Starting,
    }
}

fn port_protocol(value: wire::PortProtocol) -> domain::PortProtocol {
    match value {
        wire::PortProtocol::Tcp => domain::PortProtocol::Tcp,
        wire::PortProtocol::Udp => domain::PortProtocol::Udp,
        wire::PortProtocol::Sctp => domain::PortProtocol::Sctp,
    }
}

fn log_stream(value: wire::LogStream) -> domain::LogStream {
    match value {
        wire::LogStream::Stdout => domain::LogStream::Stdout,
        wire::LogStream::Stderr => domain::LogStream::Stderr,
    }
}

fn engine_info(value: wire::EngineInfo) -> domain::EngineInfo {
    domain::EngineInfo {
        version: value.version,
        api_version: value.api_version,
        os: value.os,
        rootless: value.rootless,
        podman: value.podman,
    }
}

fn container_port(value: wire::ContainerPort) -> domain::ContainerPort {
    domain::ContainerPort {
        ip: value.ip,
        host_port: value.host_port,
        container_port: value.container_port,
        protocol: port_protocol(value.protocol),
    }
}

fn compose_labels(value: wire::ComposeLabels) -> domain::ComposeLabels {
    domain::ComposeLabels {
        project: value.project,
        service: value.service,
        oneoff: value.oneoff,
        number: value.number,
        working_dir: value.working_dir,
        config_files: value.config_files,
    }
}

fn container(value: wire::Container) -> domain::Container {
    domain::Container {
        id: value.id,
        name: value.name,
        image: value.image,
        image_id: value.image_id,
        command: value.command,
        state: container_state(value.state),
        status: value.status,
        health: value.health.map(container_health),
        exit_code: value.exit_code,
        oom_killed: value.oom_killed,
        error: value.error,
        restart_count: value.restart_count,
        restart_policy: value.restart_policy,
        created: value.created,
        started_at: value.started_at,
        finished_at: value.finished_at,
        ports: value.ports.into_iter().map(container_port).collect(),
        compose: value.compose.map(compose_labels),
        cpu: value.cpu,
        memory: value.memory,
        memory_limit: value.memory_limit,
    }
}

fn containers_snapshot(value: wire::ContainersSnapshot) -> domain::ContainersSnapshot {
    domain::ContainersSnapshot {
        engine: engine_info(value.engine),
        containers: value.containers.into_iter().map(container).collect(),
        truncated: value.truncated,
    }
}

fn env_var(value: wire::EnvVar) -> domain::EnvVar {
    domain::EnvVar {
        name: value.name,
        value: value.value,
    }
}

fn mount(value: wire::Mount) -> domain::Mount {
    domain::Mount {
        kind: value.kind,
        source: value.source,
        destination: value.destination,
        mode: value.mode,
        rw: value.rw,
        name: value.name,
    }
}

fn network(value: wire::Network) -> domain::Network {
    domain::Network {
        name: value.name,
        ip_address: value.ip_address,
        aliases: value.aliases,
    }
}

fn healthcheck(value: wire::Healthcheck) -> domain::Healthcheck {
    domain::Healthcheck {
        test: value.test,
        interval: value.interval,
    }
}

fn health_result(value: wire::HealthResult) -> domain::HealthResult {
    domain::HealthResult {
        at: value.at,
        exit_code: value.exit_code,
        output: value.output,
    }
}

fn container_inspect(value: wire::ContainerInspect) -> domain::ContainerInspect {
    domain::ContainerInspect {
        id: value.id,
        name: value.name,
        image: value.image,
        created: value.created,
        entrypoint: value.entrypoint,
        cmd: value.cmd,
        working_dir: value.working_dir,
        user: value.user,
        hostname: value.hostname,
        env: value.env.into_iter().map(env_var).collect(),
        mounts: value.mounts.into_iter().map(mount).collect(),
        networks: value.networks.into_iter().map(network).collect(),
        restart_policy: value.restart_policy,
        labels: value.labels,
        healthcheck: value.healthcheck.map(healthcheck),
        last_health: value.last_health.into_iter().map(health_result).collect(),
    }
}

fn container_log_entry(value: wire::ContainerLogEntry) -> domain::ContainerLogEntry {
    domain::ContainerLogEntry {
        at: value.at,
        stream: log_stream(value.stream),
        message: value.message,
        container: value.container,
    }
}

fn container_log_page(value: wire::ContainerLogPage) -> domain::ContainerLogPage {
    domain::ContainerLogPage {
        entries: value.entries.into_iter().map(container_log_entry).collect(),
        cursor: value.cursor,
    }
}

fn docker_image(value: wire::DockerImage) -> domain::DockerImage {
    domain::DockerImage {
        id: value.id,
        tags: value.tags,
        size: value.size,
        shared_size: value.shared_size,
        created: value.created,
        containers: value.containers,
        dangling: value.dangling,
    }
}

fn docker_volume(value: wire::DockerVolume) -> domain::DockerVolume {
    domain::DockerVolume {
        name: value.name,
        driver: value.driver,
        size: value.size,
        ref_count: value.ref_count,
        anonymous: value.anonymous,
        compose: value.compose,
    }
}

fn disk_totals(value: wire::DiskTotals) -> domain::DiskTotals {
    domain::DiskTotals {
        count: value.count,
        size: value.size,
        reclaimable: value.reclaimable,
    }
}

fn docker_totals(value: wire::DockerTotals) -> domain::DockerTotals {
    domain::DockerTotals {
        images: disk_totals(value.images),
        containers: disk_totals(value.containers),
        volumes: disk_totals(value.volumes),
        build_cache: disk_totals(value.build_cache),
    }
}

fn docker_disk(value: wire::DockerDisk) -> domain::DockerDisk {
    domain::DockerDisk {
        images: value.images.into_iter().map(docker_image).collect(),
        volumes: value.volumes.into_iter().map(docker_volume).collect(),
        totals: docker_totals(value.totals),
        volumes_measured: value.volumes_measured,
    }
}

fn pruned(value: wire::Pruned) -> domain::Pruned {
    domain::Pruned {
        removed: value.removed,
        reclaimed: value.reclaimed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_preserves_large_counters_missing_observations_and_variants() {
        let value = wire::ContainersSnapshot {
            engine: wire::EngineInfo {
                version: "v".into(),
                api_version: "a".into(),
                os: "linux".into(),
                rootless: true,
                podman: true,
            },
            containers: vec![wire::Container {
                id: "a".repeat(64),
                name: "one".into(),
                state: wire::ContainerState::Restarting,
                health: Some(wire::ContainerHealth::Unhealthy),
                exit_code: Some(-1),
                restart_count: u64::MAX,
                memory: Some(u64::MAX),
                cpu: None,
                memory_limit: None,
                ports: vec![wire::ContainerPort {
                    ip: None,
                    host_port: Some(1),
                    container_port: 2,
                    protocol: wire::PortProtocol::Sctp,
                }],
                ..Default::default()
            }],
            truncated: true,
        };
        let decoded = convert(Reply::Snapshot(Box::new(value)))
            .unwrap()
            .snapshot()
            .unwrap();
        assert!(decoded.engine.rootless && decoded.engine.podman && decoded.truncated);
        let container = &decoded.containers[0];
        assert_eq!(container.restart_count, u64::MAX);
        assert_eq!(container.memory, Some(u64::MAX));
        assert_eq!(container.exit_code, Some(-1));
        assert_eq!(container.state, domain::ContainerState::Restarting);
        assert_eq!(container.health, Some(domain::ContainerHealth::Unhealthy));
        assert_eq!(container.ports[0].protocol, domain::PortProtocol::Sctp);
        assert!(container.cpu.is_none() && container.memory_limit.is_none());
    }

    #[test]
    fn disk_and_logs_keep_unknowns_and_typed_receipts() {
        let disk = wire::DockerDisk {
            volumes: vec![wire::DockerVolume {
                name: "unknown".into(),
                size: None,
                ref_count: Some(0),
                ..Default::default()
            }],
            images: Vec::new(),
            totals: wire::DockerTotals::default(),
            volumes_measured: false,
        };
        let disk = convert(Reply::Disk(Box::new(disk)))
            .unwrap()
            .disk()
            .unwrap();
        assert!(!disk.volumes_measured);
        assert_eq!(disk.volumes[0].size, None);
        assert_eq!(disk.volumes[0].ref_count, Some(0));
        let logs = wire::ContainerLogPage {
            entries: vec![wire::ContainerLogEntry {
                at: None,
                stream: wire::LogStream::Stderr,
                message: "line".into(),
                container: None,
            }],
            cursor: Some("cursor".into()),
        };
        let logs = convert(Reply::Logs(Box::new(logs)))
            .unwrap()
            .logs()
            .unwrap();
        assert_eq!(logs.entries[0].stream, domain::LogStream::Stderr);
        assert!(logs.entries[0].at.is_none());
        assert_eq!(logs.cursor.as_deref(), Some("cursor"));
        let pruned = convert(Reply::Prune(Box::new(wire::Pruned {
            removed: 1,
            reclaimed: u64::MAX,
        })))
        .unwrap();
        assert_eq!(pruned.pruned().unwrap().reclaimed, u64::MAX);
        assert!(pruned.snapshot().is_err());
    }
}
