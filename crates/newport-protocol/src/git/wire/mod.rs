//! Directional Git messages and the typed body carried by acknowledged diff streams.
use super::{ClientMessage, Diff, Invalid, MAX_DIFF, MAX_FRAME, ServerMessage};
use crate::{
    common::Limits,
    encoding::protobuf::{self, FrameError, generated::newport::git as pb},
};
use bytes::Bytes;
use prost::Message as ProstMessage;
use std::io::{self, Read, Write};

pub(crate) mod convert;
mod scan;
#[cfg(test)]
mod tests;

pub const FRAME_LIMITS: Limits = Limits {
    message: MAX_FRAME,
    metadata: MAX_FRAME,
};
const DIFF_LIMITS: Limits = Limits {
    message: MAX_DIFF,
    metadata: MAX_DIFF,
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Frame(#[from] FrameError),
    #[error(transparent)]
    Invalid(#[from] Invalid),
    #[error(transparent)]
    Control(#[from] crate::worker::wire::Error),
    #[error("Git stream ended")]
    Ended,
}

impl From<Error> for io::Error {
    fn from(error: Error) -> Self {
        match error {
            Error::Frame(FrameError::Io(error)) => error,
            Error::Ended | Error::Frame(FrameError::Truncated) => {
                Self::new(io::ErrorKind::UnexpectedEof, error)
            }
            _ => Self::new(io::ErrorKind::InvalidData, error),
        }
    }
}

mod sealed {
    pub trait Sealed {}
}
pub trait Message: sealed::Sealed + Sized {
    fn encode(&self) -> Result<Vec<u8>, Error>;
    fn decode(body: Bytes) -> Result<Self, Error>;
}

pub fn read<T: Message>(input: &mut impl Read) -> Result<T, Error> {
    T::decode(protobuf::read_frame(input, MAX_FRAME)?.ok_or(Error::Ended)?)
}

pub fn write<T: Message>(output: &mut impl Write, value: &T) -> Result<(), Error> {
    output
        .write_all(&value.encode()?)
        .map_err(FrameError::from)?;
    output.flush().map_err(FrameError::from)?;
    Ok(())
}

fn encode(value: &(impl ProstMessage + protobuf::Layout)) -> Result<Vec<u8>, Error> {
    let mut bytes = vec![];
    protobuf::write_message(&mut bytes, value, &FRAME_LIMITS)?;
    Ok(bytes)
}

impl sealed::Sealed for ClientMessage {}
impl Message for ClientMessage {
    fn encode(&self) -> Result<Vec<u8>, Error> {
        use pb::client_frame::Message;
        self.validate()?;
        let message = match self {
            Self::Configure(value) => Message::Configure(convert::configure_to_wire(value)?),
            Self::Call(value) => Message::Call(convert::call_to_wire(value)?),
            Self::Ping(value) => Message::Ping(value.clone()),
            Self::Ack(value) => Message::Ack(convert::ack_to_wire(value)?),
        };
        encode(&pb::ClientFrame {
            message: Some(message),
        })
    }

    fn decode(body: Bytes) -> Result<Self, Error> {
        use pb::client_frame::Message;
        crate::worker::wire::reject_controls(&body, MAX_FRAME)?;
        scan::client(&body)?;
        let value: pb::ClientFrame = protobuf::decode(body, &FRAME_LIMITS)?;
        let message = match value.message.ok_or(Invalid("client operation"))? {
            Message::Configure(value) => Self::Configure(convert::configure_from_wire(value)?),
            Message::Call(value) => Self::Call(convert::call_from_wire(value)?),
            Message::Ping(value) => Self::Ping(value),
            Message::Ack(value) => Self::Ack(convert::ack_from_wire(value)?),
            Message::WorkerControl(_) => return Err(Invalid("worker control").into()),
        };
        message.validate()?;
        Ok(message)
    }
}

impl sealed::Sealed for ServerMessage {}
impl Message for ServerMessage {
    fn encode(&self) -> Result<Vec<u8>, Error> {
        use pb::server_frame::Message;
        self.validate()?;
        let message = match self {
            Self::Ready(value) => Message::Ready(convert::ready_to_wire(value)?),
            Self::Response(value) => Message::Response(convert::response_to_wire(value)?),
            Self::Pong(value) => Message::Pong(value.clone()),
            Self::Begin(value) => Message::Begin(convert::begin_to_wire(value)?),
            Self::Chunk(value) => Message::Chunk(convert::chunk_to_wire(value)?),
            Self::Log(value) => Message::Log(convert::log_to_wire(value)?),
            Self::Failed(value) => Message::Failed(convert::failure_to_wire(value)?),
        };
        encode(&pb::ServerFrame {
            message: Some(message),
        })
    }

    fn decode(body: Bytes) -> Result<Self, Error> {
        use pb::server_frame::Message;
        crate::worker::wire::reject_controls(&body, MAX_FRAME)?;
        scan::server(&body)?;
        let value: pb::ServerFrame = protobuf::decode(body, &FRAME_LIMITS)?;
        let message = match value.message.ok_or(Invalid("server operation"))? {
            Message::Ready(value) => Self::Ready(convert::ready_from_wire(value)?),
            Message::Response(value) => Self::Response(convert::response_from_wire(value)?),
            Message::Pong(value) => Self::Pong(value),
            Message::Begin(value) => Self::Begin(convert::begin_from_wire(value)?),
            Message::Chunk(value) => Self::Chunk(convert::chunk_from_wire(value)?),
            Message::Log(value) => Self::Log(convert::log_from_wire(value)?),
            Message::Failed(value) => Self::Failed(convert::failure_from_wire(value)?),
            Message::WorkerControl(_) => return Err(Invalid("worker control").into()),
        };
        message.validate()?;
        Ok(message)
    }
}

pub fn encode_diff(value: &Diff) -> Result<Vec<u8>, Error> {
    value.validate()?;
    let value = convert::diff_to_wire(value)?;
    let length = value.encoded_len();
    if length > MAX_DIFF {
        return Err(FrameError::TooLarge {
            length: length as u64,
            limit: MAX_DIFF,
        }
        .into());
    }
    let body = value.encode_to_vec();
    scan::diff(&body)?;
    Ok(body)
}

pub fn decode_diff(body: Bytes) -> Result<Diff, Error> {
    if body.len() > MAX_DIFF {
        return Err(FrameError::TooLarge {
            length: body.len() as u64,
            limit: MAX_DIFF,
        }
        .into());
    }
    scan::diff(&body)?;
    let value = convert::diff_from_wire(protobuf::decode(body, &DIFF_LIMITS)?)?;
    value.validate()?;
    Ok(value)
}
