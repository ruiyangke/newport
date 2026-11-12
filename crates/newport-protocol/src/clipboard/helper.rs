//! Native Mac clipboard helper operations, independent of the native owner.
use crate::common;
use std::fmt;
#[cfg(feature = "protobuf")]
pub mod wire;
pub const VERSION: u32 = 1;
pub const CHUNK: usize = 64 * 1024;
pub const MAX_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_FORMATS: usize = 256;
pub const MAX_METADATA: usize = 128 * 1024;
pub const LIMITS: common::Limits = common::Limits {
    message: MAX_METADATA + CHUNK + 1024,
    metadata: MAX_METADATA,
};
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Board {
    General {},
    /// Only unique disposable fixture names are accepted by the helper.
    Test {
        name: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Hello {
        version: u32,
        board: Board,
    },
    Publish {
        generation: u64,
        expected_revision: i64,
        formats: Vec<String>,
    },
    /// Replace the current publication's promises with the files in `folder`
    /// (a folder relative to the clipboard file cache, not a path list, to
    /// stay under the metadata budget).
    AttachFiles {
        generation: u64,
        expected_revision: i64,
        folder: String,
    },
    Revoke {
        generation: u64,
    },
    Data {
        generation: u64,
        id: u64,
        offset: u64,
        eof: bool,
    },
    Failed {
        generation: u64,
        id: u64,
    },
    ReadLocal {
        id: u64,
        revision: i64,
        format: String,
    },
    CancelLocal {
        id: u64,
    },
    Stop {},
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Ready {
        version: u32,
    },
    Snapshot {
        revision: i64,
        formats: Vec<String>,
        origin: Option<u64>,
    },
    Published {
        generation: u64,
        revision: i64,
    },
    /// Answers every `AttachFiles`; `revision` is `None` on refusal.
    Attached {
        generation: u64,
        revision: Option<i64>,
    },
    Rejected {
        generation: u64,
    },
    Revoked {
        generation: u64,
        revision: Option<i64>,
    },
    Read {
        generation: u64,
        id: u64,
        format: String,
    },
    Cancelled {
        generation: u64,
        id: u64,
    },
    LocalData {
        id: u64,
        revision: i64,
        offset: u64,
        eof: bool,
    },
    LocalFailed {
        id: u64,
    },
}

pub struct Frame<T> {
    pub header: T,
    pub bytes: Vec<u8>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Invalid(pub &'static str);
impl fmt::Display for Invalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Invalid clipboard helper {}", self.0)
    }
}
impl std::error::Error for Invalid {}
fn format(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}
fn formats(values: &[String]) -> bool {
    values.len() <= MAX_FORMATS
        && values.iter().all(|value| format(value))
        && values
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            == values.len()
}
impl Command {
    pub fn validate(&self, bytes: &[u8]) -> Result<(), Invalid> {
        if bytes.len() > CHUNK || (!bytes.is_empty() && !matches!(self, Self::Data { .. })) {
            return Err(Invalid("payload"));
        }
        match self {
            Self::Hello { version, board } => {
                if *version != VERSION {
                    return Err(Invalid("version"));
                }
                if let Board::Test { name } = board
                    && (name.is_empty() || name.len() > 256 || name.chars().any(char::is_control))
                {
                    return Err(Invalid("board"));
                }
            }
            Self::Publish {
                generation,
                formats: values,
                ..
            } => {
                if *generation == 0 || values.is_empty() || !formats(values) {
                    return Err(Invalid("publication"));
                }
            }
            Self::AttachFiles { folder, .. } if folder.len() > 128 || folder.contains('\0') => {
                return Err(Invalid("folder"));
            }
            Self::ReadLocal { format: value, .. } if !format(value) => {
                return Err(Invalid("format"));
            }
            Self::Data { offset, eof, .. }
                if *offset > MAX_BYTES as u64
                    || offset
                        .checked_add(bytes.len() as u64)
                        .is_none_or(|end| end > MAX_BYTES as u64)
                    || (!eof && bytes.is_empty()) =>
            {
                return Err(Invalid("data range"));
            }
            _ => {}
        }
        Ok(())
    }
}
impl Event {
    pub fn validate(&self, bytes: &[u8]) -> Result<(), Invalid> {
        if bytes.len() > CHUNK || (!bytes.is_empty() && !matches!(self, Self::LocalData { .. })) {
            return Err(Invalid("payload"));
        }
        match self {
            Self::Ready { version } if *version != VERSION => return Err(Invalid("version")),
            Self::Snapshot {
                formats: values, ..
            } if !formats(values) => return Err(Invalid("formats")),
            Self::Read { format: value, .. } if !format(value) => return Err(Invalid("format")),
            Self::LocalData { offset, eof, .. }
                if *offset > MAX_BYTES as u64
                    || offset
                        .checked_add(bytes.len() as u64)
                        .is_none_or(|end| end > MAX_BYTES as u64)
                    || (!eof && bytes.is_empty()) =>
            {
                return Err(Invalid("data range"));
            }
            _ => {}
        }
        Ok(())
    }
}
