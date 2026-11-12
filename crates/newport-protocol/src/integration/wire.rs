//! Directional integration messages on bounded protobuf frames.
use super::*;
use crate::{
    encoding::protobuf::{self, FrameError, generated::newport::integration as pb},
    runtime::wire::convert as runtime,
};
use exports::wire as export;
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
    #[error(transparent)]
    Export(#[from] export::Error),
    #[error(transparent)]
    Runtime(#[from] crate::runtime::wire::Error),
    #[error(transparent)]
    Control(#[from] crate::worker::wire::Error),
    #[error("integration stream ended")]
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
    const LIMITS: common::Limits;
    fn encode(&self) -> Result<Vec<u8>, Error>;
    fn decode(body: Bytes) -> Result<Self, Error>;
}
pub fn read<T: Message>(input: &mut impl Read) -> Result<T, Error> {
    read_limited(input, T::LIMITS.message)
}
pub fn read_limited<T: Message>(input: &mut impl Read, limit: usize) -> Result<T, Error> {
    T::decode(protobuf::read_frame(input, limit.min(T::LIMITS.message))?.ok_or(Error::Ended)?)
}
pub fn write<T: Message>(output: &mut impl Write, value: &T) -> Result<(), Error> {
    output
        .write_all(&value.encode()?)
        .map_err(FrameError::from)?;
    output.flush().map_err(FrameError::from)?;
    Ok(())
}
fn encode(
    value: &(impl prost::Message + protobuf::Layout),
    limits: &common::Limits,
) -> Result<Vec<u8>, Error> {
    let mut bytes = vec![];
    protobuf::write_message(&mut bytes, value, limits)?;
    Ok(bytes)
}
impl sealed::Sealed for ClientMessage {}
impl Message for ClientMessage {
    const LIMITS: common::Limits = CLIENT_LIMITS;
    fn encode(&self) -> Result<Vec<u8>, Error> {
        use pb::client_frame::Message;
        self.validate()?;
        let message = match self {
            Self::Call(value) => Message::Call(call_to_wire(value)),
            Self::Clipboard(value) => Message::Clipboard(pb::ClipboardReply {
                id: value.id,
                revision: value.revision,
                encoding: match value.encoding {
                    imports::Encoding::Raw => 0,
                    imports::Encoding::Failed => 1,
                    imports::Encoding::Zlib => 2,
                },
                done: value.done,
                data: value.bytes.clone().into(),
            }),
            Self::Browser(value) => Message::Browser(pb::BrowserReply {
                id: value.id,
                success: value.success,
                message: value.message.clone(),
            }),
            Self::Quit => Message::Quit(protobuf::Present {}),
        };
        encode(
            &pb::ClientFrame {
                message: Some(message),
            },
            &CLIENT_LIMITS,
        )
    }
    fn decode(body: Bytes) -> Result<Self, Error> {
        use pb::client_frame::Message;
        crate::worker::wire::reject_controls(&body, CLIENT_LIMITS.message)?;
        scan::client(&body)?;
        let frame: pb::ClientFrame = protobuf::decode(body, &CLIENT_LIMITS)?;
        let message = match frame.message.ok_or(Invalid("client operation"))? {
            Message::Call(value) => Self::Call(call_from_wire(value)?),
            Message::Clipboard(value) => Self::Clipboard(imports::Reply {
                id: value.id,
                revision: value.revision,
                encoding: match value.encoding {
                    0 => imports::Encoding::Raw,
                    1 => imports::Encoding::Failed,
                    2 => imports::Encoding::Zlib,
                    _ => return Err(Invalid("clipboard encoding").into()),
                },
                done: value.done,
                bytes: value.data.to_vec(),
            }),
            Message::Browser(value) => Self::Browser(BrowserReply {
                id: value.id,
                success: value.success,
                message: value.message,
            }),
            Message::Quit(_) => Self::Quit,
            Message::WorkerControl(_) => return Err(Invalid("worker control").into()),
        };
        message.validate()?;
        Ok(message)
    }
}
impl sealed::Sealed for ServerMessage {}
impl Message for ServerMessage {
    const LIMITS: common::Limits = SERVER_LIMITS;
    fn encode(&self) -> Result<Vec<u8>, Error> {
        use pb::server_frame::Message;
        self.validate()?;
        let message = match self {
            Self::Response(value) => Message::Response(response_to_wire(value)?),
            Self::ClipboardRequest(value) => Message::ClipboardRequest(pb::ClipboardRequest {
                id: value.id,
                revision: value.revision,
                format: value.format.clone(),
            }),
            Self::Browser(value) => Message::Browser(pb::BrowserRequest {
                id: value.id,
                url: value.url.clone(),
            }),
            Self::GuiNotice => Message::GuiNotice(protobuf::Present {}),
            Self::Export(value) => Message::Export(export::event_to_wire(value)),
            Self::Failed(value) => Message::Error(runtime::error(value)),
        };
        encode(
            &pb::ServerFrame {
                message: Some(message),
            },
            &SERVER_LIMITS,
        )
    }
    fn decode(body: Bytes) -> Result<Self, Error> {
        use pb::server_frame::Message;
        crate::worker::wire::reject_controls(&body, SERVER_LIMITS.message)?;
        scan::server(&body)?;
        let frame: pb::ServerFrame = protobuf::decode(body, &SERVER_LIMITS)?;
        let message = match frame.message.ok_or(Invalid("server operation"))? {
            Message::Response(value) => Self::Response(response_from_wire(value)?),
            Message::ClipboardRequest(value) => Self::ClipboardRequest(imports::Request {
                id: value.id,
                revision: value.revision,
                format: value.format,
            }),
            Message::Browser(value) => Self::Browser(BrowserRequest {
                id: value.id,
                url: value.url,
            }),
            Message::GuiNotice(_) => Self::GuiNotice,
            Message::Export(value) => {
                Self::Export(export::event_from_wire(value).map_err(export::Error::from)?)
            }
            Message::Error(value) => Self::Failed(runtime::from_error(value)?),
            Message::WorkerControl(_) => return Err(Invalid("worker control").into()),
        };
        message.validate()?;
        Ok(message)
    }
}
fn call_to_wire(value: &Call) -> pb::Call {
    use pb::call::Request as Message;
    let request = match &value.request {
        Request::Permissions(value) => Message::Permissions(runtime::permission_values(value)),
        Request::ClipboardOffer(value) => Message::ClipboardOffer(pb::ClipboardOffer {
            revision: value.revision,
            formats: value.formats.clone(),
        }),
        Request::Snapshot(value) => Message::Snapshot(pb::Snapshot {
            archive: value.clone(),
        }),
        Request::GuiNotices => Message::GuiNotices(protobuf::Present {}),
        Request::Heartbeat => Message::Heartbeat(protobuf::Present {}),
        Request::Export(value) => Message::Export(export::control_to_wire(value)),
    };
    pb::Call {
        id: value.id,
        request: Some(request),
    }
}
fn call_from_wire(value: pb::Call) -> Result<Call, Error> {
    use pb::call::Request as Message;
    let request = match value.request.ok_or(Invalid("request operation"))? {
        Message::Permissions(value) => {
            Request::Permissions(runtime::from_permission_values(Some(value))?)
        }
        Message::ClipboardOffer(value) => Request::ClipboardOffer(imports::Offer {
            revision: value.revision,
            formats: value.formats,
        }),
        Message::Snapshot(value) => Request::Snapshot(value.archive),
        Message::GuiNotices(_) => Request::GuiNotices,
        Message::Heartbeat(_) => Request::Heartbeat,
        Message::Export(value) => {
            Request::Export(export::control_from_wire(value).map_err(export::Error::from)?)
        }
    };
    Ok(Call {
        id: value.id,
        request,
    })
}
fn response_to_wire(value: &Response) -> Result<pb::Response, Error> {
    use pb::response::Reply as Message;
    let reply = match &value.reply {
        Reply::Acknowledged { message } => Message::Acknowledged(pb::Acknowledged {
            message: message.clone(),
        }),
        Reply::Permissions(value) => Message::Permissions(runtime::applied(value)?),
        Reply::Failed(value) => Message::Error(runtime::error(value)),
    };
    Ok(pb::Response {
        id: value.id,
        reply: Some(reply),
    })
}
fn response_from_wire(value: pb::Response) -> Result<Response, Error> {
    use pb::response::Reply as Message;
    let reply = match value.reply.ok_or(Invalid("response operation"))? {
        Message::Acknowledged(value) => Reply::Acknowledged {
            message: value.message,
        },
        Message::Permissions(value) => Reply::Permissions(runtime::from_applied(value)?),
        Message::Error(value) => Reply::Failed(runtime::from_error(value)?),
    };
    Ok(Response {
        id: value.id,
        reply,
    })
}
