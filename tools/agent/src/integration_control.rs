//! Versioned, bounded integration control messages shared with Newport.
use serde::{Deserialize, Serialize};
use std::io;

pub const PROTOCOL: &str = "newport.integration";
pub const VERSION: u32 = 1;
pub const LIMIT: usize = 8192;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Hello {
    pub protocol: String,
    pub versions: Vec<u32>,
    pub instance_id: String,
    pub live_permissions: bool,
    pub max_control_bytes: usize,
}
impl Hello {
    pub fn new(instance_id: String) -> Self {
        Self {
            protocol: PROTOCOL.into(),
            versions: vec![VERSION],
            instance_id,
            live_permissions: true,
            max_control_bytes: LIMIT,
        }
    }
    pub fn validate(&self) -> io::Result<()> {
        if self.protocol != PROTOCOL
            || !self.versions.contains(&VERSION)
            || !self.live_permissions
            || self.max_control_bytes != LIMIT
            || uuid::Uuid::parse_str(&self.instance_id).is_err()
        {
            return Err(io::Error::other(
                "Needle integration protocol is incompatible. Update Needle.",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Permissions {
    pub revision: u64,
    pub clipboard: bool,
    pub browser: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Applied {
    pub permissions: Permissions,
    pub error: Option<String>,
}

pub fn encode<T: Serialize>(value: &T) -> io::Result<Vec<u8>> {
    let bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    if bytes.len() > LIMIT {
        return Err(io::Error::other("control message exceeds limit"));
    }
    Ok(bytes)
}
pub fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> io::Result<T> {
    if bytes.len() > LIMIT {
        return Err(io::Error::other("control message exceeds limit"));
    }
    serde_json::from_slice(bytes).map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn handshake_requires_compatible_protocol_and_bounded_messages() {
        let mut hello = Hello::new(uuid::Uuid::new_v4().to_string());
        decode::<Hello>(&encode(&hello).unwrap())
            .unwrap()
            .validate()
            .unwrap();
        hello.versions = vec![VERSION + 1];
        assert!(hello.validate().is_err());
        assert!(decode::<Permissions>(&vec![b' '; LIMIT + 1]).is_err());
        let unknown = br#"{"revision":1,"clipboard":true,"browser":false,"unexpected":1}"#;
        assert!(decode::<Permissions>(unknown).is_err());
    }
}
