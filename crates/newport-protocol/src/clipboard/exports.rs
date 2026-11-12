//! Native clipboard exports carried by the integration service.
//! Runtime routing owns access and focus; these messages cannot grant either.
use super::{Chunk, CopyLease, ExportOffer, Origin};
use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests;
#[cfg(feature = "protobuf")]
pub mod wire;

pub const CAPABILITY: &str = "clipboard-export";
pub const VERSION: u32 = 1;
pub const VERSIONS: &[u32] = &[VERSION];
pub const MAX_FRAME: usize = 72 * 1024;
pub const MAX_READS: usize = 8;
pub const CHUNK: usize = 64 * 1024;
pub const MAX_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_FORMATS: usize = 256;
pub const MAX_FORMAT_BYTES: usize = 256;
pub const MAX_ID_BYTES: usize = 64;
pub const LIMITS: crate::common::Limits = crate::common::Limits {
    message: MAX_FRAME,
    metadata: MAX_FRAME,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum Control {
    Enable {
        version: u32,
    },
    Disable,
    Read {
        id: u64,
        offer: ExportOffer,
        format: String,
    },
    Cancel {
        id: u64,
    },
    /// Focus authorizes this acknowledgment, not later reads. A successful
    /// commit replaces the previous committed copy.
    Commit {
        id: u64,
        offer: ExportOffer,
    },
    Release {
        copy: CopyLease,
    },
    ReadCopy {
        id: u64,
        copy: CopyLease,
        format: String,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum Event {
    /// Invalidates focused reads. Committed copies have their own lifetime,
    /// reported by Committed and CopyRevoked.
    Offer {
        offer: Option<ExportOffer>,
    },
    /// IDs never repeat within an integration channel. None is a failed read;
    /// consumers must discard the complete representation on any failure.
    Data {
        id: u64,
        offset: usize,
        chunk: Option<Chunk>,
    },
    /// None rejects the proposal without replacing a prior copy.
    Committed {
        id: u64,
        copy: Option<CopyLease>,
    },
    CopyRevoked {
        id: String,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Invalid;
impl std::fmt::Display for Invalid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Invalid clipboard export message")
    }
}
impl std::error::Error for Invalid {}
fn invalid() -> Invalid {
    Invalid
}
fn format(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_FORMAT_BYTES
        && !name.chars().any(char::is_control)
        && !name
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
}
fn offer(value: &ExportOffer) -> Result<(), Invalid> {
    let source = &value.offer;
    if uuid::Uuid::parse_str(&source.runtime).is_err()
        || source.revision <= 0
        || value.route_revision == 0
        || source.formats.is_empty()
        || source.formats.len() > MAX_FORMATS
        || source.formats.iter().any(|name| !format(name))
        || !matches!(&source.origin, Origin::Native { worker, revision }
            if *revision > 0 && !worker.is_empty() && worker.len() <= MAX_ID_BYTES
                && worker.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'))
    {
        return Err(invalid());
    }
    let unique: std::collections::BTreeSet<_> = source.formats.iter().collect();
    if unique.len() != source.formats.len() {
        return Err(invalid());
    }
    Ok(())
}
fn copy(value: &CopyLease) -> Result<(), Invalid> {
    if uuid::Uuid::parse_str(&value.id).is_err() {
        return Err(invalid());
    }
    offer(&value.offer)
}
impl Control {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Enable { version } if !VERSIONS.contains(version) => Err(invalid()),
            Self::Commit { id, offer: value } => {
                if *id == 0 {
                    return Err(invalid());
                }
                offer(value)
            }
            Self::Release { copy: value } => copy(value),
            Self::ReadCopy {
                id,
                copy: value,
                format: name,
            } => {
                copy(value)?;
                if *id == 0 || !value.offer.offer.formats.contains(name) {
                    return Err(invalid());
                }
                Ok(())
            }
            Self::Read {
                id,
                offer: value,
                format: name,
            } => {
                offer(value)?;
                if *id == 0 || !value.offer.formats.contains(name) {
                    return Err(invalid());
                }
                Ok(())
            }
            Self::Cancel { id: 0 } => Err(invalid()),
            _ => Ok(()),
        }
    }
}
impl Event {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Committed { id, copy: value } => {
                if *id == 0 {
                    return Err(invalid());
                }
                value.as_ref().map(copy).transpose().map(|_| ())
            }
            Self::CopyRevoked { id } => {
                uuid::Uuid::parse_str(id).map(|_| ()).map_err(|_| invalid())
            }
            Self::Offer { offer: Some(value) } => offer(value),
            Self::Data { id, offset, chunk } => {
                if *id == 0 || *offset > MAX_BYTES {
                    return Err(invalid());
                }
                if chunk.as_ref().is_some_and(|chunk| {
                    chunk.bytes.len() > CHUNK
                        || (chunk.bytes.is_empty() && !chunk.eof)
                        || offset
                            .checked_add(chunk.bytes.len())
                            .is_none_or(|end| end > MAX_BYTES)
                }) {
                    return Err(invalid());
                }
                Ok(())
            }
            Self::Offer { offer: None } => Ok(()),
        }
    }
}
