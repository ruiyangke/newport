//! Local browser helpers and their permission-owning integration service.
use crate::common;
use std::fmt;

#[cfg(feature = "protobuf")]
pub mod wire;

pub const CAPABILITY: &str = "browser-helper";
pub const VERSION: u32 = 1;
pub const MAX_TEXT: usize = 8192;
pub const LIMITS: common::Limits = common::Limits {
    message: MAX_TEXT + 128,
    metadata: MAX_TEXT + 128,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    Opened { warning: Option<String> },
    Failed(common::Error),
}

/// The owner announces its protocol before the helper sends a URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerMessage {
    Hello { version: u32 },
    Reply(Reply),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Invalid(pub &'static str);
impl fmt::Display for Invalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Invalid browser {}", self.0)
    }
}
impl std::error::Error for Invalid {}

pub fn web_url(value: &str) -> Option<url::Url> {
    if value.len() > MAX_TEXT || value.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    let url = url::Url::parse(value).ok()?;
    (matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none())
    .then_some(url)
}

impl Request {
    pub fn validate(&self) -> Result<(), Invalid> {
        web_url(&self.url).map(|_| ()).ok_or(Invalid("URL"))
    }
}
impl Reply {
    pub fn validate(&self) -> Result<(), Invalid> {
        let message = match self {
            Self::Opened { warning } => warning.as_deref(),
            Self::Failed(error) => {
                if error.code.is_empty()
                    || error.code.len() > 64
                    || !error
                        .code
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
                {
                    return Err(Invalid("error code"));
                }
                Some(error.message.as_str())
            }
        };
        if message.is_some_and(|text| text.len() > MAX_TEXT || text.contains('\0')) {
            return Err(Invalid("message"));
        }
        Ok(())
    }
}
impl ServerMessage {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Hello { version } if *version != VERSION => Err(Invalid("protocol version")),
            Self::Hello { .. } => Ok(()),
            Self::Reply(reply) => reply.validate(),
        }
    }
}
