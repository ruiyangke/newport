//! Values shared by independently implemented endpoints.
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Retry {
    #[default]
    Unknown,
    Yes,
    No,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Error {
    pub code: String,
    pub message: String,
    pub retry: Retry,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for Error {}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Present {}

/// Opaque credential bytes. Debug output never reveals their contents.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Secret {
    pub value: Bytes,
}
impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

/// Byte budgets for one direction of a protocol.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub message: usize,
    pub metadata: usize,
}

pub(crate) fn valid_contract_id(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
