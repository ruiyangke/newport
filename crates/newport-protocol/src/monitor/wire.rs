//! Directional, bounded monitor messages with operation-specific reply checks.
use super::ports::discovery::wire::Record as DiscoveryRecord;
use super::{
    CLIENT_LIMITS, Call, Invalid, InvalidRequest, Reply, Request, Response, SERVER_LIMITS,
};
use crate::{
    common,
    encoding::protobuf::{self, FrameError, generated::newport::monitor as pb},
};
use bytes::Bytes;
use std::io::{self, Read, Write};

mod compose;
mod containers;
mod file_watch;
mod metrics;
pub(crate) mod scan;
mod services;
mod stable_display;
#[cfg(test)]
mod tests;

/// The enum maps the monitor protocol's `Record` impls hold.
#[cfg(test)]
pub(crate) fn enum_maps() -> Vec<crate::desktop::wire::wire_tables::EnumMap> {
    fn record<T: Record<Wire = i32> + std::fmt::Debug + 'static>(
        proto: &'static str,
    ) -> crate::desktop::wire::wire_tables::EnumMap {
        crate::desktop::wire::wire_tables::enum_map(
            proto,
            |value| T::from_wire(value).ok(),
            T::to_wire,
        )
    }
    vec![
        record::<super::compose::CliKind>("newport.monitor.ComposeCliKind"),
        record::<super::compose::State>("newport.monitor.ComposeState"),
        record::<super::containers::ContainerVerb>("newport.monitor.ContainersContainerVerb"),
        record::<super::containers::ProjectVerb>("newport.monitor.ContainersProjectVerb"),
        record::<super::containers::PruneTarget>("newport.monitor.ContainersPruneTarget"),
        record::<super::containers::ContainerState>("newport.monitor.ContainersContainerState"),
        record::<super::containers::ContainerHealth>("newport.monitor.ContainersContainerHealth"),
        record::<super::containers::PortProtocol>("newport.monitor.ContainersPortProtocol"),
        record::<super::containers::LogStream>("newport.monitor.ContainersLogStream"),
        record::<super::containers::DiskPart>("newport.monitor.ContainersDiskPart"),
        record::<super::services::Scope>("newport.monitor.ServicesScope"),
        record::<super::services::Privilege>("newport.monitor.ServicesPrivilege"),
        record::<super::services::Verb>("newport.monitor.ServicesVerb"),
        record::<super::services::Priority>("newport.monitor.ServicesPriority"),
        record::<super::services::ExitKind>("newport.monitor.ServicesExitKind"),
    ]
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Frame(#[from] FrameError),
    #[error(transparent)]
    Invalid(#[from] Invalid),
    #[error(transparent)]
    Request(#[from] InvalidRequest),
    #[error(transparent)]
    Control(#[from] crate::worker::wire::Error),
    #[error("monitor encoded field budget exceeded")]
    FieldBudget,
    #[error("monitor stream ended")]
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
pub(crate) trait Record: Sized {
    type Wire;
    fn to_wire(&self) -> Self::Wire;
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid>;
}
fn required<T>(value: Option<T>) -> Result<T, Invalid> {
    value.ok_or(Invalid("required field"))
}
fn labels(values: Vec<pb::Label>) -> Result<std::collections::BTreeMap<String, String>, Invalid> {
    let mut labels = std::collections::BTreeMap::new();
    for value in values {
        if labels.insert(value.key, value.value).is_some() {
            return Err(Invalid("duplicate container label"));
        }
    }
    Ok(labels)
}
fn check(body: &[u8], client: bool) -> Result<(), Error> {
    let limits = if client {
        &CLIENT_LIMITS
    } else {
        &SERVER_LIMITS
    };
    crate::worker::wire::reject_controls(body, limits.message)?;
    scan::check(body, client)?;
    Ok(())
}
fn encode<M: prost::Message + protobuf::Layout>(value: &M, client: bool) -> Result<Vec<u8>, Error> {
    let limits = if client {
        &CLIENT_LIMITS
    } else {
        &SERVER_LIMITS
    };
    // Validate the encoded shape as well as the Rust record before sending it.
    let length = value.encoded_len();
    if length > limits.message {
        return Err(FrameError::TooLarge {
            length: length as u64,
            limit: limits.message,
        }
        .into());
    }
    let body = value.encode_to_vec();
    check(&body, client)?;
    let mut output = Vec::new();
    protobuf::write_message(&mut output, value, limits)?;
    Ok(output)
}
pub fn encode_request(call: &Call) -> Result<Vec<u8>, Error> {
    use pb::call::Request as Message;
    call.validate()?;
    let message = match &call.request {
        Request::Metrics => Message::Metrics(protobuf::Present {}),
        Request::StableDisplay => Message::StableDisplay(protobuf::Present {}),
        Request::Services(v) => Message::Services(v.to_wire()),
        Request::Containers(v) => Message::Containers(v.to_wire()),
        Request::Compose(v) => Message::Compose(v.to_wire()),
        Request::Ports(value) => Message::Discovery(
            crate::encoding::protobuf::generated::newport::discovery::Request {
                refresh: value.refresh,
            },
        ),
        Request::FileWatch(v) => Message::FileWatch(v.to_wire()),
    };
    encode(
        &pb::ClientFrame {
            message: Some(pb::client_frame::Message::Call(pb::Call {
                id: call.id,
                request: Some(message),
            })),
        },
        true,
    )
}
pub fn decode_request(body: Bytes) -> Result<Call, Error> {
    use pb::call::Request as Message;
    check(&body, true)?;
    let frame: pb::ClientFrame = protobuf::decode(body, &CLIENT_LIMITS)?;
    let value = match required(frame.message)? {
        pb::client_frame::Message::Call(v) => v,
        pb::client_frame::Message::WorkerControl(_) => return Err(Invalid("worker control").into()),
    };
    let request = match required(value.request)? {
        Message::Metrics(_) => Request::Metrics,
        Message::StableDisplay(_) => Request::StableDisplay,
        Message::Services(v) => Request::Services(super::services::Request::from_wire(v)?),
        Message::Containers(v) => Request::Containers(super::containers::Request::from_wire(v)?),
        Message::Compose(v) => Request::Compose(super::compose::Request::from_wire(v)?),
        Message::Discovery(value) => Request::Ports(super::ports::discovery::Request {
            refresh: value.refresh,
        }),
        Message::FileWatch(v) => Request::FileWatch(super::file_watch::Request::from_wire(v)?),
    };
    let call = Call {
        id: value.id,
        request,
    };
    call.validate()?;
    Ok(call)
}
pub fn encode_reply(response: &Response, call: &Call) -> Result<Vec<u8>, Error> {
    use pb::response::Reply as Message;
    response.validate_for(call)?;
    let message = match &response.reply {
        Reply::Metrics(v) => Message::Metrics(v.to_wire()),
        Reply::StableDisplay(v) => Message::StableDisplay(v.to_wire()),
        Reply::Services(v) => Message::Services(v.to_wire()),
        Reply::Containers(v) => Message::Containers(v.to_wire()),
        Reply::Compose(v) => Message::Compose(v.to_wire()),
        Reply::Ports(v) => Message::Discovery(v.to_wire()),
        Reply::FileWatch(v) => Message::FileWatch(v.to_wire()),
        Reply::Failed(v) => Message::Error(protobuf::Error {
            code: v.code.clone(),
            message: v.message.clone(),
            retry: match v.retry {
                common::Retry::Unknown => 0,
                common::Retry::Yes => 1,
                common::Retry::No => 2,
            },
        }),
    };
    encode(
        &pb::ServerFrame {
            message: Some(pb::server_frame::Message::Response(Box::new(
                pb::Response {
                    id: response.id,
                    reply: Some(message),
                },
            ))),
        },
        false,
    )
}
pub fn decode_reply(body: Bytes, call: &Call) -> Result<Response, Error> {
    use pb::response::Reply as Message;
    check(&body, false)?;
    let frame: pb::ServerFrame = protobuf::decode(body, &SERVER_LIMITS)?;
    let value = match required(frame.message)? {
        pb::server_frame::Message::Response(v) => *v,
        pb::server_frame::Message::WorkerControl(_) => return Err(Invalid("worker control").into()),
    };
    let reply = match required(value.reply)? {
        Message::Metrics(v) => Reply::Metrics(super::metrics::Snapshot::from_wire(v)?),
        Message::StableDisplay(v) => {
            Reply::StableDisplay(super::stable_display::State::from_wire(v)?)
        }
        Message::Services(v) => Reply::Services(super::services::Reply::from_wire(v)?),
        Message::Containers(v) => Reply::Containers(super::containers::Reply::from_wire(v)?),
        Message::Compose(v) => Reply::Compose(super::compose::Reply::from_wire(v)?),
        Message::Discovery(v) => {
            Reply::Ports(Box::new(super::ports::discovery::Snapshot::from_wire(v)?))
        }
        Message::FileWatch(v) => Reply::FileWatch(super::file_watch::Reply::from_wire(v)?),
        Message::Error(v) => Reply::Failed(common::Error {
            code: v.code,
            message: v.message,
            retry: match v.retry {
                0 => common::Retry::Unknown,
                1 => common::Retry::Yes,
                2 => common::Retry::No,
                _ => return Err(Invalid("retry value").into()),
            },
        }),
    };
    let response = Response {
        id: value.id,
        reply,
    };
    response.validate_for(call)?;
    Ok(response)
}
pub fn read_request(input: &mut impl Read) -> Result<Option<Call>, Error> {
    protobuf::read_frame(input, CLIENT_LIMITS.message)?
        .map(decode_request)
        .transpose()
}
pub fn read_reply(input: &mut impl Read, call: &Call) -> Result<Response, Error> {
    let body = protobuf::read_frame(input, SERVER_LIMITS.message)?.ok_or(Error::Ended)?;
    decode_reply(body, call)
}
pub fn write_reply(output: &mut impl Write, response: &Response, call: &Call) -> Result<(), Error> {
    output
        .write_all(&encode_reply(response, call)?)
        .map_err(FrameError::from)?;
    output.flush().map_err(FrameError::from)?;
    Ok(())
}
