//! Bounded protobuf messages for one local browser open.
use super::{Invalid, LIMITS, Reply, Request, ServerMessage};
use crate::{
    common,
    encoding::protobuf::{self, FrameError, fields, generated::newport::browser as pb},
};
use bytes::Bytes;
use std::io::{self, Read, Write};

#[cfg(test)]
mod tests;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Frame(#[from] FrameError),
    #[error(transparent)]
    Invalid(#[from] Invalid),
    #[error("browser owner ended the stream")]
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

pub fn write_request(output: &mut impl Write, request: &Request) -> Result<(), Error> {
    request.validate()?;
    protobuf::write_message(
        output,
        &pb::ClientFrame {
            open: Some(pb::Open {
                url: request.url.clone(),
            }),
        },
        &LIMITS,
    )?;
    Ok(())
}

pub fn read_request(input: &mut impl Read) -> Result<Request, Error> {
    decode_request(protobuf::read_frame(input, LIMITS.message)?.ok_or(Error::Ended)?)
}

pub fn decode_request(body: Bytes) -> Result<Request, Error> {
    bounded(&body)?;
    let field = fields::single(&body)?;
    let known = request_fields(field.number).ok_or(Invalid("request operation"))?;
    scan(field.bytes()?, known)?;
    let frame: pb::ClientFrame = protobuf::decode(body, &LIMITS)?;
    let request = Request {
        url: frame.open.ok_or(Invalid("open request"))?.url,
    };
    request.validate()?;
    Ok(request)
}

pub fn write_server(output: &mut impl Write, message: &ServerMessage) -> Result<(), Error> {
    use pb::server_frame::Message;
    message.validate()?;
    let message = match message {
        ServerMessage::Hello { version } => Message::Hello(pb::Hello { version: *version }),
        ServerMessage::Reply(Reply::Opened { warning }) => Message::Opened(pb::Opened {
            warning: warning.clone(),
        }),
        ServerMessage::Reply(Reply::Failed(error)) => Message::Error(protobuf::Error {
            code: error.code.clone(),
            message: error.message.clone(),
            retry: match error.retry {
                common::Retry::Unknown => 0,
                common::Retry::Yes => 1,
                common::Retry::No => 2,
            },
        }),
    };
    protobuf::write_message(
        output,
        &pb::ServerFrame {
            message: Some(message),
        },
        &LIMITS,
    )?;
    Ok(())
}

pub fn read_server(input: &mut impl Read) -> Result<ServerMessage, Error> {
    decode_server(protobuf::read_frame(input, LIMITS.message)?.ok_or(Error::Ended)?)
}

pub fn decode_server(body: Bytes) -> Result<ServerMessage, Error> {
    use pb::server_frame::Message;
    bounded(&body)?;
    let field = fields::single(&body)?;
    let known = server_fields(field.number).ok_or(Invalid("response operation"))?;
    scan(field.bytes()?, known)?;
    let frame: pb::ServerFrame = protobuf::decode(body, &LIMITS)?;
    let message = match frame.message.ok_or(Invalid("response operation"))? {
        Message::Hello(value) => ServerMessage::Hello {
            version: value.version,
        },
        Message::Opened(value) => ServerMessage::Reply(Reply::Opened {
            warning: value.warning,
        }),
        Message::Error(value) => ServerMessage::Reply(Reply::Failed(common::Error {
            code: value.code,
            message: value.message,
            retry: match value.retry {
                0 => common::Retry::Unknown,
                1 => common::Retry::Yes,
                2 => common::Retry::No,
                _ => return Err(Invalid("retry value").into()),
            },
        })),
    };
    message.validate()?;
    Ok(message)
}

fn bounded(body: &[u8]) -> Result<(), Error> {
    if body.len() > LIMITS.message {
        return Err(FrameError::TooLarge {
            length: body.len() as u64,
            limit: LIMITS.message,
        }
        .into());
    }
    Ok(())
}

/// The fields a request's case in field `number` knows: number, and whether
/// it is an integer rather than text.
fn request_fields(number: u32) -> Option<&'static [(u32, bool)]> {
    Some(match number {
        1 => &[(1, false)],
        _ => return None,
    })
}
/// The fields a server message's case in field `number` knows.
fn server_fields(number: u32) -> Option<&'static [(u32, bool)]> {
    Some(match number {
        1 => &[(1, true)],
        2 => &[(1, false)],
        3 => &[(1, false), (2, false), (3, true)],
        _ => return None,
    })
}

#[cfg(test)]
pub(crate) fn tables() -> Vec<(Table, &'static str)> {
    fn table(known: &'static [(u32, bool)]) -> Table {
        Table::new(format!("browser {known:?}"), move || {
            known
                .iter()
                .map(|&(number, integer)| {
                    Row::new(number, if integer { Wire::Varint32 } else { Wire::Len })
                })
                .collect()
        })
    }
    vec![
        (
            Table::dispatch("browser request", |number| {
                request_fields(number).map(table)
            }),
            "newport.browser.ClientFrame",
        ),
        (
            Table::dispatch("browser server", |number| server_fields(number).map(table)),
            "newport.browser.ServerFrame",
        ),
    ]
}

#[cfg(test)]
use crate::desktop::wire::wire_tables::{Row, Table, Wire};

fn scan(mut body: &[u8], allowed: &[(u32, bool)]) -> Result<(), Error> {
    let mut seen = 0u8;
    while !body.is_empty() {
        let field = fields::field(&mut body)?;
        let Some((index, (_, integer))) = allowed
            .iter()
            .enumerate()
            .find(|(_, (number, _))| *number == field.number)
        else {
            continue;
        };
        let bit = 1 << index;
        if seen & bit != 0 {
            return Err(Invalid("duplicate field").into());
        }
        seen |= bit;
        if *integer {
            field.u32()?;
        } else {
            field.bytes()?;
        }
    }
    Ok(())
}
