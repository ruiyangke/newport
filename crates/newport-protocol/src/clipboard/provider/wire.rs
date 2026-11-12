//! Generated provider protobuf messages; native ownership stays in adapters.
use super::*;
use crate::encoding::protobuf::{
    self, FrameError, fields, generated::newport::clipboard::provider as pb,
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
fn identity(value: Identity) -> pb::Identity {
    pb::Identity {
        sequence: value.sequence,
        owner: value.owner,
    }
}
fn read_identity(value: Option<pb::Identity>) -> Result<Identity, Error> {
    let value = value.ok_or(Invalid("identity"))?;
    Ok(Identity {
        sequence: value.sequence,
        owner: value.owner,
    })
}
fn stage(value: PublicationStage) -> pb::PublicationStage {
    match value {
        PublicationStage::Source => pb::PublicationStage::Source,
        PublicationStage::Queue => pb::PublicationStage::Queue,
        PublicationStage::Observe => pb::PublicationStage::Observe,
        PublicationStage::Baseline => pb::PublicationStage::Baseline,
        PublicationStage::Formats => pb::PublicationStage::Formats,
        PublicationStage::Generation => pb::PublicationStage::Generation,
        PublicationStage::Open => pb::PublicationStage::Open,
        PublicationStage::Validate => pb::PublicationStage::Validate,
        PublicationStage::Admission => pb::PublicationStage::Admission,
        PublicationStage::Empty => pb::PublicationStage::Empty,
        PublicationStage::DelayedFormats => pb::PublicationStage::DelayedFormats,
        PublicationStage::Concealment => pb::PublicationStage::Concealment,
        PublicationStage::Reply => pb::PublicationStage::Reply,
    }
}
fn read_stage(value: i32) -> Result<PublicationStage, Error> {
    Ok(
        match pb::PublicationStage::try_from(value).map_err(|_| Invalid("publication stage"))? {
            pb::PublicationStage::Source => PublicationStage::Source,
            pb::PublicationStage::Queue => PublicationStage::Queue,
            pb::PublicationStage::Observe => PublicationStage::Observe,
            pb::PublicationStage::Baseline => PublicationStage::Baseline,
            pb::PublicationStage::Formats => PublicationStage::Formats,
            pb::PublicationStage::Generation => PublicationStage::Generation,
            pb::PublicationStage::Open => PublicationStage::Open,
            pb::PublicationStage::Validate => PublicationStage::Validate,
            pb::PublicationStage::Admission => PublicationStage::Admission,
            pb::PublicationStage::Empty => PublicationStage::Empty,
            pb::PublicationStage::DelayedFormats => PublicationStage::DelayedFormats,
            pb::PublicationStage::Concealment => PublicationStage::Concealment,
            pb::PublicationStage::Reply => PublicationStage::Reply,
        },
    )
}
fn read_nonce(value: Option<protobuf::Secret>) -> Result<String, Error> {
    let value = value.ok_or(Invalid("nonce"))?;
    String::from_utf8(value.value.to_vec()).map_err(|_| Invalid("nonce").into())
}
fn path(value: &NativePath) -> Bytes {
    value
        .0
        .iter()
        .flat_map(|unit| unit.to_le_bytes())
        .collect::<Vec<_>>()
        .into()
}
fn read_path(value: Bytes) -> Result<NativePath, Error> {
    let (units, remainder) = value.as_chunks::<2>();
    if !remainder.is_empty() || units.len() > MAX_PATH_UNITS {
        return Err(Invalid("UTF-16 path length").into());
    }
    let value = NativePath(units.iter().copied().map(u16::from_le_bytes).collect());
    value.validate()?;
    Ok(value)
}
fn outcome(value: &Outcome, data: &Bytes) -> pb::Outcome {
    use pb::outcome::Result as R;
    let result = match value {
        Outcome::PublicationRejected(value) => R::PublicationRejected(pb::PublicationRejection {
            stage: stage(value.stage) as i32,
            expected: value.expected,
            observed: value.observed,
            native_error: value.native_error,
        }),
        Outcome::Snapshot {
            revision,
            identity: id,
            formats,
            origin,
        } => R::Snapshot(pb::Snapshot {
            revision: *revision,
            identity: Some(identity(*id)),
            formats: formats.clone(),
            origin: *origin,
        }),
        Outcome::Read {
            revision,
            identity: id,
        } => R::Read(pb::ReadResult {
            revision: *revision,
            identity: Some(identity(*id)),
            data: data.clone(),
        }),
        Outcome::Published {
            generation,
            owner,
            preparation,
        } => R::Published(pb::Published {
            generation: *generation,
            owner: *owner,
            preparation: preparation.map(|(revision, id)| pb::Preparation {
                revision,
                identity: Some(identity(id)),
            }),
        }),
        Outcome::Attached { result } => R::Attached(pb::FileWrite {
            outcome: Some(match result {
                FileWrite::Unchanged => pb::file_write::Outcome::Unchanged(protobuf::Present {}),
                FileWrite::Committed { revision, complete } => {
                    pb::file_write::Outcome::Committed(pb::Committed {
                        revision: *revision,
                        complete: *complete,
                    })
                }
                FileWrite::Uncertain => pb::file_write::Outcome::Uncertain(protobuf::Present {}),
            }),
        }),
    };
    pb::Outcome {
        result: Some(result),
    }
}
fn read_outcome(value: pb::Outcome, data: &mut Vec<u8>) -> Result<Outcome, Error> {
    use pb::outcome::Result as R;
    Ok(match value.result.ok_or(Invalid("outcome"))? {
        R::PublicationRejected(value) => Outcome::PublicationRejected(PublicationRejection {
            stage: read_stage(value.stage)?,
            expected: value.expected,
            observed: value.observed,
            native_error: value.native_error,
        }),
        R::Snapshot(value) => Outcome::Snapshot {
            revision: value.revision,
            identity: read_identity(value.identity)?,
            formats: value.formats,
            origin: value.origin,
        },
        R::Read(value) => {
            *data = value.data.to_vec();
            Outcome::Read {
                revision: value.revision,
                identity: read_identity(value.identity)?,
            }
        }
        R::Published(value) => Outcome::Published {
            generation: value.generation,
            owner: value.owner,
            preparation: value
                .preparation
                .map(|value| Ok::<_, Error>((value.revision, read_identity(value.identity)?)))
                .transpose()?,
        },
        R::Attached(value) => Outcome::Attached {
            result: match value.outcome.ok_or(Invalid("file receipt"))? {
                pb::file_write::Outcome::Unchanged(_) => FileWrite::Unchanged,
                pb::file_write::Outcome::Committed(value) => FileWrite::Committed {
                    revision: value.revision,
                    complete: value.complete,
                },
                pb::file_write::Outcome::Uncertain(_) => FileWrite::Uncertain,
            },
        },
    })
}
pub fn encode(packet: Packet) -> Result<Vec<u8>, Error> {
    use pb::frame::Message as M;
    packet.validate()?;
    let Packet { message, data } = packet;
    let data = Bytes::from(data);
    let message = match &message {
        Message::Hello { version, nonce } => M::Hello(pb::Greeting {
            version: *version,
            nonce: Some(protobuf::Secret {
                value: Bytes::copy_from_slice(nonce.as_bytes()),
            }),
        }),
        Message::Ready { version, nonce } => M::Ready(pb::Greeting {
            version: *version,
            nonce: Some(protobuf::Secret {
                value: Bytes::copy_from_slice(nonce.as_bytes()),
            }),
        }),
        Message::Snapshot { id } => M::Snapshot(pb::Id { id: *id }),
        Message::Read {
            id,
            revision,
            format,
        } => M::Read(pb::Read {
            id: *id,
            revision: *revision,
            format: format.clone(),
        }),
        Message::Publish {
            id,
            expected,
            formats,
        } => M::Publish(pb::Publish {
            id: *id,
            expected: *expected,
            formats: formats.clone(),
        }),
        Message::Attach {
            id,
            generation,
            folder,
        } => M::Attach(pb::Attach {
            id: *id,
            generation: *generation,
            folder_utf16le: path(folder),
        }),
        Message::Done { id, outcome: value } => M::Done(pb::Done {
            id: *id,
            outcome: value.as_ref().map(|value| outcome(value, &data)),
        }),
        Message::Admit {
            id,
            source,
            preparation,
        } => M::Admit(pb::Admit {
            id: *id,
            source: *source,
            preparation: *preparation,
        }),
        Message::Admitted { id, accepted } => M::Admitted(pb::Admitted {
            id: *id,
            accepted: *accepted,
        }),
        Message::Demand { id, source, format } => M::Demand(pb::Demand {
            id: *id,
            source: *source,
            format: format.clone(),
        }),
        Message::Data { id, present } => M::Data(pb::Data {
            id: *id,
            present: *present,
            data: data.clone(),
        }),
        Message::Consumed { id } => M::Consumed(pb::Id { id: *id }),
        Message::Revoke { source } => M::Revoke(pb::Source { source: *source }),
        Message::Revoked { source } => M::Revoked(pb::Source { source: *source }),
        Message::Stop => M::Stop(protobuf::Present {}),
    };
    let mut bytes = Vec::new();
    protobuf::write_message(
        &mut bytes,
        &pb::Frame {
            message: Some(message),
        },
        &LIMITS,
    )?;
    Ok(bytes)
}
pub fn decode(body: Bytes) -> Result<Packet, Error> {
    use pb::frame::Message as M;
    if body.len() > LIMITS.message {
        return Err(FrameError::TooLarge {
            length: body.len() as u64,
            limit: LIMITS.message,
        }
        .into());
    }
    scan_frame(&body)?;
    let value: pb::Frame = protobuf::decode(body, &LIMITS)?;
    let mut data = Vec::new();
    let message = match value.message.ok_or(Invalid("message"))? {
        M::Hello(value) => Message::Hello {
            version: value.version,
            nonce: read_nonce(value.nonce)?,
        },
        M::Ready(value) => Message::Ready {
            version: value.version,
            nonce: read_nonce(value.nonce)?,
        },
        M::Snapshot(value) => Message::Snapshot { id: value.id },
        M::Read(value) => Message::Read {
            id: value.id,
            revision: value.revision,
            format: value.format,
        },
        M::Publish(value) => Message::Publish {
            id: value.id,
            expected: value.expected,
            formats: value.formats,
        },
        M::Attach(value) => Message::Attach {
            id: value.id,
            generation: value.generation,
            folder: read_path(value.folder_utf16le)?,
        },
        M::Done(value) => Message::Done {
            id: value.id,
            outcome: value
                .outcome
                .map(|value| read_outcome(value, &mut data))
                .transpose()?,
        },
        M::Admit(value) => Message::Admit {
            id: value.id,
            source: value.source,
            preparation: value.preparation,
        },
        M::Admitted(value) => Message::Admitted {
            id: value.id,
            accepted: value.accepted,
        },
        M::Demand(value) => Message::Demand {
            id: value.id,
            source: value.source,
            format: value.format,
        },
        M::Data(value) => {
            data = value.data.to_vec();
            Message::Data {
                id: value.id,
                present: value.present,
            }
        }
        M::Consumed(value) => Message::Consumed { id: value.id },
        M::Revoke(value) => Message::Revoke {
            source: value.source,
        },
        M::Revoked(value) => Message::Revoked {
            source: value.source,
        },
        M::Stop(_) => Message::Stop,
    };
    let packet = Packet { message, data };
    packet.validate()?;
    Ok(packet)
}
#[derive(Clone, Copy)]
enum Shape {
    Empty,
    Secret,
    Greeting,
    Identity,
    Rejection,
    Preparation,
    Committed,
    FileWrite,
    Snapshot,
    ReadResult,
    Published,
    Outcome,
    Id,
    Read,
    Publish,
    Attach,
    Done,
    Admit,
    Admitted,
    Demand,
    Data,
    Source,
}
#[derive(Clone, Copy)]
enum Kind {
    Integer,
    Unsigned,
    Boolean,
    Text,
    Nonce,
    Formats,
    Payload,
    Path,
    Message(Shape),
}
fn scan(mut body: &[u8], shape: Shape) -> Result<(), Error> {
    use Kind::*;
    use Shape::*;
    if matches!(shape, Outcome | FileWrite) {
        let field = fields::single(body)?;
        let inner = match (shape, field.number) {
            (Outcome, 1) => Rejection,
            (Outcome, 2) => Snapshot,
            (Outcome, 3) => ReadResult,
            (Outcome, 4) => Published,
            (Outcome, 5) => FileWrite,
            (FileWrite, 1 | 3) => Empty,
            (FileWrite, 2) => Committed,
            _ => return Err(Invalid("outcome").into()),
        };
        return scan(field.bytes()?, inner);
    }
    let known: &[(u32, Kind)] = match shape {
        Empty | Outcome | FileWrite => &[],
        Secret => &[(1, Nonce)],
        Greeting => &[(1, Unsigned), (2, Message(Secret))],
        Identity => &[(1, Unsigned), (2, Integer)],
        Rejection => &[(1, Unsigned), (2, Integer), (3, Integer), (4, Unsigned)],
        Preparation => &[(1, Integer), (2, Message(Identity))],
        Committed => &[(1, Integer), (2, Boolean)],
        Snapshot => &[
            (1, Integer),
            (2, Message(Identity)),
            (3, Formats),
            (4, Integer),
        ],
        ReadResult => &[(1, Integer), (2, Message(Identity)), (3, Payload)],
        Published => &[(1, Integer), (2, Integer), (3, Message(Preparation))],
        Id | Source => &[(1, Integer)],
        Read => &[(1, Integer), (2, Integer), (3, Text)],
        Publish => &[(1, Integer), (2, Integer), (3, Formats)],
        Attach => &[(1, Integer), (2, Integer), (3, Path)],
        Done => &[(1, Integer), (2, Message(Outcome))],
        Admit => &[(1, Integer), (2, Integer), (3, Boolean)],
        Admitted => &[(1, Integer), (2, Boolean)],
        Demand => &[(1, Integer), (2, Integer), (3, Text)],
        Data => &[(1, Integer), (2, Boolean), (3, Payload)],
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
            Nonce => {
                if field.bytes()?.len() > 64 {
                    return Err(Invalid("nonce").into());
                }
            }
            Payload => {
                if field.bytes()?.len() > MAX_BYTES {
                    return Err(Invalid("payload").into());
                }
            }
            Path => {
                let bytes = field.bytes()?;
                if bytes.len() % 2 != 0 || bytes.len() > MAX_PATH_UNITS * 2 {
                    return Err(Invalid("UTF-16 path length").into());
                }
            }
            Message(shape) => scan(field.bytes()?, *shape)?,
        }
    }
    Ok(())
}
fn scan_frame(body: &[u8]) -> Result<(), Error> {
    use Shape::*;
    let field = fields::single(body)?;
    let shape = match field.number {
        1 | 2 => Greeting,
        3 | 12 => Id,
        4 => Read,
        5 => Publish,
        6 => Attach,
        7 => Done,
        8 => Admit,
        9 => Admitted,
        10 => Demand,
        11 => Data,
        13 | 14 => Source,
        15 => Empty,
        _ => return Err(Invalid("message").into()),
    };
    scan(field.bytes()?, shape)
}
#[cfg(test)]
mod tests;
