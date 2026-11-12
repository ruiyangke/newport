use super::super::metrics as contract;
use super::{Invalid, Record, pb};

impl Record for contract::Snapshot {
    type Wire = pb::MetricsSnapshot;
    fn to_wire(&self) -> Self::Wire {
        Self::Wire {
            hostname: self.hostname.clone(),
            os: self.os.clone(),
            kernel: self.kernel.clone(),
            cores: self.cores,
            uptime: self.uptime,
            load: self.load.to_vec(),
            cpu: self.cpu,
            cpus: self.cpus.iter().map(Record::to_wire).collect(),
            memory_total: self.memory_total,
            memory_used: self.memory_used,
            swap_total: self.swap_total,
            swap_used: self.swap_used,
            disks: self.disks.iter().map(Record::to_wire).collect(),
            network: self.network.iter().map(Record::to_wire).collect(),
            processes: self.processes.iter().map(Record::to_wire).collect(),
            process_count: self.process_count,
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            hostname: value.hostname,
            os: value.os,
            kernel: value.kernel,
            cores: value.cores,
            uptime: value.uptime,
            load: value
                .load
                .try_into()
                .map_err(|_| Invalid("metrics load count"))?,
            cpu: value.cpu,
            cpus: value
                .cpus
                .into_iter()
                .map(contract::Core::from_wire)
                .collect::<Result<_, _>>()?,
            memory_total: value.memory_total,
            memory_used: value.memory_used,
            swap_total: value.swap_total,
            swap_used: value.swap_used,
            disks: value
                .disks
                .into_iter()
                .map(contract::Disk::from_wire)
                .collect::<Result<_, _>>()?,
            network: value
                .network
                .into_iter()
                .map(contract::Network::from_wire)
                .collect::<Result<_, _>>()?,
            processes: value
                .processes
                .into_iter()
                .map(contract::Process::from_wire)
                .collect::<Result<_, _>>()?,
            process_count: value.process_count,
        })
    }
}

impl Record for contract::Core {
    type Wire = pb::MetricsCore;
    fn to_wire(&self) -> Self::Wire {
        Self::Wire {
            id: self.id,
            usage: self.usage,
            history: self.history.clone(),
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            id: value.id,
            usage: value.usage,
            history: value.history,
        })
    }
}

impl Record for contract::Disk {
    type Wire = pb::MetricsDisk;
    fn to_wire(&self) -> Self::Wire {
        Self::Wire {
            mount: self.mount.clone(),
            device: self.device.clone(),
            total: self.total,
            used: self.used,
            available: self.available,
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            mount: value.mount,
            device: value.device,
            total: value.total,
            used: value.used,
            available: value.available,
        })
    }
}

impl Record for contract::Network {
    type Wire = pb::MetricsNetwork;
    fn to_wire(&self) -> Self::Wire {
        Self::Wire {
            name: self.name.clone(),
            received: self.received,
            sent: self.sent,
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            name: value.name,
            received: value.received,
            sent: value.sent,
        })
    }
}

impl Record for contract::Process {
    type Wire = pb::MetricsProcess;
    fn to_wire(&self) -> Self::Wire {
        Self::Wire {
            pid: self.pid,
            user: self.user.clone(),
            cpu: self.cpu,
            memory: self.memory,
            state: self.state.clone(),
            name: self.name.clone(),
            elapsed: self.elapsed,
            command: self.command.clone(),
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            pid: value.pid,
            user: value.user,
            cpu: value.cpu,
            memory: value.memory,
            state: value.state,
            name: value.name,
            elapsed: value.elapsed,
            command: value.command,
        })
    }
}
