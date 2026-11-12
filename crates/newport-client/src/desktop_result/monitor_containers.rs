//! Explicit monitor reply fields at the UI rendering boundary.
use newport_protocol::monitor::containers::*;
use serde_json::{Value, json};
pub(super) fn engine_info(value: EngineInfo) -> Value {
    json!({"version":value.version, "apiVersion":value.api_version, "os":value.os, "rootless":value.rootless, "podman":value.podman})
}
fn container_state(value: ContainerState) -> &'static str {
    match value {
        ContainerState::Created => "created",
        ContainerState::Running => "running",
        ContainerState::Paused => "paused",
        ContainerState::Restarting => "restarting",
        ContainerState::Removing => "removing",
        ContainerState::Exited => "exited",
        ContainerState::Dead => "dead",
    }
}
fn container_health(value: ContainerHealth) -> &'static str {
    match value {
        ContainerHealth::Healthy => "healthy",
        ContainerHealth::Unhealthy => "unhealthy",
        ContainerHealth::Starting => "starting",
    }
}
fn port_protocol(value: PortProtocol) -> &'static str {
    match value {
        PortProtocol::Tcp => "tcp",
        PortProtocol::Udp => "udp",
        PortProtocol::Sctp => "sctp",
    }
}
pub(super) fn container_port(value: ContainerPort) -> Value {
    json!({"ip":value.ip, "hostPort":value.host_port, "containerPort":value.container_port, "protocol":port_protocol(value.protocol)})
}
pub(super) fn compose_labels(value: ComposeLabels) -> Value {
    json!({"project":value.project, "service":value.service, "oneoff":value.oneoff, "number":value.number, "workingDir":value.working_dir, "configFiles":value.config_files})
}
pub(super) fn container(value: Container) -> Value {
    json!({"id":value.id, "name":value.name, "image":value.image, "imageId":value.image_id, "command":value.command, "state":container_state(value.state), "status":value.status, "health":value.health.map(container_health), "exitCode":value.exit_code, "oomKilled":value.oom_killed, "error":value.error, "restartCount":value.restart_count, "restartPolicy":value.restart_policy, "created":value.created, "startedAt":value.started_at, "finishedAt":value.finished_at, "ports":value.ports.into_iter().map(container_port).collect::<Vec<_>>(), "compose":value.compose.map(compose_labels), "cpu":value.cpu, "memory":value.memory, "memoryLimit":value.memory_limit})
}
pub(super) fn containers_snapshot(value: ContainersSnapshot) -> Value {
    json!({"engine":engine_info(value.engine), "containers":value.containers.into_iter().map(container).collect::<Vec<_>>(), "truncated":value.truncated})
}
pub(super) fn env_var(value: EnvVar) -> Value {
    json!({"name":value.name, "value":value.value})
}
pub(super) fn mount(value: Mount) -> Value {
    json!({"type":value.kind, "source":value.source, "destination":value.destination, "mode":value.mode, "rw":value.rw, "name":value.name})
}
pub(super) fn network(value: Network) -> Value {
    json!({"name":value.name, "ipAddress":value.ip_address, "aliases":value.aliases})
}
pub(super) fn healthcheck(value: Healthcheck) -> Value {
    json!({"test":value.test, "interval":value.interval})
}
pub(super) fn health_result(value: HealthResult) -> Value {
    json!({"at":value.at, "exitCode":value.exit_code, "output":value.output})
}
pub(super) fn container_inspect(value: ContainerInspect) -> Value {
    json!({"id":value.id, "name":value.name, "image":value.image, "created":value.created, "entrypoint":value.entrypoint, "cmd":value.cmd, "workingDir":value.working_dir, "user":value.user, "hostname":value.hostname, "env":value.env.into_iter().map(env_var).collect::<Vec<_>>(), "mounts":value.mounts.into_iter().map(mount).collect::<Vec<_>>(), "networks":value.networks.into_iter().map(network).collect::<Vec<_>>(), "restartPolicy":value.restart_policy, "labels":value.labels, "healthcheck":value.healthcheck.map(healthcheck), "lastHealth":value.last_health.into_iter().map(health_result).collect::<Vec<_>>()})
}
fn log_stream(value: LogStream) -> &'static str {
    match value {
        LogStream::Stdout => "stdout",
        LogStream::Stderr => "stderr",
    }
}
pub(super) fn container_log_entry(value: ContainerLogEntry) -> Value {
    json!({"at":value.at, "stream":log_stream(value.stream), "message":value.message, "container":value.container})
}
pub(super) fn container_log_page(value: ContainerLogPage) -> Value {
    json!({"entries":value.entries.into_iter().map(container_log_entry).collect::<Vec<_>>(), "cursor":value.cursor})
}
pub(super) fn docker_image(value: DockerImage) -> Value {
    json!({"id":value.id, "tags":value.tags, "size":value.size, "sharedSize":value.shared_size, "created":value.created, "containers":value.containers, "dangling":value.dangling})
}
pub(super) fn docker_volume(value: DockerVolume) -> Value {
    json!({"name":value.name, "driver":value.driver, "size":value.size, "refCount":value.ref_count, "anonymous":value.anonymous, "compose":value.compose})
}
pub(super) fn disk_totals(value: DiskTotals) -> Value {
    json!({"count":value.count, "size":value.size, "reclaimable":value.reclaimable})
}
pub(super) fn docker_totals(value: DockerTotals) -> Value {
    json!({"images":disk_totals(value.images), "containers":disk_totals(value.containers), "volumes":disk_totals(value.volumes), "buildCache":disk_totals(value.build_cache)})
}
pub(super) fn docker_disk(value: DockerDisk) -> Value {
    json!({"images":value.images.into_iter().map(docker_image).collect::<Vec<_>>(), "volumes":value.volumes.into_iter().map(docker_volume).collect::<Vec<_>>(), "totals":docker_totals(value.totals), "volumesMeasured":value.volumes_measured})
}
pub(super) fn pruned(value: Pruned) -> Value {
    json!({"removed":value.removed, "reclaimed":value.reclaimed})
}
pub(super) fn project(reply: newport_protocol::desktop::containers::Reply) -> Value {
    use newport_protocol::desktop::containers::Reply;
    match reply {
        Reply::Snapshot(value) => containers_snapshot(*value),
        Reply::Inspect(value) => container_inspect(*value),
        Reply::Logs(value) => container_log_page(*value),
        Reply::Disk(value) => docker_disk(*value),
        Reply::Prune(value) => pruned(*value),
        Reply::Action(value) | Reply::ProjectAction(value) => json!({"ok":value.ok}),
        Reply::Remove(value) => json!({"removed":value.removed}),
        Reply::ComposeProject(value) => super::monitor_compose::project(*value),
        Reply::ComposeRun(value) => super::monitor_compose::started(*value),
        Reply::ComposeJob(value) => super::monitor_compose::job(*value),
        Reply::ComposeJobs(value) => super::monitor_compose::jobs(*value),
        Reply::ComposeCancel(value) => super::monitor_compose::done(*value),
    }
}
