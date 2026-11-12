//! Integration service contracts. Worker admission owns capability negotiation.
use crate::{
    clipboard::{exports, imports},
    common,
    runtime::{Applied, Permissions},
};
use bytes::Bytes;
use std::fmt;

#[cfg(feature = "protobuf")]
pub mod wire;

pub const VERSION: u32 = 1;
pub const MAX_TEXT: usize = 8192;
pub const MAX_ARCHIVE: usize = imports::LIMIT + 1024 * 1024;
pub const CLIENT_LIMITS: common::Limits = common::Limits {
    message: MAX_ARCHIVE + 4096,
    metadata: 72 * 1024,
};
pub const SERVER_LIMITS: common::Limits = common::Limits {
    message: 72 * 1024,
    metadata: 72 * 1024,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserRequest {
    pub id: u64,
    pub url: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserReply {
    pub id: u64,
    pub success: bool,
    pub message: Option<String>,
}
#[derive(Clone, PartialEq, Eq)]
pub enum Request {
    Permissions(Permissions),
    ClipboardOffer(imports::Offer),
    Snapshot(Bytes),
    GuiNotices,
    Heartbeat,
    Export(exports::Control),
}
impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Permissions(_) => "Permissions",
            Self::ClipboardOffer(_) => "ClipboardOffer",
            Self::Snapshot(_) => "Snapshot",
            Self::GuiNotices => "GuiNotices",
            Self::Heartbeat => "Heartbeat",
            Self::Export(_) => "Export",
        })
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    pub id: u64,
    pub request: Request,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    Acknowledged { message: String },
    Permissions(Applied),
    Failed(common::Error),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub id: u64,
    pub reply: Reply,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientMessage {
    Call(Call),
    Clipboard(imports::Reply),
    Browser(BrowserReply),
    Quit,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerMessage {
    Response(Response),
    ClipboardRequest(imports::Request),
    Browser(BrowserRequest),
    GuiNotice,
    Export(exports::Event),
    Failed(common::Error),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Invalid(pub &'static str);
impl fmt::Display for Invalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Invalid integration {}", self.0)
    }
}
impl std::error::Error for Invalid {}
fn id(value: u64) -> Result<(), Invalid> {
    if value == 0 {
        Err(Invalid("request ID"))
    } else {
        Ok(())
    }
}
fn text(value: &str) -> Result<(), Invalid> {
    if value.len() > MAX_TEXT || value.contains('\0') {
        return Err(Invalid("text"));
    }
    Ok(())
}
fn error(value: &common::Error) -> Result<(), Invalid> {
    if value.code.is_empty()
        || value.code.len() > 64
        || !value
            .code
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(Invalid("error code"));
    }
    text(&value.message)
}
impl Call {
    pub fn validate(&self) -> Result<(), Invalid> {
        id(self.id)?;
        match &self.request {
            Request::Permissions(value) => value.validate().map_err(|_| Invalid("permissions")),
            Request::ClipboardOffer(value) => {
                value.validate().map_err(|_| Invalid("clipboard offer"))
            }
            Request::Snapshot(value) if value.len() > MAX_ARCHIVE => Err(Invalid("snapshot size")),
            Request::Export(value) => value.validate().map_err(|_| Invalid("clipboard export")),
            _ => Ok(()),
        }
    }
}
impl Response {
    pub fn validate(&self) -> Result<(), Invalid> {
        id(self.id)?;
        match &self.reply {
            Reply::Acknowledged { message } => text(message),
            Reply::Permissions(value) => value.validate().map_err(|_| Invalid("permission result")),
            Reply::Failed(value) => error(value),
        }
    }
    pub fn validate_for(&self, call: &Call) -> Result<(), Invalid> {
        call.validate()?;
        self.validate()?;
        if self.id != call.id {
            return Err(Invalid("response ID"));
        }
        match (&call.request, &self.reply) {
            (_, Reply::Failed(_)) => Ok(()),
            (Request::Permissions(wanted), Reply::Permissions(applied))
                if applied.permissions.revision > wanted.revision =>
            {
                Ok(())
            }
            (Request::Permissions(wanted), Reply::Permissions(applied)) => applied
                .validate_for(wanted)
                .map_err(|_| Invalid("permission acknowledgment")),
            (Request::Permissions(_), _) | (_, Reply::Permissions(_)) => {
                Err(Invalid("response operation"))
            }
            (_, Reply::Acknowledged { .. }) => Ok(()),
        }
    }
}
impl BrowserRequest {
    pub fn validate(&self) -> Result<(), Invalid> {
        id(self.id)?;
        crate::browser::web_url(&self.url)
            .map(|_| ())
            .ok_or(Invalid("browser URL"))
    }
}
impl BrowserReply {
    pub fn validate(&self) -> Result<(), Invalid> {
        id(self.id)?;
        self.message.as_deref().map(text).transpose().map(|_| ())
    }
}
impl ClientMessage {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Call(value) => value.validate(),
            Self::Clipboard(value) => value.validate().map_err(|_| Invalid("clipboard reply")),
            Self::Browser(value) => value.validate(),
            Self::Quit => Ok(()),
        }
    }
}
impl ServerMessage {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Response(value) => value.validate(),
            Self::ClipboardRequest(value) => {
                value.validate().map_err(|_| Invalid("clipboard request"))
            }
            Self::Browser(value) => value.validate(),
            Self::GuiNotice => Ok(()),
            Self::Export(value) => value.validate().map_err(|_| Invalid("clipboard export")),
            Self::Failed(value) => error(value),
        }
    }
}
