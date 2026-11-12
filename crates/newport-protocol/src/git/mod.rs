//! Shared Git requests, results and flow control, independent of the Git executor.
mod projection;
pub use projection::Projection;
mod request;
mod result;
mod session;
mod validate;
#[cfg(feature = "protobuf")]
pub mod wire;
pub use request::*;
pub use result::*;
use serde::{Deserialize, Serialize};
pub use session::*;
pub use validate::Invalid;
#[cfg(test)]
mod tests;

pub const VERSION: u32 = 1;
pub const MAX_FRAME: usize = 1024 * 1024;
pub const MAX_DIFF: usize = 20 * 1024 * 1024;
pub const MAX_PATH: usize = 4096;
pub const MAX_TEXT: usize = 65536;
pub const CHUNK_SIZE: usize = 64 * 1024;
pub const STREAM_WINDOW: u32 = 8;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Error {
    pub code: String,
    pub message: String,
    pub retry: String,
}
impl Error {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retry: "never".into(),
        }
    }
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new("INVALID_REQUEST", message)
    }
    pub fn transport(message: impl Into<String>) -> Self {
        Self::new("TRANSPORT_ERROR", message)
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for Error {}
