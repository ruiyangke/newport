//! Native Windows clipboard provider operations and publication receipts.
use crate::common;
use std::fmt;
#[cfg(feature = "protobuf")]
pub mod wire;
pub const VERSION: u32 = 1;
pub const MAX_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_METADATA: usize = 64 * 1024;
pub const MAX_FORMATS: usize = 256;
pub const MAX_PATH_UNITS: usize = 32767;
pub const LIMITS: common::Limits = common::Limits {
    message: MAX_BYTES + MAX_METADATA + 1024,
    metadata: MAX_METADATA,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Identity {
    pub sequence: u32,
    pub owner: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileWrite {
    Unchanged,
    Committed { revision: i64, complete: bool },
    Uncertain,
}
/// Exact native UTF-16 units. No normalization or Unicode replacement occurs.
#[derive(Clone, PartialEq, Eq)]
pub struct NativePath(pub Vec<u16>);
impl fmt::Debug for NativePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NativePath")
            .field("units", &self.0.len())
            .finish()
    }
}
impl NativePath {
    pub fn validate(&self) -> Result<(), Invalid> {
        if self.0.is_empty() || self.0.len() > MAX_PATH_UNITS || self.0.contains(&0) {
            Err(Invalid("native path"))
        } else {
            Ok(())
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublicationStage {
    Source,
    Queue,
    Observe,
    Baseline,
    Formats,
    Generation,
    Open,
    Validate,
    Admission,
    Empty,
    DelayedFormats,
    Concealment,
    Reply,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PublicationRejection {
    pub stage: PublicationStage,
    pub expected: i64,
    pub observed: Option<i64>,
    pub native_error: Option<u32>,
}
#[derive(Clone, PartialEq, Eq)]
pub enum Message {
    Hello {
        version: u32,
        nonce: String,
    },
    Ready {
        version: u32,
        nonce: String,
    },
    Snapshot {
        id: u64,
    },
    Read {
        id: u64,
        revision: i64,
        format: String,
    },
    Publish {
        id: u64,
        expected: i64,
        formats: Vec<String>,
    },
    Attach {
        id: u64,
        generation: u64,
        folder: NativePath,
    },
    Done {
        id: u64,
        outcome: Option<Outcome>,
    },
    Admit {
        id: u64,
        source: u64,
        preparation: bool,
    },
    Admitted {
        id: u64,
        accepted: bool,
    },
    Demand {
        id: u64,
        source: u64,
        format: String,
    },
    Data {
        id: u64,
        present: bool,
    },
    Consumed {
        id: u64,
    },
    Revoke {
        source: u64,
    },
    Revoked {
        source: u64,
    },
    Stop,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    PublicationRejected(PublicationRejection),
    Snapshot {
        revision: i64,
        identity: Identity,
        formats: Vec<String>,
        origin: Option<u64>,
    },
    Read {
        revision: i64,
        identity: Identity,
    },
    Published {
        generation: u64,
        owner: u64,
        preparation: Option<(i64, Identity)>,
    },
    Attached {
        result: FileWrite,
    },
}
impl fmt::Debug for Message {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Hello { .. } => "Hello",
            Self::Ready { .. } => "Ready",
            Self::Snapshot { .. } => "Snapshot",
            Self::Read { .. } => "Read",
            Self::Publish { .. } => "Publish",
            Self::Attach { .. } => "Attach",
            Self::Done { .. } => "Done",
            Self::Admit { .. } => "Admit",
            Self::Admitted { .. } => "Admitted",
            Self::Demand { .. } => "Demand",
            Self::Data { .. } => "Data",
            Self::Consumed { .. } => "Consumed",
            Self::Revoke { .. } => "Revoke",
            Self::Revoked { .. } => "Revoked",
            Self::Stop => "Stop",
        })
    }
}
#[derive(Clone, PartialEq, Eq)]
pub struct Packet {
    pub message: Message,
    pub data: Vec<u8>,
}
impl fmt::Debug for Packet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Packet")
            .field("message", &self.message)
            .field("bytes", &self.data.len())
            .finish()
    }
}
impl From<Message> for Packet {
    fn from(message: Message) -> Self {
        Self {
            message,
            data: Vec::new(),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Invalid(pub &'static str);
impl fmt::Display for Invalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Invalid clipboard provider {}", self.0)
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
impl Packet {
    pub fn validate(&self) -> Result<(), Invalid> {
        if self.data.len() > MAX_BYTES
            || (!self.data.is_empty()
                && !matches!(
                    self.message,
                    Message::Data { present: true, .. }
                        | Message::Done {
                            outcome: Some(Outcome::Read { .. }),
                            ..
                        }
                ))
        {
            return Err(Invalid("payload"));
        }
        match &self.message {
            Message::Hello { version, nonce } | Message::Ready { version, nonce }
                if *version != VERSION
                    || !uuid::Uuid::parse_str(nonce).is_ok_and(|id| !id.is_nil()) =>
            {
                return Err(Invalid("greeting"));
            }
            Message::Read { format: value, .. } | Message::Demand { format: value, .. }
                if !format(value) =>
            {
                return Err(Invalid("format"));
            }
            Message::Publish {
                formats: values, ..
            } if !formats(values) => return Err(Invalid("formats")),
            Message::Attach { folder, .. } => folder.validate()?,
            Message::Done {
                outcome:
                    Some(Outcome::Snapshot {
                        formats: values, ..
                    }),
                ..
            } if !formats(values) => return Err(Invalid("formats")),
            _ => {}
        }
        Ok(())
    }
}
