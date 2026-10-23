//! Bounded messages for a waiting local GUI launch. Arguments stay in the process.
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io};

pub const MAX_MESSAGE: usize = 32 * 1024;
pub const MAX_PENDING: usize = 32;
pub const WAIT_SECONDS: u64 = 15 * 60;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub id: uuid::Uuid,
    pub terminal: uuid::Uuid,
    pub application: String,
}
impl Request {
    pub fn validate(&self) -> io::Result<()> {
        if self.id.is_nil()
            || self.terminal.is_nil()
            || self.application.is_empty()
            || self.application.len() > 256
            || self.application.chars().any(char::is_control)
        {
            return Err(io::Error::other("Invalid GUI launch request"));
        }
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    pub id: uuid::Uuid,
    pub environment: Option<BTreeMap<String, Option<String>>>,
    pub error: Option<String>,
}

pub fn encode(value: &impl Serialize) -> io::Result<Vec<u8>> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > MAX_MESSAGE {
        return Err(io::Error::other("GUI message too large"));
    }
    Ok(bytes)
}
pub fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> io::Result<T> {
    if bytes.len() > MAX_MESSAGE {
        return Err(io::Error::other("GUI message too large"));
    }
    Ok(serde_json::from_slice(bytes)?)
}
