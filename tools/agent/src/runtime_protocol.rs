//! Small control frames; feature data never travels on this channel.
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::{self, Read, Write},
};

pub const PROTOCOL: &str = "newport.runtime";
pub const VERSION: u32 = 1;
#[path = "integration_control.rs"]
pub mod integration;
pub const MAX_WORKERS: usize = 16;
pub const MAX_REGISTRATIONS: usize = 4096;
pub const MAX_FRAME: usize = 16 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Capability {
    pub versions: Vec<u32>,
    pub available: bool,
    pub reason: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Limits {
    pub max_control_bytes: usize,
    pub max_inflight: usize,
    pub max_workers: usize,
    pub max_registrations: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Hello {
    pub protocol: String,
    pub versions: Vec<u32>,
    pub instance_id: String,
    pub build_sha256: String,
    pub worker_socket: String,
    pub capabilities: BTreeMap<String, Capability>,
    pub dependencies: BTreeMap<String, bool>,
    pub limits: Limits,
}
impl Hello {
    pub fn validate(&self) -> io::Result<()> {
        if self.protocol != PROTOCOL
            || !self.versions.contains(&VERSION)
            || self.versions.len() > 8
            || uuid::Uuid::parse_str(&self.instance_id).is_err()
            || !valid_build(&self.build_sha256)
            || self.limits.max_control_bytes != MAX_FRAME
            || self.limits.max_inflight != 1
            || self.limits.max_workers != MAX_WORKERS
            || self.limits.max_registrations != MAX_REGISTRATIONS
            || !self.worker_socket.starts_with('/')
            || self.worker_socket.len() > 1024
            || self.worker_socket.contains('\0')
            || self.capabilities.len() > 16
            || self.dependencies.len() > 16
        {
            return Err(io::Error::other(
                "Needle runtime protocol is incompatible. Update Needle.",
            ));
        }
        for (name, capability) in &self.capabilities {
            if name.is_empty()
                || name.len() > 64
                || capability.versions.is_empty()
                || capability.versions.len() > 8
                || capability.reason.as_ref().is_some_and(|s| s.len() > 512)
            {
                return Err(io::Error::other("Invalid Needle capability description."));
            }
        }
        Ok(())
    }
    pub fn require(&self, name: &str, version: u32) -> io::Result<()> {
        let capability = self.capabilities.get(name).ok_or_else(|| {
            io::Error::other(format!("Needle does not support {name}. Update Needle."))
        })?;
        if !capability.versions.contains(&version) {
            return Err(io::Error::other(format!(
                "Needle {name} protocol is incompatible. Update Needle."
            )));
        }
        if !capability.available {
            return Err(io::Error::other(
                capability
                    .reason
                    .clone()
                    .unwrap_or_else(|| format!("Needle {name} is unavailable.")),
            ));
        }
        Ok(())
    }
}
pub fn valid_build(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit())
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "method",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Operation {
    Initialize {
        version: u32,
        session_id: String,
        transport_generation: u64,
    },
    Register {
        worker_id: String,
        service: String,
        version: u32,
    },
    Permissions {
        worker_id: String,
        permissions: integration::Permissions,
    },
    Ping,
    Shutdown,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Request {
    pub id: String,
    pub runtime_id: String,
    pub operation: Operation,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Reply {
    pub applied: Option<integration::Applied>,
    pub id: String,
    pub runtime_id: String,
    pub error: Option<String>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Attach {
    pub runtime_id: String,
    pub worker_id: String,
    pub build_sha256: String,
    pub service: String,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Attached {
    pub session_id: Option<String>,
    pub runtime_id: String,
    pub worker_id: String,
    pub error: Option<String>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkerPermissions {
    pub runtime_id: String,
    pub worker_id: String,
    pub permissions: integration::Permissions,
}

pub fn encode<T: Serialize>(value: &T) -> io::Result<Vec<u8>> {
    let bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    if bytes.is_empty() || bytes.len() > MAX_FRAME {
        return Err(io::Error::other("Runtime control frame exceeds limit"));
    }
    Ok(bytes)
}
pub fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> io::Result<T> {
    if bytes.is_empty() || bytes.len() > MAX_FRAME {
        return Err(io::Error::other("Runtime control frame exceeds limit"));
    }
    serde_json::from_slice(bytes).map_err(io::Error::other)
}
pub fn read<T: serde::de::DeserializeOwned>(input: &mut impl Read) -> io::Result<T> {
    let mut header = [0; 4];
    input.read_exact(&mut header)?;
    let length = u32::from_be_bytes(header) as usize;
    if length == 0 || length > MAX_FRAME {
        return Err(io::Error::other("Runtime control frame exceeds limit"));
    }
    let mut bytes = vec![0; length];
    input.read_exact(&mut bytes)?;
    decode(&bytes)
}
pub fn write<T: Serialize>(output: &mut impl Write, value: &T) -> io::Result<()> {
    let bytes = encode(value)?;
    output.write_all(&(bytes.len() as u32).to_be_bytes())?;
    output.write_all(&bytes)?;
    output.flush()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lengths_are_rejected_before_payload_allocation() {
        assert!(read::<Reply>(&mut &(u32::MAX.to_be_bytes())[..]).is_err());
        assert!(read::<Reply>(&mut &[0; 4][..]).is_err());
        assert!(decode::<Reply>(&vec![b' '; MAX_FRAME + 1]).is_err());
        assert!(!valid_build("not a build"));
    }
}
