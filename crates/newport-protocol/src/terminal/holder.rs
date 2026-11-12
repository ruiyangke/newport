//! Terminal attachment contracts. A holder announces its identity before a
//! client sends attachment data or changes its shell.
use super::{Computer, Exit};
use crate::common;
use bytes::Bytes;
use std::fmt;
#[cfg(feature = "protobuf")]
pub mod wire;

pub const CAPABILITY: &str = "terminal-holder";
pub const VERSION: u32 = 1;
pub const MAX_DATA: usize = 64 * 1024;
pub const MAX_SNAPSHOT: usize = 16 * 1024 * 1024;
/// Raw DEFLATE may grow incompressible input; restored state has its own cap.
pub const MAX_COMPRESSED_SNAPSHOT: usize = MAX_SNAPSHOT + 64 * 1024;
pub const MAX_NAME: usize = 256;
pub const MAX_RENAME: usize = 120;
pub const CLIENT_LIMITS: common::Limits = common::Limits {
    message: MAX_DATA + 1024,
    metadata: 4096,
};
pub const SERVER_LIMITS: common::Limits = common::Limits {
    message: MAX_COMPRESSED_SNAPSHOT + 1024,
    metadata: 4096,
};
pub const REOPEN_REQUIRED: &str = "reopen_required";
/// The code of a holder greeting that never completed.
pub const GREETING_INCOMPLETE: &str = "holder_greeting_incomplete";
pub const REOPEN_MESSAGE: &str = "This terminal uses an incompatible protocol. Close it to end its shell, then open a new terminal.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hello {
    pub version: u32,
    pub terminal_id: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Attach {
    pub version: u32,
    pub columns: u16,
    pub rows: u16,
    pub control: bool,
    pub opened: bool,
    pub computer: Option<Computer>,
    pub colors: Option<[[u8; 3]; 3]>,
}
#[derive(Clone, PartialEq, Eq)]
pub enum ClientMessage {
    Attach(Attach),
    Input(Bytes),
    Resize { columns: u16, rows: u16 },
    End,
    Rename(String),
    Activate,
}
#[derive(Clone, PartialEq, Eq)]
pub enum ServerMessage {
    Hello(Hello),
    Attached { prompted: bool },
    Screen(Bytes),
    Full(Bytes),
    Output(Bytes),
    Exit(Exit),
    Active(bool),
    Failed(common::Error),
    Renamed(String),
}
impl fmt::Debug for ClientMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Attach(value) => f.debug_tuple("Attach").field(value).finish(),
            Self::Input(bytes) => f
                .debug_struct("Input")
                .field("bytes", &bytes.len())
                .finish(),
            Self::Resize { columns, rows } => f
                .debug_struct("Resize")
                .field("columns", columns)
                .field("rows", rows)
                .finish(),
            Self::End => f.write_str("End"),
            Self::Rename(_) => f.write_str("Rename"),
            Self::Activate => f.write_str("Activate"),
        }
    }
}
impl fmt::Debug for ServerMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Hello(value) => f.debug_tuple("Hello").field(value).finish(),
            Self::Attached { prompted } => f
                .debug_struct("Attached")
                .field("prompted", prompted)
                .finish(),
            Self::Screen(bytes) => f
                .debug_struct("Screen")
                .field("bytes", &bytes.len())
                .finish(),
            Self::Full(bytes) => f.debug_struct("Full").field("bytes", &bytes.len()).finish(),
            Self::Output(bytes) => f
                .debug_struct("Output")
                .field("bytes", &bytes.len())
                .finish(),
            Self::Exit(value) => f.debug_tuple("Exit").field(value).finish(),
            Self::Active(value) => f.debug_tuple("Active").field(value).finish(),
            Self::Failed(value) => f.debug_tuple("Failed").field(value).finish(),
            Self::Renamed(_) => f.write_str("Renamed"),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Invalid(pub &'static str);
impl fmt::Display for Invalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Invalid terminal holder {}", self.0)
    }
}
impl std::error::Error for Invalid {}
pub fn reopen_required() -> common::Error {
    common::Error {
        code: REOPEN_REQUIRED.into(),
        message: REOPEN_MESSAGE.into(),
        retry: common::Retry::No,
    }
}
fn name(value: &str, characters: usize) -> bool {
    value.chars().count() <= characters && !value.chars().any(char::is_control)
}
impl Hello {
    pub fn validate(&self) -> Result<(), Invalid> {
        if self.version == VERSION && super::valid_id(&self.terminal_id) {
            Ok(())
        } else {
            Err(Invalid("greeting"))
        }
    }
    pub fn require(&self, terminal_id: &str) -> Result<(), Invalid> {
        self.validate()?;
        if self.terminal_id == terminal_id {
            Ok(())
        } else {
            Err(Invalid("terminal identity"))
        }
    }
}
impl Attach {
    pub fn validate(&self) -> Result<(), Invalid> {
        if self.version != VERSION
            || self.columns == 0
            || self.rows == 0
            || self.computer.as_ref().is_some_and(|computer| {
                !uuid::Uuid::parse_str(&computer.id).is_ok_and(|id| !id.is_nil())
                    || !name(&computer.name, MAX_NAME)
            })
            || (!self.control && self.computer.is_none())
            || (self.control && self.opened)
        {
            return Err(Invalid("attachment"));
        }
        Ok(())
    }
}
impl ClientMessage {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Attach(value) => value.validate(),
            Self::Input(value) if value.is_empty() || value.len() > MAX_DATA => {
                Err(Invalid("input size"))
            }
            Self::Resize { columns, rows } if *columns == 0 || *rows == 0 => Err(Invalid("size")),
            Self::Rename(value) if !name(value, MAX_RENAME) => Err(Invalid("name")),
            _ => Ok(()),
        }
    }
}
impl ServerMessage {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Hello(value) => value.validate(),
            Self::Screen(value) if value.len() > MAX_SNAPSHOT => Err(Invalid("snapshot size")),
            Self::Full(value) if value.len() > MAX_COMPRESSED_SNAPSHOT => {
                Err(Invalid("compressed snapshot size"))
            }
            Self::Output(value) if value.is_empty() || value.len() > MAX_DATA => {
                Err(Invalid("output size"))
            }
            Self::Renamed(value) if !name(value, MAX_RENAME) || value.trim() != value => {
                Err(Invalid("persisted name"))
            }
            Self::Exit(Exit::Signal(signal)) if !(1..=64).contains(signal) => {
                Err(Invalid("exit signal"))
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
