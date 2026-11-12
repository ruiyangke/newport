//! Generated protobuf messages with shared bounded framing.
use super::*;
use crate::encoding::protobuf::{
    self, FrameError, fields, generated::newport::clipboard::helper as pb,
};
use bytes::Bytes;
use std::io;
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Frame(#[from] FrameError),
    #[error(transparent)]
    Invalid(#[from] Invalid),
}
impl From<Error> for io::Error {
    fn from(value: Error) -> Self {
        match value {
            Error::Frame(FrameError::Io(error)) => error,
            Error::Frame(FrameError::Truncated) => Self::new(io::ErrorKind::UnexpectedEof, value),
            _ => Self::new(io::ErrorKind::InvalidData, value),
        }
    }
}
pub trait Message: Sized {
    fn encode(&self, data: &[u8]) -> Result<Vec<u8>, Error>;
    fn decode(body: Bytes) -> Result<Frame<Self>, Error>;
}
pub fn encode<T: Message>(message: &T, data: &[u8]) -> Result<Vec<u8>, Error> {
    message.encode(data)
}
pub fn decode<T: Message>(body: Bytes) -> Result<Frame<T>, Error> {
    T::decode(body)
}
fn encoded<T: prost::Message + protobuf::Layout>(value: &T) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    protobuf::write_message(&mut bytes, value, &LIMITS)?;
    Ok(bytes)
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
impl Message for Command {
    fn encode(&self, data: &[u8]) -> Result<Vec<u8>, Error> {
        use pb::command_frame::Command as C;
        self.validate(data)?;
        let command = match self {
            Self::Hello { version, board } => C::Hello(pb::Hello {
                version: *version,
                board: Some(pb::Board {
                    kind: Some(match board {
                        Board::General {} => pb::board::Kind::General(protobuf::Present {}),
                        Board::Test { name } => pb::board::Kind::Test(name.clone()),
                    }),
                }),
            }),
            Self::Publish {
                generation,
                expected_revision,
                formats,
            } => C::Publish(pb::Publish {
                generation: *generation,
                expected_revision: *expected_revision,
                formats: formats.clone(),
            }),
            Self::AttachFiles {
                generation,
                expected_revision,
                folder,
            } => C::AttachFiles(pb::AttachFiles {
                generation: *generation,
                expected_revision: *expected_revision,
                folder: folder.clone(),
            }),
            Self::Revoke { generation } => C::Revoke(pb::Generation {
                generation: *generation,
            }),
            Self::Data {
                generation,
                id,
                offset,
                eof,
            } => C::Data(pb::Data {
                generation: *generation,
                id: *id,
                offset: *offset,
                eof: *eof,
                data: Bytes::copy_from_slice(data),
            }),
            Self::Failed { generation, id } => C::Failed(pb::Demand {
                generation: *generation,
                id: *id,
            }),
            Self::ReadLocal {
                id,
                revision,
                format,
            } => C::ReadLocal(pb::ReadLocal {
                id: *id,
                revision: *revision,
                format: format.clone(),
            }),
            Self::CancelLocal { id } => C::CancelLocal(pb::Id { id: *id }),
            Self::Stop {} => C::Stop(protobuf::Present {}),
        };
        encoded(&pb::CommandFrame {
            command: Some(command),
        })
    }
    fn decode(body: Bytes) -> Result<Frame<Self>, Error> {
        use pb::command_frame::Command as C;
        bounded(&body)?;
        scan_frame(&body, true)?;
        let frame: pb::CommandFrame = protobuf::decode(body, &LIMITS)?;
        let mut bytes = Vec::new();
        let header = match frame.command.ok_or(Invalid("command"))? {
            C::Hello(value) => Self::Hello {
                version: value.version,
                board: match value
                    .board
                    .and_then(|board| board.kind)
                    .ok_or(Invalid("board"))?
                {
                    pb::board::Kind::General(_) => Board::General {},
                    pb::board::Kind::Test(name) => Board::Test { name },
                },
            },
            C::Publish(value) => Self::Publish {
                generation: value.generation,
                expected_revision: value.expected_revision,
                formats: value.formats,
            },
            C::AttachFiles(value) => Self::AttachFiles {
                generation: value.generation,
                expected_revision: value.expected_revision,
                folder: value.folder,
            },
            C::Revoke(value) => Self::Revoke {
                generation: value.generation,
            },
            C::Data(value) => {
                bytes = value.data.to_vec();
                Self::Data {
                    generation: value.generation,
                    id: value.id,
                    offset: value.offset,
                    eof: value.eof,
                }
            }
            C::Failed(value) => Self::Failed {
                generation: value.generation,
                id: value.id,
            },
            C::ReadLocal(value) => Self::ReadLocal {
                id: value.id,
                revision: value.revision,
                format: value.format,
            },
            C::CancelLocal(value) => Self::CancelLocal { id: value.id },
            C::Stop(_) => Self::Stop {},
        };
        header.validate(&bytes)?;
        Ok(Frame { header, bytes })
    }
}
impl Message for Event {
    fn encode(&self, data: &[u8]) -> Result<Vec<u8>, Error> {
        use pb::event_frame::Event as E;
        self.validate(data)?;
        let event = match self {
            Self::Ready { version } => E::Ready(pb::Ready { version: *version }),
            Self::Snapshot {
                revision,
                formats,
                origin,
            } => E::Snapshot(pb::Snapshot {
                revision: *revision,
                formats: formats.clone(),
                origin: *origin,
            }),
            Self::Published {
                generation,
                revision,
            } => E::Published(pb::Published {
                generation: *generation,
                revision: *revision,
            }),
            Self::Attached {
                generation,
                revision,
            } => E::Attached(pb::Revision {
                generation: *generation,
                revision: *revision,
            }),
            Self::Rejected { generation } => E::Rejected(pb::Generation {
                generation: *generation,
            }),
            Self::Revoked {
                generation,
                revision,
            } => E::Revoked(pb::Revision {
                generation: *generation,
                revision: *revision,
            }),
            Self::Read {
                generation,
                id,
                format,
            } => E::Read(pb::Read {
                generation: *generation,
                id: *id,
                format: format.clone(),
            }),
            Self::Cancelled { generation, id } => E::Cancelled(pb::Demand {
                generation: *generation,
                id: *id,
            }),
            Self::LocalData {
                id,
                revision,
                offset,
                eof,
            } => E::LocalData(pb::LocalData {
                id: *id,
                revision: *revision,
                offset: *offset,
                eof: *eof,
                data: Bytes::copy_from_slice(data),
            }),
            Self::LocalFailed { id } => E::LocalFailed(pb::Id { id: *id }),
        };
        encoded(&pb::EventFrame { event: Some(event) })
    }
    fn decode(body: Bytes) -> Result<Frame<Self>, Error> {
        use pb::event_frame::Event as E;
        bounded(&body)?;
        scan_frame(&body, false)?;
        let frame: pb::EventFrame = protobuf::decode(body, &LIMITS)?;
        let mut bytes = Vec::new();
        let header = match frame.event.ok_or(Invalid("event"))? {
            E::Ready(value) => Self::Ready {
                version: value.version,
            },
            E::Snapshot(value) => Self::Snapshot {
                revision: value.revision,
                formats: value.formats,
                origin: value.origin,
            },
            E::Published(value) => Self::Published {
                generation: value.generation,
                revision: value.revision,
            },
            E::Attached(value) => Self::Attached {
                generation: value.generation,
                revision: value.revision,
            },
            E::Rejected(value) => Self::Rejected {
                generation: value.generation,
            },
            E::Revoked(value) => Self::Revoked {
                generation: value.generation,
                revision: value.revision,
            },
            E::Read(value) => Self::Read {
                generation: value.generation,
                id: value.id,
                format: value.format,
            },
            E::Cancelled(value) => Self::Cancelled {
                generation: value.generation,
                id: value.id,
            },
            E::LocalData(value) => {
                bytes = value.data.to_vec();
                Self::LocalData {
                    id: value.id,
                    revision: value.revision,
                    offset: value.offset,
                    eof: value.eof,
                }
            }
            E::LocalFailed(value) => Self::LocalFailed { id: value.id },
        };
        header.validate(&bytes)?;
        Ok(Frame { header, bytes })
    }
}
#[derive(Clone, Copy)]
enum Shape {
    Empty,
    Board,
    Hello,
    Publish,
    Attach,
    Generation,
    Data,
    Demand,
    ReadLocal,
    Id,
    Ready,
    Snapshot,
    Published,
    Revision,
    Read,
    LocalData,
}
#[derive(Clone, Copy)]
enum Kind {
    Integer,
    Unsigned,
    Boolean,
    Text,
    Formats,
    Payload,
    Message(Shape),
}
fn scan(mut body: &[u8], shape: Shape) -> Result<(), Error> {
    use Kind::*;
    use Shape::*;
    if matches!(shape, Board) {
        let field = fields::single(body)?;
        return match field.number {
            1 => scan(field.bytes()?, Empty),
            2 => {
                field.bytes()?;
                Ok(())
            }
            _ => Err(Invalid("board").into()),
        };
    }
    let known: &[(u32, Kind)] = match shape {
        Empty | Board => &[],
        Hello => &[(1, Unsigned), (2, Message(Board))],
        Publish => &[(1, Integer), (2, Integer), (3, Formats)],
        Attach => &[(1, Integer), (2, Integer), (3, Text)],
        Generation | Id => &[(1, Integer)],
        Data | LocalData => &[
            (1, Integer),
            (2, Integer),
            (3, Integer),
            (4, Boolean),
            (5, Payload),
        ],
        Demand | Published | Revision => &[(1, Integer), (2, Integer)],
        ReadLocal | Read => &[(1, Integer), (2, Integer), (3, Text)],
        Ready => &[(1, Unsigned)],
        Snapshot => &[(1, Integer), (2, Formats), (3, Integer)],
    };
    let mut seen = 0u16;
    let mut formats = 0;
    while !body.is_empty() {
        let field = fields::field(&mut body)?;
        let Some((index, (_, kind))) = known
            .iter()
            .enumerate()
            .find(|(_, (number, _))| *number == field.number)
        else {
            continue;
        };
        if matches!(kind, Formats) {
            formats += 1;
            if formats > MAX_FORMATS {
                return Err(Invalid("format count").into());
            }
        } else {
            let bit = 1u16 << index;
            if seen & bit != 0 {
                return Err(Invalid("duplicate field").into());
            }
            seen |= bit;
        }
        match kind {
            Integer => {
                field.u64()?;
            }
            Unsigned => {
                field.u32()?;
            }
            Boolean => {
                if field.u64()? > 1 {
                    return Err(Invalid("boolean").into());
                }
            }
            Text | Formats => {
                field.bytes()?;
            }
            Payload => {
                if field.bytes()?.len() > CHUNK {
                    return Err(Invalid("payload").into());
                }
            }
            Message(shape) => scan(field.bytes()?, *shape)?,
        }
    }
    Ok(())
}
fn scan_frame(body: &[u8], command: bool) -> Result<(), Error> {
    use Shape::*;
    let field = fields::single(body)?;
    let shape = if command {
        match field.number {
            1 => Hello,
            2 => Publish,
            3 => Attach,
            4 => Generation,
            5 => Data,
            6 => Demand,
            7 => ReadLocal,
            8 => Id,
            9 => Empty,
            _ => return Err(Invalid("command").into()),
        }
    } else {
        match field.number {
            1 => Ready,
            2 => Snapshot,
            3 => Published,
            4 | 6 => Revision,
            5 => Generation,
            7 => Read,
            8 => Demand,
            9 => LocalData,
            10 => Id,
            _ => return Err(Invalid("event").into()),
        }
    };
    scan(field.bytes()?, shape)
}
#[cfg(test)]
mod tests;
