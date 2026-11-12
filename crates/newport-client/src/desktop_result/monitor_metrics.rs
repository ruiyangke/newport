//! Explicit monitor reply fields at the UI rendering boundary.
use newport_protocol::monitor::metrics::*;
use serde_json::{Value, json};
pub(super) fn core(value: Core) -> Value {
    json!({"id":value.id, "usage":value.usage, "history":value.history})
}
pub(super) fn disk(value: Disk) -> Value {
    json!({"mount":value.mount, "device":value.device, "total":value.total, "used":value.used, "available":value.available})
}
pub(super) fn network(value: Network) -> Value {
    json!({"name":value.name, "received":value.received, "sent":value.sent})
}
pub(super) fn process(value: Process) -> Value {
    json!({"pid":value.pid, "user":value.user, "cpu":value.cpu, "memory":value.memory, "state":value.state, "name":value.name, "elapsed":value.elapsed, "command":value.command})
}
pub(super) fn snapshot(value: Snapshot) -> Value {
    json!({"hostname":value.hostname, "os":value.os, "kernel":value.kernel, "cores":value.cores, "uptime":value.uptime, "load":value.load, "cpu":value.cpu, "cpus":value.cpus.into_iter().map(core).collect::<Vec<_>>(), "memoryTotal":value.memory_total, "memoryUsed":value.memory_used, "swapTotal":value.swap_total, "swapUsed":value.swap_used, "disks":value.disks.into_iter().map(disk).collect::<Vec<_>>(), "network":value.network.into_iter().map(network).collect::<Vec<_>>(), "processes":value.processes.into_iter().map(process).collect::<Vec<_>>(), "processCount":value.process_count})
}
