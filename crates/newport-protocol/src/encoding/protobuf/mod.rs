//! Newport's wire: one Protocol Buffers message per varint length-delimited
//! frame, read with a bounded decoder.
//!
//! Reading checks a frame against its protocol's [`Limits`] before decoding
//! it: the declared length before the body is read, then the size of
//! everything outside `bytes` fields, using the message's generated
//! [`Layout`]. `bytes` fields decode as [`bytes::Bytes`] borrowed from the
//! frame. Writing a message checks it against the writer's own limits, so a
//! frame the peer would refuse fails locally instead of closing the
//! connection.
//!
//! The metadata limit counts every byte of the frame except the contents of
//! `bytes` fields, at any depth: tags, lengths, varints, strings and packed
//! repeated scalars all count. Depth counts every message a frame nests,
//! and a map entry is a message of its own, so a map's message values sit
//! two levels below the message holding the map.

pub(crate) mod fields;
mod frame;
mod incremental;
mod layout;
#[cfg(feature = "tokio")]
pub mod tokio;
#[cfg(feature = "trace")]
pub mod trace;

pub use crate::common::Limits;
use std::fmt;

pub use frame::{LengthDecoder, decode, read_frame, read_message, write_frame, write_message};
pub use incremental::{EncodedFrame, FrameDecoder};
pub use layout::{Field, Layout};

#[allow(dead_code)]
pub(crate) mod generated {
    include!(concat!(env!("OUT_DIR"), "/protos.rs"));
}

pub use generated::newport::wire::{Error, Present, Retry, Secret};

/// The encoded `FileDescriptorSet` of the shared protocol schemas.
pub const DESCRIPTORS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/descriptors.bin"));

pub(crate) mod contracts {
    include!(concat!(env!("OUT_DIR"), "/contract_ids.rs"));
}

/// How deep messages may nest inside a frame.
pub const MAX_DEPTH: u32 = 32;

/// Why a frame was not read or written.
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("malformed protobuf field")]
    Malformed,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("frame of {length} bytes is over the limit of {limit}")]
    TooLarge { length: u64, limit: usize },
    #[error("frame metadata of {length} bytes is over the limit of {limit}")]
    MetadataTooLarge { length: usize, limit: usize },
    #[error("messages nest deeper than {MAX_DEPTH}")]
    TooDeep,
    #[error("stream ended inside a frame")]
    Truncated,
    #[error(transparent)]
    Decode(#[from] prost::DecodeError),
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}
