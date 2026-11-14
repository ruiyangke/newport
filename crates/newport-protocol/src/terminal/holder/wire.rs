//! Prost-generated terminal holder messages over shared bounded framing.
use super::*;
use crate::encoding::protobuf::{
    self, FrameError, fields,
    generated::newport::terminal::{self as terminal_pb, holder as pb},
};
use std::io::{self, Read, Write};
mod scan;
#[cfg(test)]
mod tests;
#[cfg(test)]
pub(crate) use scan::tables;
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

pub fn encode_client(message: &ClientMessage) -> Result<Vec<u8>, Error> {
    use pb::client_frame::Message as M;
    message.validate()?;
    let message = match message {
        ClientMessage::Attach(value) => M::Attach(pb::Attach {
            version: value.version,
            size: Some(pb::Size {
                columns: u32::from(value.columns),
                rows: u32::from(value.rows),
            }),
            control: value.control,
            opened: value.opened,
            computer: value.computer.as_ref().map(|value| terminal_pb::Computer {
                id: value.id.clone(),
                name: value.name.clone(),
            }),
            colors: value
                .colors
                .map(|[foreground, background, cursor]| pb::Colors {
                    foreground: rgb(foreground),
                    background: rgb(background),
                    cursor: rgb(cursor),
                }),
        }),
        ClientMessage::Input(data) => M::Input(pb::Input { data: data.clone() }),
        ClientMessage::Resize { columns, rows } => M::Resize(pb::Size {
            columns: u32::from(*columns),
            rows: u32::from(*rows),
        }),
        ClientMessage::End => M::End(protobuf::Present {}),
        ClientMessage::Rename(name) => M::Rename(pb::Rename { name: name.clone() }),
        ClientMessage::Activate => M::Activate(protobuf::Present {}),
    };
    encode(
        &pb::ClientFrame {
            message: Some(message),
        },
        &CLIENT_LIMITS,
    )
}
fn rgb([r, g, b]: [u8; 3]) -> u32 {
    u32::from_be_bytes([0, r, g, b])
}
fn color(value: u32) -> Result<[u8; 3], Error> {
    let [high, r, g, b] = value.to_be_bytes();
    if high != 0 {
        return Err(Invalid("color").into());
    }
    Ok([r, g, b])
}
fn size(value: pb::Size) -> Result<(u16, u16), Error> {
    Ok((
        value.columns.try_into().map_err(|_| Invalid("columns"))?,
        value.rows.try_into().map_err(|_| Invalid("rows"))?,
    ))
}
pub fn decode_client(body: Bytes) -> Result<ClientMessage, Error> {
    use pb::client_frame::Message as M;
    bounded(&body, &CLIENT_LIMITS)?;
    scan::client(&body)?;
    let value: pb::ClientFrame = protobuf::decode(body, &CLIENT_LIMITS)?;
    let message = match value.message.ok_or(Invalid("client operation"))? {
        M::Attach(value) => {
            let (columns, rows) = size(value.size.ok_or(Invalid("size"))?)?;
            ClientMessage::Attach(Attach {
                version: value.version,
                columns,
                rows,
                control: value.control,
                opened: value.opened,
                computer: value.computer.map(|value| Computer {
                    id: value.id,
                    name: value.name,
                }),
                colors: value
                    .colors
                    .map(|value| -> Result<_, Error> {
                        Ok([
                            color(value.foreground)?,
                            color(value.background)?,
                            color(value.cursor)?,
                        ])
                    })
                    .transpose()?,
            })
        }
        M::Input(value) => ClientMessage::Input(value.data),
        M::Resize(value) => {
            let (columns, rows) = size(value)?;
            ClientMessage::Resize { columns, rows }
        }
        M::End(_) => ClientMessage::End,
        M::Rename(value) => ClientMessage::Rename(value.name),
        M::Activate(_) => ClientMessage::Activate,
    };
    message.validate()?;
    Ok(message)
}
pub fn encode_server(message: &ServerMessage) -> Result<Vec<u8>, Error> {
    use pb::server_frame::Message as M;
    message.validate()?;
    let message = match message {
        ServerMessage::Hello(value) => M::Hello(pb::Hello {
            version: value.version,
            terminal_id: value.terminal_id.clone(),
        }),
        ServerMessage::Attached { prompted } => M::Attached(pb::Attached {
            prompted: *prompted,
        }),
        ServerMessage::Screen(data) => M::Screen(pb::Screen { data: data.clone() }),
        ServerMessage::Full(deflate) => M::Full(pb::FullSnapshot {
            deflate: deflate.clone(),
        }),
        ServerMessage::Output(data) => M::Output(pb::Output { data: data.clone() }),
        ServerMessage::Exit(value) => M::Exit(terminal_pb::Exit {
            outcome: Some(match value {
                Exit::Code(code) => terminal_pb::exit::Outcome::Code(*code),
                Exit::Signal(signal) => terminal_pb::exit::Outcome::Signal(*signal),
            }),
        }),
        ServerMessage::Active(active) => M::Active(pb::Active { active: *active }),
        ServerMessage::Failed(value) => M::Error(failure_to_wire(value)),
        ServerMessage::Renamed(name) => M::Renamed(pb::Renamed { name: name.clone() }),
        ServerMessage::Size { columns, rows } => M::Size(pb::Size {
            columns: u32::from(*columns),
            rows: u32::from(*rows),
        }),
    };
    encode(
        &pb::ServerFrame {
            message: Some(message),
        },
        &SERVER_LIMITS,
    )
}
pub fn decode_server(body: Bytes) -> Result<ServerMessage, Error> {
    use pb::server_frame::Message as M;
    bounded(&body, &SERVER_LIMITS)?;
    scan::server(&body)?;
    let value: pb::ServerFrame = protobuf::decode(body, &SERVER_LIMITS)?;
    let message = match value.message.ok_or(Invalid("server operation"))? {
        M::Hello(value) => ServerMessage::Hello(Hello {
            version: value.version,
            terminal_id: value.terminal_id,
        }),
        M::Attached(value) => ServerMessage::Attached {
            prompted: value.prompted,
        },
        M::Screen(value) => ServerMessage::Screen(value.data),
        M::Full(value) => ServerMessage::Full(value.deflate),
        M::Output(value) => ServerMessage::Output(value.data),
        M::Exit(value) => ServerMessage::Exit(match value.outcome.ok_or(Invalid("exit"))? {
            terminal_pb::exit::Outcome::Code(code) => Exit::Code(code),
            terminal_pb::exit::Outcome::Signal(signal) => Exit::Signal(signal),
        }),
        M::Active(value) => ServerMessage::Active(value.active),
        M::Error(value) => ServerMessage::Failed(failure_from_wire(value)?),
        M::Renamed(value) => ServerMessage::Renamed(value.name),
        M::Size(value) => {
            let (columns, rows) = size(value)?;
            ServerMessage::Size { columns, rows }
        }
    };
    message.validate()?;
    Ok(message)
}
pub fn read_client(input: &mut impl Read) -> Result<ClientMessage, Error> {
    decode_client(protobuf::read_frame(input, CLIENT_LIMITS.message)?.ok_or(Error::Ended)?)
}
pub fn read_server(input: &mut impl Read) -> Result<ServerMessage, Error> {
    decode_server(protobuf::read_frame(input, SERVER_LIMITS.message)?.ok_or(Error::Ended)?)
}
pub fn write_client(output: &mut impl Write, message: &ClientMessage) -> Result<(), Error> {
    output
        .write_all(&encode_client(message)?)
        .map_err(FrameError::from)?;
    output.flush().map_err(FrameError::from)?;
    Ok(())
}
pub fn write_server(output: &mut impl Write, message: &ServerMessage) -> Result<(), Error> {
    output
        .write_all(&encode_server(message)?)
        .map_err(FrameError::from)?;
    output.flush().map_err(FrameError::from)?;
    Ok(())
}
