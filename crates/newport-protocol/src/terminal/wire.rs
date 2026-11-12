//! Bounded directional terminal messages. Worker controls belong to the adapter.
use super::*;
use crate::encoding::protobuf::{self, FrameError, fields, generated::newport::terminal as pb};
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
    #[error(transparent)]
    Control(#[from] crate::worker::wire::Error),
    #[error("terminal stream ended")]
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

fn check(body: &[u8], limits: &common::Limits) -> Result<(), Error> {
    crate::worker::wire::reject_controls(body, limits.message)?;
    fields::single(body)?.bytes()?;
    Ok(())
}
fn encode<M: prost::Message + protobuf::Layout>(
    value: &M,
    limits: &common::Limits,
) -> Result<Vec<u8>, Error> {
    let mut frame = Vec::new();
    protobuf::write_message(&mut frame, value, limits)?;
    Ok(frame)
}

pub fn encode_request(request: &Request) -> Result<Vec<u8>, Error> {
    use pb::client_frame::Message;
    request.validate()?;
    let message = match request {
        Request::Start {
            id,
            columns,
            rows,
            command,
            creator,
            stable_display,
        } => Message::Start(pb::Start {
            id: id.clone(),
            columns: u32::from(*columns),
            rows: u32::from(*rows),
            command: command.clone(),
            creator: Some(computer_to_wire(creator)),
            stable_display: *stable_display,
        }),
        Request::List => Message::List(protobuf::Present {}),
        Request::Watch => Message::Watch(protobuf::Present {}),
        Request::Rename { id, name } => Message::Rename(pb::Rename {
            id: id.clone(),
            name: name.clone(),
        }),
        Request::End { id } => Message::End(pb::End { id: id.clone() }),
    };
    encode(
        &pb::ClientFrame {
            message: Some(message),
        },
        &CLIENT_LIMITS,
    )
}

pub fn decode_request(body: Bytes) -> Result<Request, Error> {
    use pb::client_frame::Message;
    check(&body, &CLIENT_LIMITS)?;
    let frame: pb::ClientFrame = protobuf::decode(body, &CLIENT_LIMITS)?;
    let request = match frame.message.ok_or(Invalid("request operation"))? {
        Message::Start(value) => Request::Start {
            id: value.id,
            columns: size(value.columns)?,
            rows: size(value.rows)?,
            command: value.command,
            creator: computer_from_wire(value.creator)?,
            stable_display: value.stable_display,
        },
        Message::List(_) => Request::List,
        Message::Watch(_) => Request::Watch,
        Message::Rename(value) => Request::Rename {
            id: value.id,
            name: value.name,
        },
        Message::End(value) => Request::End { id: value.id },
        Message::WorkerControl(_) => return Err(Invalid("worker control").into()),
    };
    request.validate()?;
    Ok(request)
}

pub fn encode_reply(reply: &Reply) -> Result<Vec<u8>, Error> {
    use pb::server_frame::Message;
    reply.validate()?;
    let message = match reply {
        Reply::Started { socket } => Message::Started(pb::Started {
            socket: socket.clone(),
        }),
        Reply::Listed(holders) => Message::Listed(pb::Listed {
            holders: holders.iter().map(holder_to_wire).collect(),
        }),
        Reply::Renamed { renamed } => Message::Renamed(pb::Renamed { renamed: *renamed }),
        Reply::Ended { ended } => Message::Ended(pb::Ended { ended: *ended }),
        Reply::Failed(error) => Message::Error(protobuf::Error {
            code: error.code.clone(),
            message: error.message.clone(),
            retry: match error.retry {
                common::Retry::Unknown => 0,
                common::Retry::Yes => 1,
                common::Retry::No => 2,
            },
        }),
    };
    encode(
        &pb::ServerFrame {
            message: Some(message),
        },
        &SERVER_LIMITS,
    )
}

pub fn decode_reply(body: Bytes) -> Result<Reply, Error> {
    use pb::server_frame::Message;
    check(&body, &SERVER_LIMITS)?;
    check_exits(&body)?;
    let frame: pb::ServerFrame = protobuf::decode(body, &SERVER_LIMITS)?;
    let reply = match frame.message.ok_or(Invalid("reply operation"))? {
        Message::Started(value) => Reply::Started {
            socket: value.socket,
        },
        Message::Listed(value) => Reply::Listed(
            value
                .holders
                .into_iter()
                .map(holder_from_wire)
                .collect::<Result<_, _>>()?,
        ),
        Message::Renamed(value) => Reply::Renamed {
            renamed: value.renamed,
        },
        Message::Ended(value) => Reply::Ended { ended: value.ended },
        Message::Error(error) => Reply::Failed(common::Error {
            code: error.code,
            message: error.message,
            retry: match error.retry {
                0 => common::Retry::Unknown,
                1 => common::Retry::Yes,
                2 => common::Retry::No,
                _ => return Err(Invalid("retry value").into()),
            },
        }),
        Message::WorkerControl(_) => return Err(Invalid("worker control").into()),
    };
    reply.validate()?;
    Ok(reply)
}

// A shell must have exactly one exit outcome, including across repeated fields.
fn check_exits(body: &[u8]) -> Result<(), Error> {
    let frame = fields::single(body)?;
    if frame.number != 2 {
        return Ok(());
    }
    let mut list = frame.bytes()?;
    let mut holders = 0;
    while !list.is_empty() {
        let holder = fields::field(&mut list)?;
        if holder.number != 1 {
            continue;
        }
        holders += 1;
        if holders > MAX_HOLDERS {
            return Err(Invalid("holder count").into());
        }
        let mut holder = holder.bytes()?;
        let mut exited = false;
        let mut attached = 0;
        while !holder.is_empty() {
            let field = fields::field(&mut holder)?;
            if field.number == 10 {
                attached += 1;
                if attached > MAX_ATTACHED {
                    return Err(Invalid("attachment count").into());
                }
            }
            if field.number == 11 {
                if exited {
                    return Err(Invalid("duplicate exit").into());
                }
                let outcome = fields::single(field.bytes()?)?;
                if !matches!(outcome.number, 1 | 2) {
                    return Err(Invalid("exit outcome").into());
                }
                outcome.u32()?;
                exited = true;
            }
        }
    }
    Ok(())
}

pub fn read_request(input: &mut impl Read) -> Result<Request, Error> {
    decode_request(protobuf::read_frame(input, CLIENT_LIMITS.message)?.ok_or(Error::Ended)?)
}
pub fn read_reply(input: &mut impl Read) -> Result<Reply, Error> {
    decode_reply(protobuf::read_frame(input, SERVER_LIMITS.message)?.ok_or(Error::Ended)?)
}
pub fn write_reply(output: &mut impl Write, reply: &Reply) -> Result<(), Error> {
    output
        .write_all(&encode_reply(reply)?)
        .map_err(FrameError::from)?;
    output.flush().map_err(FrameError::from)?;
    Ok(())
}

fn size(value: u32) -> Result<u16, Error> {
    u16::try_from(value).map_err(|_| Invalid("size").into())
}
fn computer_to_wire(value: &Computer) -> pb::Computer {
    pb::Computer {
        id: value.id.clone(),
        name: value.name.clone(),
    }
}
fn computer_from_wire(value: Option<pb::Computer>) -> Result<Computer, Error> {
    let value = value.ok_or(Invalid("computer"))?;
    Ok(Computer {
        id: value.id,
        name: value.name,
    })
}
fn holder_to_wire(value: &Holder) -> pb::Holder {
    pb::Holder {
        id: value.id.clone(),
        socket: value.socket.clone(),
        version: value.version,
        columns: u32::from(value.columns),
        rows: u32::from(value.rows),
        started: value.started,
        name: value.name.clone(),
        title: value.title.clone(),
        creator: Some(computer_to_wire(&value.creator)),
        attached: value
            .attached
            .iter()
            .map(|value| pb::Attached {
                id: value.id.clone(),
                name: value.name.clone(),
                active: value.active,
            })
            .collect(),
        exited: value.exited.map(|value| pb::Exit {
            outcome: Some(match value {
                Exit::Code(code) => pb::exit::Outcome::Code(code),
                Exit::Signal(signal) => pb::exit::Outcome::Signal(signal),
            }),
        }),
    }
}
fn holder_from_wire(value: pb::Holder) -> Result<Holder, Error> {
    Ok(Holder {
        id: value.id,
        socket: value.socket,
        version: value.version,
        columns: size(value.columns)?,
        rows: size(value.rows)?,
        started: value.started,
        name: value.name,
        title: value.title,
        creator: computer_from_wire(value.creator)?,
        attached: value
            .attached
            .into_iter()
            .map(|value| Attached {
                id: value.id,
                name: value.name,
                active: value.active,
            })
            .collect(),
        exited: value
            .exited
            .map(|value| match value.outcome {
                Some(pb::exit::Outcome::Code(code)) => Ok(Exit::Code(code)),
                Some(pb::exit::Outcome::Signal(signal)) => Ok(Exit::Signal(signal)),
                None => Err(Invalid("exit outcome")),
            })
            .transpose()?,
    })
}
