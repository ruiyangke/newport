//! Shared Git requests, results and flow control, independent of the Git executor.
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

/// Error codes one side of the Git service sends and another acts on.
pub mod code {
    pub const INVALID_REQUEST: &str = "INVALID_REQUEST";
    pub const TRANSPORT_ERROR: &str = "TRANSPORT_ERROR";
    pub const PROTOCOL_ERROR: &str = "PROTOCOL_ERROR";
    pub const UNSUPPORTED_METHOD: &str = "UNSUPPORTED_METHOD";
    /// The repository moved or was deleted.
    pub const REPO_NOT_FOUND: &str = "REPO_NOT_FOUND";
    /// The opened checkout is gone, though its repository may remain.
    pub const CHECKOUT_UNAVAILABLE: &str = "CHECKOUT_UNAVAILABLE";
    /// The repository changed since the snapshot the request names.
    pub const STALE_SNAPSHOT: &str = "STALE_SNAPSHOT";
    /// The file the request names is no longer among the changes.
    pub const STALE_ENTRY: &str = "STALE_ENTRY";
    /// A write may or may not have happened.
    pub const OUTCOME_UNKNOWN: &str = "OUTCOME_UNKNOWN";
    pub const OPERATION_NOT_FOUND: &str = "OPERATION_NOT_FOUND";
    pub const OPERATION_NOT_REVIEWABLE: &str = "OPERATION_NOT_REVIEWABLE";
    /// Committing needs a name and email.
    pub const IDENTITY_REQUIRED: &str = "IDENTITY_REQUIRED";
    pub const ALREADY_REPOSITORY: &str = "ALREADY_REPOSITORY";
    pub const DIRECTORY_REQUIRED: &str = "DIRECTORY_REQUIRED";
    pub const PATH_EXISTS: &str = "PATH_EXISTS";
    pub const PATH_CHANGED: &str = "PATH_CHANGED";
}

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
        Self::new(code::INVALID_REQUEST, message)
    }
    pub fn transport(message: impl Into<String>) -> Self {
        Self::new(code::TRANSPORT_ERROR, message)
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for Error {}
