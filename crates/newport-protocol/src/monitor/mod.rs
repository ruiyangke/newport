//! Server observations and feature operations carried by the monitor service.
pub mod compose;
pub mod containers;
pub mod file_watch;
pub mod metrics;
pub mod ports;
pub mod services;
pub mod stable_display;

#[cfg(feature = "protobuf")]
pub mod wire;

use crate::common;

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Invalid(pub &'static str);
impl fmt::Display for Invalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Invalid monitor {}", self.0)
    }
}
impl std::error::Error for Invalid {}

pub const VERSION: u32 = 1;
#[cfg(feature = "protobuf")]
pub const CONTRACT_ID: &str = crate::encoding::protobuf::contracts::MONITOR;
pub const MAX_ERROR_MESSAGE: usize = 4096;
pub const CLIENT_LIMITS: common::Limits = common::Limits {
    message: 64 * 1024,
    metadata: 64 * 1024,
};
pub const SERVER_LIMITS: common::Limits = common::Limits {
    message: 4 * 1024 * 1024,
    metadata: 4 * 1024 * 1024,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    Metrics,
    StableDisplay,
    Services(services::Request),
    Containers(containers::Request),
    Compose(compose::Request),
    Ports(ports::discovery::Request),
    FileWatch(file_watch::Request),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InvalidRequest {
    Correlation,
    Services(services::InvalidRequest),
    Containers(containers::InvalidRequest),
    Compose(compose::InvalidRequest),
    FileWatch(Invalid),
}
impl fmt::Display for InvalidRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Correlation => f.write_str("Monitor requests need a nonzero identifier."),
            Self::Services(e) => e.fmt(f),
            Self::Containers(e) => e.fmt(f),
            Self::Compose(e) => e.fmt(f),
            Self::FileWatch(e) => e.fmt(f),
        }
    }
}
impl std::error::Error for InvalidRequest {}
impl Request {
    pub fn validate(&self) -> Result<(), InvalidRequest> {
        match self {
            Self::Services(v) => v.validate().map_err(InvalidRequest::Services)?,
            Self::Containers(v) => v.validate().map_err(InvalidRequest::Containers)?,
            Self::Compose(v) => v.validate().map_err(InvalidRequest::Compose)?,
            Self::FileWatch(v) => v.validate().map_err(InvalidRequest::FileWatch)?,
            Self::Metrics | Self::StableDisplay | Self::Ports(_) => {}
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    pub id: u64,
    pub request: Request,
}
impl Call {
    pub fn validate(&self) -> Result<(), InvalidRequest> {
        if self.id == 0 {
            return Err(InvalidRequest::Correlation);
        }
        self.request.validate()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Reply {
    Metrics(metrics::Snapshot),
    StableDisplay(stable_display::State),
    Services(services::Reply),
    Containers(containers::Reply),
    Compose(compose::Reply),
    Ports(Box<ports::discovery::Snapshot>),
    FileWatch(file_watch::Reply),
    Failed(common::Error),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Response {
    pub id: u64,
    pub reply: Reply,
}
impl Response {
    pub fn validate_for(&self, call: &Call) -> Result<(), Invalid> {
        if self.id == 0 || self.id != call.id {
            return Err(Invalid("reply correlation"));
        }
        self.reply.validate_for(&call.request)
    }
}
impl Reply {
    pub fn validate_for(&self, request: &Request) -> Result<(), Invalid> {
        match (self, request) {
            (Self::Metrics(v), Request::Metrics) => v.validate(),
            (Self::StableDisplay(v), Request::StableDisplay) => v.validate(),
            (Self::Services(v), Request::Services(r)) => v.validate_for(r),
            (Self::Containers(v), Request::Containers(r)) => v.validate_for(r),
            (Self::Compose(v), Request::Compose(r)) => v.validate_for(r),
            (Self::Ports(v), Request::Ports(_)) => v.validate(),
            (Self::FileWatch(v), Request::FileWatch(r)) => v.validate_for(r),
            (Self::Failed(e), _)
                if !e.code.is_empty()
                    && e.code.len() <= 64
                    && !e.message.is_empty()
                    && e.message.len() <= MAX_ERROR_MESSAGE
                    && !e.code.contains('\0')
                    && !e.message.contains('\0') =>
            {
                Ok(())
            }
            _ => Err(Invalid("result operation")),
        }
    }
}
