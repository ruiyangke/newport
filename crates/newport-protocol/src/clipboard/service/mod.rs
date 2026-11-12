//! Authenticated runtime clipboard IPC. Publication and export authority are
//! independent of authentication and remain owned by runtime control.
use super::{
    Access, Chunk, CopyLease, ExportOffer, NativeRequest, Offer, Origin, ProviderLease,
    ProviderPoll, imports,
};
use crate::common;
use std::{collections::BTreeSet, fmt};

#[cfg(feature = "protobuf")]
pub mod wire;

pub const VERSION: u32 = 1;
pub const LIMITS: common::Limits = common::Limits {
    message: 72 * 1024,
    metadata: 72 * 1024,
};
pub const MAX_REQUESTS: usize = 8;
pub const MAX_ID: usize = 64;
pub const MAX_PATH: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hello {
    pub version: u32,
    pub runtime: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExportRead {
    Focused(ExportOffer),
    Committed(CopyLease),
}
impl ExportRead {
    pub fn offer(&self) -> &ExportOffer {
        match self {
            Self::Focused(offer) => offer,
            Self::Committed(copy) => &copy.offer,
        }
    }
    fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Focused(offer) => export(offer),
            Self::Committed(value) => copy(value),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    Authenticate(Access),
    Offer,
    Read {
        runtime: String,
        revision: i64,
        format: String,
        offset: usize,
        limit: usize,
        export: Option<ExportRead>,
    },
    Claim,
    Enabled(bool),
    Publish {
        revision: i64,
        formats: Vec<String>,
    },
    Poll,
    Reply(imports::Reply),
    NativePoll,
    NativePublish {
        lease: ProviderLease,
        expected_runtime: String,
        expected_revision: i64,
        revision: u64,
        formats: Vec<String>,
    },
    NativeReply {
        request: NativeRequest,
        chunk: Option<Chunk>,
    },
    ExportOffer,
    CommitCopy(ExportOffer),
    CopyStatus,
    ReleaseCopy(CopyLease),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Response {
    Hello(Hello),
    Ready,
    Offer(Offer),
    Chunk(Chunk),
    Requests(Vec<imports::Request>),
    Provider(ProviderPoll),
    Published(bool),
    Export(Option<ExportOffer>),
    Copy(Option<CopyLease>),
    Error(common::Error),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Invalid(pub &'static str);
impl fmt::Display for Invalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Invalid clipboard service {}", self.0)
    }
}
impl std::error::Error for Invalid {}
fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_ID
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}
fn formats(values: &[String]) -> Result<(), Invalid> {
    if values.len() > imports::MAX_FORMATS
        || values.iter().any(|value| !imports::valid_format(value))
        || values.iter().collect::<BTreeSet<_>>().len() != values.len()
    {
        return Err(Invalid("formats"));
    }
    Ok(())
}
pub(crate) fn access(value: &Access) -> Result<(), Invalid> {
    // This socket is on the Linux server, independent of the desktop's OS.
    if !value
        .path
        .to_str()
        .is_some_and(|path| path.starts_with('/') && path.len() <= MAX_PATH && !path.contains('\0'))
        || !identifier(&value.runtime)
        || !identifier(&value.worker)
        || uuid::Uuid::parse_str(&value.token).is_err()
    {
        return Err(Invalid("access"));
    }
    Ok(())
}
fn offer(value: &Offer) -> Result<(), Invalid> {
    if !identifier(&value.runtime) {
        return Err(Invalid("offer runtime"));
    }
    if let Origin::Native { worker, revision } = &value.origin
        && (!identifier(worker) || *revision == 0)
    {
        return Err(Invalid("native origin"));
    }
    if value.origin == Origin::Empty && !value.formats.is_empty() {
        return Err(Invalid("empty offer"));
    }
    formats(&value.formats)
}
fn export(value: &ExportOffer) -> Result<(), Invalid> {
    offer(&value.offer)?;
    if value.route_revision == 0 || !matches!(value.offer.origin, Origin::Native { .. }) {
        return Err(Invalid("export offer"));
    }
    Ok(())
}
fn copy(value: &CopyLease) -> Result<(), Invalid> {
    if uuid::Uuid::parse_str(&value.id).is_err() {
        return Err(Invalid("copy identity"));
    }
    export(&value.offer)
}
fn lease(value: &ProviderLease) -> Result<(), Invalid> {
    if !identifier(&value.runtime) || !identifier(&value.worker) || value.epoch == 0 {
        return Err(Invalid("provider lease"));
    }
    Ok(())
}
fn native_request(value: &NativeRequest) -> Result<(), Invalid> {
    lease(&value.lease)?;
    if value.id == 0 || value.revision == 0 || !imports::valid_format(&value.format) {
        return Err(Invalid("native request"));
    }
    Ok(())
}
fn chunk(value: &Chunk) -> Result<(), Invalid> {
    if value.bytes.len() > imports::CHUNK || (value.bytes.is_empty() && !value.eof) {
        return Err(Invalid("chunk"));
    }
    Ok(())
}
impl Hello {
    pub fn validate(&self) -> Result<(), Invalid> {
        if self.version != VERSION || !identifier(&self.runtime) {
            return Err(Invalid("announcement"));
        }
        Ok(())
    }
}
impl Request {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Authenticate(value) => access(value),
            Self::Read {
                runtime,
                format,
                offset,
                limit,
                export: authority,
                revision,
            } => {
                if !identifier(runtime)
                    || !imports::valid_format(format)
                    || *offset > imports::LIMIT
                    || *limit == 0
                    || *limit > imports::CHUNK
                {
                    return Err(Invalid("range"));
                }
                if let Some(authority) = authority {
                    authority.validate()?;
                    let offer = &authority.offer().offer;
                    if &offer.runtime != runtime
                        || offer.revision != *revision
                        || !offer.formats.contains(format)
                    {
                        return Err(Invalid("range authority"));
                    }
                }
                Ok(())
            }
            Self::Publish {
                formats: values, ..
            } => formats(values),
            Self::Reply(value) => value.validate().map_err(|_| Invalid("producer reply")),
            Self::NativePublish {
                lease: authority,
                expected_runtime,
                revision,
                formats: values,
                ..
            } => {
                lease(authority)?;
                if !identifier(expected_runtime) || *revision == 0 {
                    return Err(Invalid("native publication"));
                }
                formats(values)
            }
            Self::NativeReply {
                request,
                chunk: value,
            } => {
                native_request(request)?;
                value.as_ref().map(chunk).transpose().map(|_| ())
            }
            Self::CommitCopy(value) => export(value),
            Self::ReleaseCopy(value) => copy(value),
            _ => Ok(()),
        }
    }
}
impl Response {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Hello(value) => value.validate(),
            Self::Offer(value) => offer(value),
            Self::Chunk(value) => chunk(value),
            Self::Requests(values) => {
                if values.len() > MAX_REQUESTS
                    || values
                        .iter()
                        .map(|value| value.id)
                        .collect::<BTreeSet<_>>()
                        .len()
                        != values.len()
                {
                    return Err(Invalid("producer requests"));
                }
                for value in values {
                    value.validate().map_err(|_| Invalid("producer request"))?;
                }
                Ok(())
            }
            Self::Provider(value) => {
                value.lease.as_ref().map(lease).transpose()?;
                offer(&value.offer)?;
                if value.requests.len() > MAX_REQUESTS
                    || value
                        .requests
                        .iter()
                        .map(|request| request.id)
                        .collect::<BTreeSet<_>>()
                        .len()
                        != value.requests.len()
                {
                    return Err(Invalid("native requests"));
                }
                for request in &value.requests {
                    native_request(request)?;
                    if Some(&request.lease) != value.lease.as_ref() {
                        return Err(Invalid("native request lease"));
                    }
                }
                Ok(())
            }
            Self::Export(value) => value.as_ref().map(export).transpose().map(|_| ()),
            Self::Copy(value) => value.as_ref().map(copy).transpose().map(|_| ()),
            Self::Error(value) => {
                if value.code.is_empty()
                    || value.code.len() > 64
                    || !value
                        .code
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
                    || value.message.len() > 4096
                    || value.message.contains('\0')
                {
                    return Err(Invalid("error"));
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}
