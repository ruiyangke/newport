//! Bounded protobuf encoding for runtime control, worker lease and admission.
use super::*;
use crate::encoding::protobuf::{
    self, FrameError, Layout, fields, generated::newport::runtime as pb,
};
use bytes::Bytes;
use prost::Message as ProstMessage;
use std::io::{Read, Write};

pub(crate) mod convert;
#[cfg(test)]
mod tests;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Frame(#[from] FrameError),
    #[error(transparent)]
    Invalid(#[from] Invalid),
    #[error(transparent)]
    Clipboard(#[from] crate::clipboard::service::wire::Error),
    #[error("runtime stream ended")]
    Ended,
    #[error("unexpected runtime message")]
    Unexpected,
}

impl From<Error> for std::io::Error {
    fn from(error: Error) -> Self {
        match error {
            Error::Frame(FrameError::Io(error)) => error,
            Error::Ended | Error::Frame(FrameError::Truncated) => {
                Self::new(std::io::ErrorKind::UnexpectedEof, error)
            }
            _ => Self::new(std::io::ErrorKind::InvalidData, error),
        }
    }
}

mod sealed {
    pub trait Sealed {}
}

/// A directional runtime message. Implementations keep generated types inside
/// the codec and validate values on both sides of the transport boundary.
pub trait Message: sealed::Sealed + Sized {
    const LIMITS: common::Limits;
    fn encode(&self) -> Result<Vec<u8>, Error>;
    fn decode(body: Bytes) -> Result<Self, Error>;
}

pub fn read<T: Message>(input: &mut impl Read) -> Result<T, Error> {
    let body = protobuf::read_frame(input, T::LIMITS.message)?.ok_or(Error::Ended)?;
    T::decode(body)
}

pub fn write<T: Message>(output: &mut impl Write, value: &T) -> Result<(), Error> {
    output
        .write_all(&value.encode()?)
        .map_err(FrameError::from)?;
    output.flush().map_err(FrameError::from)?;
    Ok(())
}

fn encode<M: ProstMessage + Layout>(value: &M, limits: &common::Limits) -> Result<Vec<u8>, Error> {
    let mut frame = Vec::new();
    protobuf::write_message(&mut frame, value, limits)?;
    Ok(frame)
}

fn decode<M: ProstMessage + Default + Layout>(
    body: Bytes,
    limits: &common::Limits,
    tag: u32,
) -> Result<M, Error> {
    check_length(&body, limits.message)?;
    let field = fields::single(&body)?;
    if field.number != tag {
        return Err(Error::Unexpected);
    }
    field.bytes()?;
    Ok(protobuf::decode(body, limits)?)
}

fn check_length(body: &[u8], limit: usize) -> Result<(), Error> {
    if body.len() > limit {
        return Err(FrameError::TooLarge {
            length: body.len() as u64,
            limit,
        }
        .into());
    }
    Ok(())
}

/// Request/reply cases cannot hide each other through last-one-wins decoding.
fn one_case(mut body: &[u8], tags: std::ops::RangeInclusive<u32>) -> Result<(), Error> {
    let mut found = false;
    let mut seen = std::collections::BTreeSet::new();
    while !body.is_empty() {
        let field = fields::field(&mut body)?;
        if !seen.insert(field.number) {
            return Err(Invalid("duplicate runtime field").into());
        }
        if tags.contains(&field.number) {
            if found {
                return Err(Invalid("ambiguous runtime result or operation").into());
            }
            field.bytes()?;
            found = true;
        }
    }
    if !found {
        return Err(Invalid("missing runtime result or operation").into());
    }
    Ok(())
}

macro_rules! message {
    ($semantic:ty, $frame:ident, $oneof:path, $tag:literal, $to:ident, $from:ident, $limits:expr) => {
        impl sealed::Sealed for $semantic {}
        impl Message for $semantic {
            const LIMITS: common::Limits = $limits;
            fn encode(&self) -> Result<Vec<u8>, Error> {
                self.validate()?;
                encode(
                    &pb::$frame {
                        message: Some($oneof(convert::$to(self)?)),
                    },
                    &Self::LIMITS,
                )
            }
            fn decode(body: Bytes) -> Result<Self, Error> {
                let frame: pb::$frame = decode(body, &Self::LIMITS, $tag)?;
                let Some($oneof(value)) = frame.message else {
                    return Err(Error::Unexpected);
                };
                let value = convert::$from(value)?;
                value.validate()?;
                Ok(value)
            }
        }
    };
}
message!(
    Hello,
    ServerFrame,
    pb::server_frame::Message::Hello,
    1,
    hello,
    from_hello,
    LIMITS
);
message!(
    Attach,
    LeaseWorkerFrame,
    pb::lease_worker_frame::Message::Attach,
    1,
    attach,
    from_attach,
    LIMITS
);
message!(
    Applied,
    LeaseWorkerFrame,
    pb::lease_worker_frame::Message::Applied,
    2,
    applied,
    from_applied,
    LIMITS
);
message!(
    WorkerPermissions,
    LeaseRuntimeFrame,
    pb::lease_runtime_frame::Message::Permissions,
    2,
    permissions,
    from_permissions,
    LIMITS
);
message!(
    admission::Open,
    AdmissionClientFrame,
    pb::admission_client_frame::Message::Open,
    1,
    open,
    from_open,
    admission::LIMITS
);

macro_rules! result_message {
    ($semantic:ty, $frame:ident, $oneof:path, $tag:literal, $range:expr, $to:ident, $from:ident) => {
        impl sealed::Sealed for $semantic {}
        impl Message for $semantic {
            const LIMITS: common::Limits = LIMITS;
            fn encode(&self) -> Result<Vec<u8>, Error> {
                self.validate()?;
                encode(
                    &pb::$frame {
                        message: Some($oneof(convert::$to(self)?)),
                    },
                    &Self::LIMITS,
                )
            }
            fn decode(body: Bytes) -> Result<Self, Error> {
                check_length(&body, Self::LIMITS.message)?;
                let field = fields::single(&body)?;
                one_case(field.bytes()?, $range)?;
                let frame: pb::$frame = decode(body, &Self::LIMITS, $tag)?;
                let Some($oneof(value)) = frame.message else {
                    return Err(Error::Unexpected);
                };
                let value = convert::$from(value)?;
                value.validate()?;
                Ok(value)
            }
        }
    };
}
result_message!(
    Request,
    ClientFrame,
    pb::client_frame::Message::Request,
    1,
    3..=8,
    request,
    from_request
);
result_message!(
    Reply,
    ServerFrame,
    pb::server_frame::Message::Reply,
    2,
    3..=7,
    reply,
    from_reply
);
result_message!(
    Attached,
    LeaseRuntimeFrame,
    pb::lease_runtime_frame::Message::Attached,
    1,
    3..=4,
    attached,
    from_attached
);

impl sealed::Sealed for admission::Admission {}
impl Message for admission::Admission {
    const LIMITS: common::Limits = LIMITS;
    fn encode(&self) -> Result<Vec<u8>, Error> {
        self.validate()?;
        use pb::admission_server_frame::Result;
        let result = match self {
            Self::Started(started) => Result::Started(pb::Started {
                runtime_id: started.runtime_id.clone(),
                worker_id: started.worker_id.clone(),
                build_sha256: started.build_sha256.clone(),
            }),
            Self::Refused(error) => Result::Error(convert::error(error)),
        };
        encode(
            &pb::AdmissionServerFrame {
                result: Some(result),
            },
            &Self::LIMITS,
        )
    }
    fn decode(body: Bytes) -> Result<Self, Error> {
        check_length(&body, Self::LIMITS.message)?;
        let field = fields::single(&body)?;
        let frame: pb::AdmissionServerFrame = decode(body.clone(), &Self::LIMITS, field.number)?;
        use pb::admission_server_frame::Result;
        let value = match frame.result {
            Some(Result::Started(value)) => Self::Started(admission::Started {
                runtime_id: value.runtime_id,
                worker_id: value.worker_id,
                build_sha256: value.build_sha256,
            }),
            Some(Result::Error(error)) => Self::Refused(convert::from_error(error)?),
            None => return Err(Error::Unexpected),
        };
        value.validate()?;
        Ok(value)
    }
}
