//! Generated protobuf frames for authenticated runtime clipboard IPC.
use super::super::exports::wire::{
    copy_from_wire, copy_to_wire, offer_from_wire as export_from_wire,
    offer_to_wire as export_to_wire,
};
use super::*;
use crate::encoding::protobuf::{
    self, FrameError, fields,
    generated::newport::{
        clipboard as shared, clipboard::service as pb, integration as integration_pb,
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
    #[error("clipboard service ended the stream")]
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
    T::decode(protobuf::read_frame(input, LIMITS.message)?.ok_or(Error::Ended)?)
}
pub fn write<T: Message>(output: &mut impl Write, value: &T) -> Result<(), Error> {
    output
        .write_all(&value.encode()?)
        .map_err(FrameError::from)?;
    output.flush().map_err(FrameError::from)?;
    Ok(())
}
fn required<T>(value: Option<T>, name: &'static str) -> Result<T, Error> {
    value.ok_or_else(|| Invalid(name).into())
}
fn usize_from(value: u64) -> Result<usize, Error> {
    usize::try_from(value).map_err(|_| Invalid("range").into())
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
pub(crate) fn access_to_wire(value: &Access) -> Result<shared::Access, Error> {
    access(value)?;
    Ok(shared::Access {
        path: value.path.to_str().ok_or(Invalid("access path"))?.into(),
        runtime: value.runtime.clone(),
        worker: value.worker.clone(),
        token: Some(protobuf::Secret {
            value: Bytes::copy_from_slice(value.token.as_bytes()),
        }),
    })
}
pub(crate) fn access_from_wire(value: shared::Access) -> Result<Access, Error> {
    let access = Access {
        path: value.path.into(),
        runtime: value.runtime,
        worker: value.worker,
        token: String::from_utf8(required(value.token, "access token")?.value.to_vec())
            .map_err(|_| Invalid("access token"))?,
    };
    super::access(&access)?;
    Ok(access)
}
fn offer_to_wire(value: &Offer) -> pb::Offer {
    let origin = match &value.origin {
        Origin::Empty => pb::offer::Origin::Empty(protobuf::Present {}),
        Origin::Host => pb::offer::Origin::Host(protobuf::Present {}),
        Origin::Native { worker, revision } => pb::offer::Origin::Native(shared::NativeOrigin {
            worker: worker.clone(),
            revision: *revision,
        }),
    };
    pb::Offer {
        runtime: value.runtime.clone(),
        revision: value.revision,
        origin: Some(origin),
        formats: value.formats.clone(),
    }
}
fn offer_from_wire(value: pb::Offer) -> Result<Offer, Error> {
    let origin = match required(value.origin, "offer origin")? {
        pb::offer::Origin::Empty(_) => Origin::Empty,
        pb::offer::Origin::Host(_) => Origin::Host,
        pb::offer::Origin::Native(value) => Origin::Native {
            worker: value.worker,
            revision: value.revision,
        },
    };
    Ok(Offer {
        runtime: value.runtime,
        revision: value.revision,
        origin,
        formats: value.formats,
    })
}
fn lease_to_wire(value: &ProviderLease) -> pb::ProviderLease {
    pb::ProviderLease {
        runtime: value.runtime.clone(),
        worker: value.worker.clone(),
        epoch: value.epoch,
    }
}
fn lease_from_wire(value: pb::ProviderLease) -> ProviderLease {
    ProviderLease {
        runtime: value.runtime,
        worker: value.worker,
        epoch: value.epoch,
    }
}
fn native_to_wire(value: &NativeRequest) -> pb::NativeRequest {
    pb::NativeRequest {
        lease: Some(lease_to_wire(&value.lease)),
        id: value.id,
        revision: value.revision,
        format: value.format.clone(),
    }
}
fn native_from_wire(value: pb::NativeRequest) -> Result<NativeRequest, Error> {
    Ok(NativeRequest {
        lease: lease_from_wire(required(value.lease, "native lease")?),
        id: value.id,
        revision: value.revision,
        format: value.format,
    })
}
fn chunk_to_wire(value: &Chunk) -> shared::Chunk {
    shared::Chunk {
        data: Bytes::copy_from_slice(&value.bytes),
        eof: value.eof,
    }
}
fn chunk_from_wire(value: shared::Chunk) -> Chunk {
    Chunk {
        bytes: value.data.to_vec(),
        eof: value.eof,
    }
}
fn export_decode(value: shared::ExportOffer) -> Result<ExportOffer, Error> {
    export_from_wire(value).map_err(|_| Invalid("export offer").into())
}
fn copy_decode(value: shared::CopyLease) -> Result<CopyLease, Error> {
    copy_from_wire(value).map_err(|_| Invalid("copy lease").into())
}
impl sealed::Sealed for Request {}
impl Message for Request {
    fn encode(&self) -> Result<Vec<u8>, Error> {
        use pb::client_frame::Request as R;
        self.validate()?;
        let request = match self {
            Self::Authenticate(value) => R::Authenticate(access_to_wire(value)?),
            Self::Offer => R::Offer(protobuf::Present {}),
            Self::Read {
                runtime,
                revision,
                format,
                offset,
                limit,
                export,
            } => R::Read(pb::Read {
                runtime: runtime.clone(),
                revision: *revision,
                format: format.clone(),
                offset: u64::try_from(*offset).map_err(|_| Invalid("offset"))?,
                limit: u32::try_from(*limit).map_err(|_| Invalid("limit"))?,
                export: export.as_ref().map(|value| match value {
                    ExportRead::Focused(value) => pb::read::Export::Focused(export_to_wire(value)),
                    ExportRead::Committed(value) => {
                        pb::read::Export::Committed(copy_to_wire(value))
                    }
                }),
            }),
            Self::Claim => R::Claim(protobuf::Present {}),
            Self::Enabled(value) => R::Enabled(pb::Enabled { value: *value }),
            Self::Publish { revision, formats } => R::Publish(integration_pb::ClipboardOffer {
                revision: *revision,
                formats: formats.clone(),
            }),
            Self::Poll => R::Poll(protobuf::Present {}),
            Self::Reply(value) => R::Reply(integration_pb::ClipboardReply {
                id: value.id,
                revision: value.revision,
                encoding: match value.encoding {
                    imports::Encoding::Raw => 0,
                    imports::Encoding::Failed => 1,
                    imports::Encoding::Zlib => 2,
                },
                done: value.done,
                data: Bytes::copy_from_slice(&value.bytes),
            }),
            Self::NativePoll => R::NativePoll(protobuf::Present {}),
            Self::NativePublish {
                lease,
                expected_runtime,
                expected_revision,
                revision,
                formats,
            } => R::NativePublish(pb::NativePublish {
                lease: Some(lease_to_wire(lease)),
                expected_runtime: expected_runtime.clone(),
                expected_revision: *expected_revision,
                revision: *revision,
                formats: formats.clone(),
            }),
            Self::NativeReply { request, chunk } => R::NativeReply(pb::NativeReply {
                request: Some(native_to_wire(request)),
                chunk: chunk.as_ref().map(chunk_to_wire),
            }),
            Self::ExportOffer => R::ExportOffer(protobuf::Present {}),
            Self::CommitCopy(value) => R::CommitCopy(export_to_wire(value)),
            Self::CopyStatus => R::CopyStatus(protobuf::Present {}),
            Self::ReleaseCopy(value) => R::ReleaseCopy(copy_to_wire(value)),
        };
        let mut bytes = Vec::new();
        protobuf::write_message(
            &mut bytes,
            &pb::ClientFrame {
                request: Some(request),
            },
            &LIMITS,
        )?;
        Ok(bytes)
    }
    fn decode(body: Bytes) -> Result<Self, Error> {
        use pb::client_frame::Request as R;
        bounded(&body)?;
        scan::request(&body)?;
        let value: pb::ClientFrame = protobuf::decode(body, &LIMITS)?;
        let request = match required(value.request, "request operation")? {
            R::Authenticate(value) => Self::Authenticate(access_from_wire(value)?),
            R::Offer(_) => Self::Offer,
            R::Read(value) => Self::Read {
                runtime: value.runtime,
                revision: value.revision,
                format: value.format,
                offset: usize_from(value.offset)?,
                limit: usize_from(u64::from(value.limit))?,
                export: value
                    .export
                    .map(|value| -> Result<_, Error> {
                        Ok(match value {
                            pb::read::Export::Focused(value) => {
                                ExportRead::Focused(export_decode(value)?)
                            }
                            pb::read::Export::Committed(value) => {
                                ExportRead::Committed(copy_decode(value)?)
                            }
                        })
                    })
                    .transpose()?,
            },
            R::Claim(_) => Self::Claim,
            R::Enabled(value) => Self::Enabled(value.value),
            R::Publish(value) => Self::Publish {
                revision: value.revision,
                formats: value.formats,
            },
            R::Poll(_) => Self::Poll,
            R::Reply(value) => Self::Reply(imports::Reply {
                id: value.id,
                revision: value.revision,
                encoding: match value.encoding {
                    0 => imports::Encoding::Raw,
                    1 => imports::Encoding::Failed,
                    2 => imports::Encoding::Zlib,
                    _ => return Err(Invalid("reply encoding").into()),
                },
                done: value.done,
                bytes: value.data.to_vec(),
            }),
            R::NativePoll(_) => Self::NativePoll,
            R::NativePublish(value) => Self::NativePublish {
                lease: lease_from_wire(required(value.lease, "provider lease")?),
                expected_runtime: value.expected_runtime,
                expected_revision: value.expected_revision,
                revision: value.revision,
                formats: value.formats,
            },
            R::NativeReply(value) => Self::NativeReply {
                request: native_from_wire(required(value.request, "native request")?)?,
                chunk: value.chunk.map(chunk_from_wire),
            },
            R::ExportOffer(_) => Self::ExportOffer,
            R::CommitCopy(value) => Self::CommitCopy(export_decode(value)?),
            R::CopyStatus(_) => Self::CopyStatus,
            R::ReleaseCopy(value) => Self::ReleaseCopy(copy_decode(value)?),
        };
        request.validate()?;
        Ok(request)
    }
}
impl sealed::Sealed for Response {}
impl Message for Response {
    fn encode(&self) -> Result<Vec<u8>, Error> {
        use pb::server_frame::Response as R;
        self.validate()?;
        let response = match self {
            Self::Hello(value) => R::Hello(pb::Hello {
                version: value.version,
                runtime: value.runtime.clone(),
            }),
            Self::Ready => R::Ready(protobuf::Present {}),
            Self::Offer(value) => R::Offer(offer_to_wire(value)),
            Self::Chunk(value) => R::Chunk(chunk_to_wire(value)),
            Self::Requests(values) => R::Requests(pb::Requests {
                requests: values
                    .iter()
                    .map(|value| integration_pb::ClipboardRequest {
                        id: value.id,
                        revision: value.revision,
                        format: value.format.clone(),
                    })
                    .collect(),
            }),
            Self::Provider(value) => R::Provider(pb::ProviderPoll {
                lease: value.lease.as_ref().map(lease_to_wire),
                offer: Some(offer_to_wire(&value.offer)),
                requests: value.requests.iter().map(native_to_wire).collect(),
            }),
            Self::Published(accepted) => R::Published(pb::Published {
                accepted: *accepted,
            }),
            Self::Export(value) => R::Export(pb::Export {
                offer: value.as_ref().map(export_to_wire),
            }),
            Self::Copy(value) => R::Copy(pb::Copy {
                copy: value.as_ref().map(copy_to_wire),
            }),
            Self::Error(value) => R::Error(protobuf::Error {
                code: value.code.clone(),
                message: value.message.clone(),
                retry: match value.retry {
                    common::Retry::Unknown => 0,
                    common::Retry::Yes => 1,
                    common::Retry::No => 2,
                },
            }),
        };
        let mut bytes = Vec::new();
        protobuf::write_message(
            &mut bytes,
            &pb::ServerFrame {
                response: Some(response),
            },
            &LIMITS,
        )?;
        Ok(bytes)
    }
    fn decode(body: Bytes) -> Result<Self, Error> {
        use pb::server_frame::Response as R;
        bounded(&body)?;
        scan::response(&body)?;
        let value: pb::ServerFrame = protobuf::decode(body, &LIMITS)?;
        let response = match required(value.response, "response operation")? {
            R::Hello(value) => Self::Hello(Hello {
                version: value.version,
                runtime: value.runtime,
            }),
            R::Ready(_) => Self::Ready,
            R::Offer(value) => Self::Offer(offer_from_wire(value)?),
            R::Chunk(value) => Self::Chunk(chunk_from_wire(value)),
            R::Requests(value) => Self::Requests(
                value
                    .requests
                    .into_iter()
                    .map(|value| imports::Request {
                        id: value.id,
                        revision: value.revision,
                        format: value.format,
                    })
                    .collect(),
            ),
            R::Provider(value) => Self::Provider(ProviderPoll {
                lease: value.lease.map(lease_from_wire),
                offer: offer_from_wire(required(value.offer, "provider offer")?)?,
                requests: value
                    .requests
                    .into_iter()
                    .map(native_from_wire)
                    .collect::<Result<_, _>>()?,
            }),
            R::Published(value) => Self::Published(value.accepted),
            R::Export(value) => Self::Export(value.offer.map(export_decode).transpose()?),
            R::Copy(value) => Self::Copy(value.copy.map(copy_decode).transpose()?),
            R::Error(value) => Self::Error(common::Error {
                code: value.code,
                message: value.message,
                retry: match value.retry {
                    0 => common::Retry::Unknown,
                    1 => common::Retry::Yes,
                    2 => common::Retry::No,
                    _ => return Err(Invalid("retry value").into()),
                },
            }),
        };
        response.validate()?;
        Ok(response)
    }
}
