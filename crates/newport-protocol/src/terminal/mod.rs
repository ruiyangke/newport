//! Persistent-shell management, independent of the holder's attachment protocol.
use crate::common;
use serde::{Deserialize, Serialize};
use std::fmt;

pub mod holder;

#[cfg(feature = "protobuf")]
pub mod wire;

pub const VERSION: u32 = 1;
pub const CLIENT_LIMITS: common::Limits = common::Limits {
    message: 256 * 1024,
    metadata: 256 * 1024,
};
pub const SERVER_LIMITS: common::Limits = common::Limits {
    message: 1024 * 1024,
    metadata: 1024 * 1024,
};
pub const MAX_COMMAND: usize = CLIENT_LIMITS.message - 4096;
pub const MAX_TEXT: usize = 16 * 1024;
/// The longest agent status a holder lists; see [`Holder::agent`].
pub const MAX_AGENT: usize = 8 * 1024;
pub const MAX_COMPUTER_NAME: usize = 1024;
pub const MAX_HOLDERS: usize = 4096;
pub const MAX_ATTACHED: usize = 256;
/// The code of a start that failed before Needle launched anything: no holder
/// runs for its id, so the desktop may open an ordinary shell instead.
pub const NOT_STARTED: &str = "holder_not_started";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Computer {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attached {
    pub id: String,
    pub name: String,
    pub active: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Exit {
    Code(u32),
    Signal(u32),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Holder {
    pub id: String,
    pub socket: String,
    pub version: u32,
    pub columns: u16,
    pub rows: u16,
    pub started: u64,
    pub name: Option<String>,
    pub title: Option<String>,
    /// What the shell's coding agent last reported about itself, as an
    /// OSC 7501 program status report (the text after `7501;`).
    pub agent: Option<String>,
    pub creator: Computer,
    pub attached: Vec<Attached>,
    pub exited: Option<Exit>,
}

#[derive(Clone, PartialEq, Eq)]
pub enum Request {
    Start {
        id: String,
        columns: u16,
        rows: u16,
        command: String,
        creator: Computer,
        stable_display: bool,
    },
    List,
    Rename {
        id: String,
        name: String,
    },
    End {
        id: String,
    },
    Watch,
}

impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Start { .. } => "Start { .. }",
            Self::List => "List",
            Self::Rename { .. } => "Rename { .. }",
            Self::End { .. } => "End { .. }",
            Self::Watch => "Watch",
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    Started { socket: String },
    Listed(Vec<Holder>),
    Renamed { renamed: bool },
    Ended { ended: bool },
    Failed(common::Error),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Invalid(pub &'static str);
impl fmt::Display for Invalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Invalid terminal {}", self.0)
    }
}
impl std::error::Error for Invalid {}

pub fn valid_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
fn text(value: &str, limit: usize) -> bool {
    value.len() <= limit && !value.contains('\0')
}
fn socket(value: &str) -> bool {
    value.starts_with('/') && text(value, 4096)
}
fn computer(id: &str, name: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok_and(|id| !id.is_nil()) && text(name, MAX_COMPUTER_NAME)
}

impl Request {
    pub fn validate(&self) -> Result<(), Invalid> {
        let valid = match self {
            Self::Start {
                id,
                columns,
                rows,
                command,
                creator,
                ..
            } => {
                valid_id(id)
                    && *columns > 0
                    && *rows > 0
                    && !command.is_empty()
                    && text(command, MAX_COMMAND)
                    && computer(&creator.id, &creator.name)
            }
            Self::List | Self::Watch => true,
            Self::Rename { id, name } => valid_id(id) && text(name, MAX_TEXT),
            Self::End { id } => valid_id(id),
        };
        if valid {
            Ok(())
        } else {
            Err(Invalid("request"))
        }
    }
}

impl Holder {
    /// UTF-8 storage retained by this record, for bounding a collected list.
    pub fn text_bytes(&self) -> usize {
        self.id.len()
            + self.socket.len()
            + self.creator.id.len()
            + self.creator.name.len()
            + self.name.as_ref().map_or(0, String::len)
            + self.title.as_ref().map_or(0, String::len)
            + self.agent.as_ref().map_or(0, String::len)
            + self
                .attached
                .iter()
                .map(|value| value.id.len() + value.name.len())
                .sum::<usize>()
    }
    pub fn validate(&self) -> Result<(), Invalid> {
        if !valid_id(&self.id)
            || !socket(&self.socket)
            || self.version == 0
            || self.columns == 0
            || self.rows == 0
            || !computer(&self.creator.id, &self.creator.name)
            || self.name.as_ref().is_some_and(|name| !text(name, MAX_TEXT))
            || self
                .title
                .as_ref()
                .is_some_and(|title| !text(title, MAX_TEXT))
            || self.agent.as_ref().is_some_and(|agent| {
                agent.len() > MAX_AGENT || !agent.bytes().all(|b| b.is_ascii_graphic())
            })
            || self.attached.len() > MAX_ATTACHED
            || self
                .attached
                .iter()
                .any(|attached| !computer(&attached.id, &attached.name))
            || matches!(self.exited, Some(Exit::Signal(signal)) if !(1..=64).contains(&signal))
        {
            return Err(Invalid("holder"));
        }
        Ok(())
    }
}

impl Reply {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Started { socket: path } if !socket(path) => Err(Invalid("socket")),
            Self::Listed(holders) => {
                if holders.len() > MAX_HOLDERS {
                    return Err(Invalid("holder count"));
                }
                let mut ids = std::collections::BTreeSet::new();
                let mut bytes = 0usize;
                for holder in holders {
                    holder.validate()?;
                    bytes = bytes.saturating_add(holder.text_bytes());
                    if bytes > SERVER_LIMITS.metadata {
                        return Err(Invalid("list text budget"));
                    }
                    if !ids.insert(&holder.id) {
                        return Err(Invalid("duplicate holder"));
                    }
                }
                Ok(())
            }
            Self::Failed(error)
                if error.code.is_empty()
                    || !text(&error.code, 64)
                    || error.message.is_empty()
                    || !text(&error.message, 4096) =>
            {
                Err(Invalid("error"))
            }
            _ => Ok(()),
        }
    }
    pub fn validate_for(&self, request: &Request) -> Result<(), Invalid> {
        self.validate()?;
        if matches!(
            (self, request),
            (Self::Started { .. }, Request::Start { .. })
                | (Self::Listed(_), Request::List | Request::Watch)
                | (Self::Renamed { .. }, Request::Rename { .. })
                | (Self::Ended { .. }, Request::End { .. })
                | (Self::Failed(_), _)
        ) {
            Ok(())
        } else {
            Err(Invalid("reply operation"))
        }
    }
}
