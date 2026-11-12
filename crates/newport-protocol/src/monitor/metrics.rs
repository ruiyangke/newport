//! Sampled CPU, memory, filesystems, network counters and process observations.
use super::Invalid;
use serde::{Deserialize, Serialize};

pub const MAX_CPUS: usize = 1024;
pub const MAX_INTERFACES: usize = 1024;
pub const MAX_DISKS: usize = 256;
pub const MAX_PROCESSES: usize = 100;
pub const MAX_HISTORY: usize = 60;
pub const MAX_TEXT: usize = 4096;
pub const MAX_COMMAND: usize = 512;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub hostname: String,
    pub os: String,
    pub kernel: String,
    pub cores: u32,
    pub uptime: f64,
    pub load: [f64; 3],
    pub cpu: f64,
    pub cpus: Vec<Core>,
    pub memory_total: u64,
    pub memory_used: u64,
    pub swap_total: u64,
    pub swap_used: u64,
    pub disks: Vec<Disk>,
    pub network: Vec<Network>,
    pub processes: Vec<Process>,
    pub process_count: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Core {
    pub id: u32,
    pub usage: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<f64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Disk {
    pub mount: String,
    pub device: String,
    pub total: u64,
    pub used: u64,
    /// Filesystems can report a negative amount available to this user.
    pub available: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Network {
    pub name: String,
    pub received: u64,
    pub sent: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Process {
    pub pid: u32,
    pub user: String,
    /// Percent of one core; a process may use more than one core.
    pub cpu: f64,
    pub memory: u64,
    pub state: String,
    pub name: String,
    /// Seconds since start. Zero is a known age, not a missing observation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elapsed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

fn text(value: &str, limit: usize) -> bool {
    value.len() <= limit && !value.contains('\0')
}
fn nonnegative(value: f64) -> bool {
    value.is_finite() && value >= 0.0
}
fn percent(value: f64) -> bool {
    nonnegative(value) && value <= 100.0
}

impl Snapshot {
    pub fn validate(&self) -> Result<(), Invalid> {
        if self.cores == 0
            || self.cores > MAX_CPUS as u32
            || self.cpus.len() > MAX_CPUS
            || self.disks.len() > MAX_DISKS
            || self.network.len() > MAX_INTERFACES
            || self.processes.len() > MAX_PROCESSES
        {
            return Err(Invalid("metrics count"));
        }
        if !nonnegative(self.uptime)
            || !self.load.iter().all(|value| nonnegative(*value))
            || !percent(self.cpu)
            || self.memory_used > self.memory_total
            || self.swap_used > self.swap_total
        {
            return Err(Invalid("metrics value"));
        }
        if [&self.hostname, &self.os, &self.kernel]
            .into_iter()
            .any(|value| !text(value, MAX_TEXT))
        {
            return Err(Invalid("host description"));
        }
        let mut cores = std::collections::BTreeSet::new();
        for core in &self.cpus {
            if !cores.insert(core.id)
                || !percent(core.usage)
                || core.history.len() > MAX_HISTORY
                || !core.history.iter().all(|value| percent(*value))
            {
                return Err(Invalid("CPU observation"));
            }
        }
        for disk in &self.disks {
            if !text(&disk.mount, MAX_TEXT)
                || !text(&disk.device, MAX_TEXT)
                || disk.used > disk.total
            {
                return Err(Invalid("filesystem observation"));
            }
        }
        for interface in &self.network {
            if !text(&interface.name, MAX_TEXT) {
                return Err(Invalid("network observation"));
            }
        }
        let mut pids = std::collections::BTreeSet::new();
        for process in &self.processes {
            if process.pid == 0
                || !pids.insert(process.pid)
                || !nonnegative(process.cpu)
                || [&process.user, &process.state, &process.name]
                    .into_iter()
                    .any(|value| !text(value, MAX_TEXT))
                || process
                    .command
                    .as_ref()
                    .is_some_and(|value| !text(value, MAX_COMMAND))
            {
                return Err(Invalid("process observation"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot() -> Snapshot {
        Snapshot {
            cores: 2,
            cpus: vec![Core {
                id: 7,
                usage: 100.0,
                history: vec![0.0, 100.0],
            }],
            processes: vec![Process {
                pid: 8,
                cpu: 200.0,
                elapsed: Some(0),
                ..Default::default()
            }],
            disks: vec![Disk {
                total: 1,
                available: -1,
                ..Default::default()
            }],
            ..Default::default()
        }
    }
    #[test]
    fn metrics_preserve_unknown_and_zero_age_and_multicore_usage() {
        let mut value = snapshot();
        value.validate().unwrap();
        let encoded = serde_json::to_value(&value).unwrap();
        assert_eq!(encoded["processes"][0]["elapsed"], 0);
        assert_eq!(encoded["processes"][0]["cpu"], 200.0);
        assert_eq!(encoded["disks"][0]["available"], -1);
        value.processes[0].elapsed = None;
        assert!(
            serde_json::to_value(value).unwrap()["processes"][0]
                .get("elapsed")
                .is_none()
        );
    }
    #[test]
    fn nonfinite_invalid_and_oversized_observations_are_rejected() {
        for invalid in [f64::NAN, f64::INFINITY, -1.0] {
            let mut value = snapshot();
            value.uptime = invalid;
            assert!(value.validate().is_err());
            value.uptime = 0.0;
            value.processes[0].cpu = invalid;
            assert!(value.validate().is_err());
        }
        let mut value = snapshot();
        value.cpus.push(value.cpus[0].clone());
        assert!(value.validate().is_err());
        let mut value = snapshot();
        value.cpus[0].history = vec![1.0; MAX_HISTORY + 1];
        assert!(value.validate().is_err());
        let mut value = snapshot();
        value.processes[0].command = Some("x".repeat(MAX_COMMAND + 1));
        assert!(value.validate().is_err());
        let mut value = snapshot();
        value.memory_used = 1;
        assert!(value.validate().is_err());
    }
}
