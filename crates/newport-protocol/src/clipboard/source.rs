//! Read-only clipboard source served by an integration session. Requests are
//! single-use and carry no integration demand request identity.
use super::{Chunk, imports};
use crate::common;
use std::fmt;
#[cfg(feature = "protobuf")]
pub mod wire;
pub const VERSION: u32 = 1;
pub const CLIENT_LIMITS: common::Limits = common::Limits {
    message: 1024,
    metadata: 1024,
};
pub const SERVER_LIMITS: common::Limits = common::Limits {
    message: 72 * 1024,
    metadata: 72 * 1024,
};
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    Offer,
    Read { revision: i64, format: String },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Response {
    Hello { version: u32 },
    Offer(imports::Offer),
    Chunk(Chunk),
    Failed(common::Error),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Invalid(pub &'static str);
impl fmt::Display for Invalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Invalid clipboard source {}", self.0)
    }
}
impl std::error::Error for Invalid {}
impl Request {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Read { format, .. } if !imports::valid_format(format) => Err(Invalid("format")),
            _ => Ok(()),
        }
    }
}
impl Response {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Hello { version } if *version != VERSION => Err(Invalid("version")),
            Self::Offer(value) => value.validate().map_err(|_| Invalid("offer")),
            Self::Chunk(value)
                if value.bytes.len() > imports::CHUNK || (value.bytes.is_empty() && !value.eof) =>
            {
                Err(Invalid("chunk"))
            }
            Self::Failed(value)
                if value.code.is_empty()
                    || value.code.len() > 64
                    || value.message.len() > 4096
                    || value.message.contains('\0') =>
            {
                Err(Invalid("failure"))
            }
            _ => Ok(()),
        }
    }
}
