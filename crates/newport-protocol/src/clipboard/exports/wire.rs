//! Bounded protobuf bodies carried by the integration service's frames.
use super::*;
use crate::encoding::protobuf::{self, FrameError, generated::newport::clipboard as pb};
use bytes::Bytes;
use prost::Message;

pub(crate) mod scan;
#[cfg(test)]
mod tests;

const MAX_FIELDS: usize = 1024;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Frame(#[from] FrameError),
    #[error(transparent)]
    Invalid(#[from] Invalid),
}
impl From<Error> for std::io::Error {
    fn from(error: Error) -> Self {
        Self::new(std::io::ErrorKind::InvalidData, error)
    }
}

fn encode(value: &impl Message) -> Result<Vec<u8>, Error> {
    let length = value.encoded_len();
    if length > MAX_FRAME {
        return Err(FrameError::TooLarge {
            length: length as u64,
            limit: MAX_FRAME,
        }
        .into());
    }
    Ok(value.encode_to_vec())
}

pub fn encode_control(control: &Control) -> Result<Vec<u8>, Error> {
    control.validate()?;
    encode(&control_to_wire(control))
}
pub fn decode_control(body: &[u8]) -> Result<Control, Error> {
    scan::control(body)?;
    let value = control_from_wire(protobuf::decode(Bytes::copy_from_slice(body), &LIMITS)?)?;
    value.validate()?;
    Ok(value)
}
pub fn encode_event(event: &Event) -> Result<Vec<u8>, Error> {
    event.validate()?;
    encode(&event_to_wire(event))
}
pub fn decode_event(body: &[u8]) -> Result<Event, Error> {
    scan::event(body)?;
    let value = event_from_wire(protobuf::decode(Bytes::copy_from_slice(body), &LIMITS)?)?;
    value.validate()?;
    Ok(value)
}

pub(crate) fn control_to_wire(value: &Control) -> pb::ExportControl {
    use pb::export_control::Command;
    let command = match value {
        Control::Enable { version } => Command::Enable(pb::Enable { version: *version }),
        Control::Disable => Command::Disable(protobuf::Present {}),
        Control::Read { id, offer, format } => Command::Read(pb::Read {
            id: *id,
            offer: Some(offer_to_wire(offer)),
            format: format.clone(),
        }),
        Control::Cancel { id } => Command::Cancel(pb::Cancel { id: *id }),
        Control::Commit { id, offer } => Command::Commit(pb::Commit {
            id: *id,
            offer: Some(offer_to_wire(offer)),
        }),
        Control::Release { copy } => Command::Release(pb::Release {
            copy: Some(copy_to_wire(copy)),
        }),
        Control::ReadCopy { id, copy, format } => Command::ReadCopy(pb::ReadCopy {
            id: *id,
            copy: Some(copy_to_wire(copy)),
            format: format.clone(),
        }),
    };
    pb::ExportControl {
        command: Some(command),
    }
}
pub(crate) fn control_from_wire(value: pb::ExportControl) -> Result<Control, Invalid> {
    use pb::export_control::Command;
    Ok(match value.command.ok_or(Invalid)? {
        Command::Enable(value) => Control::Enable {
            version: value.version,
        },
        Command::Disable(_) => Control::Disable,
        Command::Read(value) => Control::Read {
            id: value.id,
            offer: offer_from_wire(value.offer.ok_or(Invalid)?)?,
            format: value.format,
        },
        Command::Cancel(value) => Control::Cancel { id: value.id },
        Command::Commit(value) => Control::Commit {
            id: value.id,
            offer: offer_from_wire(value.offer.ok_or(Invalid)?)?,
        },
        Command::Release(value) => Control::Release {
            copy: copy_from_wire(value.copy.ok_or(Invalid)?)?,
        },
        Command::ReadCopy(value) => Control::ReadCopy {
            id: value.id,
            copy: copy_from_wire(value.copy.ok_or(Invalid)?)?,
            format: value.format,
        },
    })
}
pub(crate) fn event_to_wire(value: &Event) -> pb::ExportEvent {
    use pb::export_event::Event as Message;
    let event = match value {
        Event::Offer { offer } => Message::Offered(pb::Offered {
            offer: offer.as_ref().map(offer_to_wire),
        }),
        Event::Data { id, offset, chunk } => Message::Data(pb::Data {
            id: *id,
            offset: *offset as u64,
            chunk: chunk.as_ref().map(|value| pb::Chunk {
                data: value.bytes.clone().into(),
                eof: value.eof,
            }),
        }),
        Event::Committed { id, copy } => Message::Committed(pb::Committed {
            id: *id,
            copy: copy.as_ref().map(copy_to_wire),
        }),
        Event::CopyRevoked { id } => Message::CopyRevoked(pb::CopyRevoked { id: id.clone() }),
    };
    pb::ExportEvent { event: Some(event) }
}
pub(crate) fn event_from_wire(value: pb::ExportEvent) -> Result<Event, Invalid> {
    use pb::export_event::Event as Message;
    Ok(match value.event.ok_or(Invalid)? {
        Message::Offered(value) => Event::Offer {
            offer: value.offer.map(offer_from_wire).transpose()?,
        },
        Message::Data(value) => Event::Data {
            id: value.id,
            offset: usize::try_from(value.offset).map_err(|_| Invalid)?,
            chunk: value.chunk.map(|value| Chunk {
                bytes: value.data.to_vec(),
                eof: value.eof,
            }),
        },
        Message::Committed(value) => Event::Committed {
            id: value.id,
            copy: value.copy.map(copy_from_wire).transpose()?,
        },
        Message::CopyRevoked(value) => Event::CopyRevoked { id: value.id },
    })
}
pub(crate) fn offer_to_wire(value: &ExportOffer) -> pb::ExportOffer {
    let origin = match &value.offer.origin {
        Origin::Native { worker, revision } => Some(pb::NativeOrigin {
            worker: worker.clone(),
            revision: *revision,
        }),
        _ => None,
    };
    pb::ExportOffer {
        runtime: value.offer.runtime.clone(),
        revision: value.offer.revision,
        origin,
        formats: value.offer.formats.clone(),
        route_revision: value.route_revision,
    }
}
pub(crate) fn offer_from_wire(value: pb::ExportOffer) -> Result<ExportOffer, Invalid> {
    let origin = value.origin.ok_or(Invalid)?;
    Ok(ExportOffer {
        offer: super::super::Offer {
            runtime: value.runtime,
            revision: value.revision,
            origin: Origin::Native {
                worker: origin.worker,
                revision: origin.revision,
            },
            formats: value.formats,
        },
        route_revision: value.route_revision,
    })
}
pub(crate) fn copy_to_wire(value: &CopyLease) -> pb::CopyLease {
    pb::CopyLease {
        id: value.id.clone(),
        offer: Some(offer_to_wire(&value.offer)),
    }
}
pub(crate) fn copy_from_wire(value: pb::CopyLease) -> Result<CopyLease, Invalid> {
    Ok(CopyLease {
        id: value.id,
        offer: offer_from_wire(value.offer.ok_or(Invalid)?)?,
    })
}
