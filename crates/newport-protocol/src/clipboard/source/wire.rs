//! Prost-generated read-only clipboard source frames.
use super::*;
use crate::encoding::protobuf::{
    self, FrameError, fields,
    generated::newport::{
        clipboard as clipboard_pb, clipboard::source as pb, integration as integration_pb,
    },
};
use bytes::Bytes;
use std::io::{self, Read, Write};
mod scan;
#[cfg(test)]
mod tests;
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Frame(#[from] FrameError),
    #[error(transparent)]
    Invalid(#[from] Invalid),
    #[error("stream ended before the required message")]
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
fn encode<M: prost::Message + protobuf::Layout>(
    message: &M,
    limits: &common::Limits,
) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    protobuf::write_message(&mut bytes, message, limits)?;
    Ok(bytes)
}
fn bounded(body: &[u8], limits: &common::Limits) -> Result<(), Error> {
    if body.len() > limits.message {
        return Err(FrameError::TooLarge {
            length: body.len() as u64,
            limit: limits.message,
        }
        .into());
    }
    Ok(())
}
fn failure_to_wire(value: &common::Error) -> protobuf::Error {
    protobuf::Error {
        code: value.code.clone(),
        message: value.message.clone(),
        retry: match value.retry {
            common::Retry::Unknown => 0,
            common::Retry::Yes => 1,
            common::Retry::No => 2,
        },
    }
}
fn failure_from_wire(value: protobuf::Error) -> Result<common::Error, Error> {
    Ok(common::Error {
        code: value.code,
        message: value.message,
        retry: match value.retry {
            0 => common::Retry::Unknown,
            1 => common::Retry::Yes,
            2 => common::Retry::No,
            _ => return Err(Invalid("retry").into()),
        },
    })
}

pub fn encode_request(request: &Request) -> Result<Vec<u8>, Error> {
    use pb::client_frame::Request as R;
    request.validate()?;
    let request = match request {
        Request::Offer => R::Offer(protobuf::Present {}),
        Request::Read { revision, format } => R::Read(pb::Read {
            revision: *revision,
            format: format.clone(),
        }),
    };
    encode(
        &pb::ClientFrame {
            request: Some(request),
        },
        &CLIENT_LIMITS,
    )
}
pub fn decode_request(body: Bytes) -> Result<Request, Error> {
    use pb::client_frame::Request as R;
    bounded(&body, &CLIENT_LIMITS)?;
    scan::request(&body)?;
    let frame: pb::ClientFrame = protobuf::decode(body, &CLIENT_LIMITS)?;
    let request = match frame.request.ok_or(Invalid("operation"))? {
        R::Offer(_) => Request::Offer,
        R::Read(value) => Request::Read {
            revision: value.revision,
            format: value.format,
        },
    };
    request.validate()?;
    Ok(request)
}
pub fn encode_response(response: &Response) -> Result<Vec<u8>, Error> {
    use pb::server_frame::Response as R;
    response.validate()?;
    let response = match response {
        Response::Hello { version } => R::Hello(pb::Hello { version: *version }),
        Response::Offer(value) => R::Offer(integration_pb::ClipboardOffer {
            revision: value.revision,
            formats: value.formats.clone(),
        }),
        Response::Chunk(value) => R::Chunk(clipboard_pb::Chunk {
            data: Bytes::copy_from_slice(&value.bytes),
            eof: value.eof,
        }),
        Response::Failed(value) => R::Error(failure_to_wire(value)),
    };
    encode(
        &pb::ServerFrame {
            response: Some(response),
        },
        &SERVER_LIMITS,
    )
}
pub fn decode_response(body: Bytes) -> Result<Response, Error> {
    use pb::server_frame::Response as R;
    bounded(&body, &SERVER_LIMITS)?;
    scan::response(&body)?;
    let frame: pb::ServerFrame = protobuf::decode(body, &SERVER_LIMITS)?;
    let response = match frame.response.ok_or(Invalid("operation"))? {
        R::Hello(value) => Response::Hello {
            version: value.version,
        },
        R::Offer(value) => Response::Offer(imports::Offer {
            revision: value.revision,
            formats: value.formats,
        }),
        R::Chunk(value) => Response::Chunk(Chunk {
            bytes: value.data.to_vec(),
            eof: value.eof,
        }),
        R::Error(value) => Response::Failed(failure_from_wire(value)?),
    };
    response.validate()?;
    Ok(response)
}
pub fn read_request(input: &mut impl Read) -> Result<Request, Error> {
    decode_request(protobuf::read_frame(input, CLIENT_LIMITS.message)?.ok_or(Error::Ended)?)
}
pub fn read_response(input: &mut impl Read) -> Result<Response, Error> {
    decode_response(protobuf::read_frame(input, SERVER_LIMITS.message)?.ok_or(Error::Ended)?)
}
pub fn write_request(output: &mut impl Write, request: &Request) -> Result<(), Error> {
    output
        .write_all(&encode_request(request)?)
        .map_err(FrameError::from)?;
    output.flush().map_err(FrameError::from)?;
    Ok(())
}
pub fn write_response(output: &mut impl Write, response: &Response) -> Result<(), Error> {
    output
        .write_all(&encode_response(response)?)
        .map_err(FrameError::from)?;
    output.flush().map_err(FrameError::from)?;
    Ok(())
}
