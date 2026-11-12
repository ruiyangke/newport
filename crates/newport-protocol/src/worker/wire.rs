//! Worker controls share a top-level field with each service's directional
//! messages. Inspect the entire frame before decoding: protobuf's ordinary
//! last-one-wins behavior cannot distinguish mixed data and controls.
use bytes::Bytes;
use prost::Message;
use std::fmt;

use super::{Chunk, ClientControl, Exit, ServerControl, Signal};
use crate::encoding::protobuf::{self, FrameError, Limits, generated::newport::worker as pb};

mod scan;
#[cfg(test)]
mod tests;

pub const CONTROL_FIELD: u32 = 127;
pub const CONTROL_LIMITS: Limits = super::Transport::Opaque.limits(super::Direction::ToWorker);
pub const OPAQUE_LIMITS: Limits = CONTROL_LIMITS;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Frame(#[from] FrameError),
    #[error("worker control is mixed with service data")]
    MixedControl,
    #[error("duplicate worker control")]
    DuplicateControl,
    #[error("worker control must contain exactly one operation")]
    InvalidOperation,
    #[error("worker control has the wrong direction")]
    WrongDirection,
    #[error("unknown worker control operation")]
    UnknownOperation,
    #[error("worker controls must be consumed by the transport adapter")]
    UnexpectedControl,
    #[error("worker input is already complete")]
    InputComplete,
    #[error("worker stream has ended")]
    Ended,
    #[error(transparent)]
    Signal(#[from] super::InvalidSignal),
    #[error(transparent)]
    Chunk(#[from] super::InvalidChunk),
}

/// Service bodies are retained byte-for-byte, including fields this build
/// does not know. Their protocol decoder validates their metadata and payload.
#[derive(Clone, PartialEq, Eq)]
pub enum Frame<C> {
    Service(Bytes),
    Control(C),
}

impl<C: fmt::Debug> fmt::Debug for Frame<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Service(data) => f
                .debug_struct("Service")
                .field("length", &data.len())
                .finish_non_exhaustive(),
            Self::Control(control) => f.debug_tuple("Control").field(control).finish(),
        }
    }
}

/// Classify a complete client frame. The caller bounds the frame before
/// reading it and applies its service's decoder to returned service bodies.
pub fn decode_client(body: Bytes, limits: &Limits) -> Result<Frame<ClientControl>, Error> {
    let Some(control) = scan::control(&body, limits.message)? else {
        return Ok(Frame::Service(body));
    };
    let field = scan::single(control)?;
    let control = match field.number {
        1 if field.bytes()?.is_empty() => ClientControl::InputComplete,
        1 => return Err(Error::InvalidOperation),
        2 => ClientControl::Signal(Signal::new(field.u32()?)?),
        3 | 4 => return Err(Error::WrongDirection),
        _ => return Err(Error::UnknownOperation),
    };
    check_control::<pb::OpaqueClientFrame>(body, limits)?;
    Ok(Frame::Control(control))
}

/// Classify a complete server frame without copying service or stderr bytes.
pub fn decode_server(body: Bytes, limits: &Limits) -> Result<Frame<ServerControl>, Error> {
    let Some(control) = scan::control(&body, limits.message)? else {
        return Ok(Frame::Service(body));
    };
    let field = scan::single(control)?;
    let control = match field.number {
        1 | 2 => return Err(Error::WrongDirection),
        3 => ServerControl::Stderr(Chunk::new(body.slice_ref(field.bytes()?))?),
        4 => {
            let outcome = scan::single(field.bytes()?)?;
            ServerControl::Exit(match outcome.number {
                1 => Exit::Status(outcome.u32()?),
                2 => Exit::Signal(Signal::new(outcome.u32()?)?),
                _ => return Err(Error::UnknownOperation),
            })
        }
        _ => return Err(Error::UnknownOperation),
    };
    check_control::<pb::OpaqueServerFrame>(body, limits)?;
    Ok(Frame::Control(control))
}

fn check_control<M: Message + Default + protobuf::Layout>(
    body: Bytes,
    limits: &Limits,
) -> Result<(), Error> {
    protobuf::decode::<M>(
        body,
        &Limits {
            message: limits.message.min(CONTROL_LIMITS.message),
            metadata: limits.metadata.min(CONTROL_LIMITS.metadata),
        },
    )?;
    Ok(())
}

/// Direct service endpoints reject controls, even when another top-level
/// field would hide them during ordinary oneof decoding.
pub fn reject_controls(body: &[u8], limit: usize) -> Result<(), Error> {
    if scan::control(body, limit)?.is_some() {
        return Err(Error::UnexpectedControl);
    }
    Ok(())
}

fn encode<M: Message + protobuf::Layout>(message: &M) -> Result<Vec<u8>, Error> {
    let mut frame = Vec::new();
    protobuf::write_message(&mut frame, message, &CONTROL_LIMITS)?;
    Ok(frame)
}

/// A complete varint-delimited control frame, usable on any carried service.
pub fn encode_client(control: ClientControl) -> Result<Vec<u8>, Error> {
    use pb::{client_control::Operation, opaque_client_frame::Message};
    let operation = match control {
        ClientControl::InputComplete => Operation::InputComplete(protobuf::Present {}),
        ClientControl::Signal(signal) => Operation::Signal(signal.number()),
    };
    encode(&pb::OpaqueClientFrame {
        message: Some(Message::WorkerControl(pb::ClientControl {
            operation: Some(operation),
        })),
    })
}

pub fn encode_server(control: &ServerControl) -> Result<Vec<u8>, Error> {
    use pb::{exit::Outcome, opaque_server_frame::Message, server_control::Operation};
    let operation = match control {
        ServerControl::Stderr(chunk) => Operation::Stderr(chunk.data().clone()),
        ServerControl::Exit(exit) => Operation::Exit(pb::Exit {
            outcome: Some(match exit {
                Exit::Status(status) => Outcome::Status(*status),
                Exit::Signal(signal) => Outcome::Signal(signal.number()),
            }),
        }),
    };
    encode(&pb::OpaqueServerFrame {
        message: Some(Message::WorkerControl(pb::ServerControl {
            operation: Some(operation),
        })),
    })
}

/// Encode external protocol bytes only after registration selected the opaque
/// transport. Ordinary Newport service messages use their native schemas.
pub fn encode_opaque(chunk: &Chunk, direction: super::Direction) -> Result<Vec<u8>, Error> {
    match direction {
        super::Direction::ToWorker => encode(&pb::OpaqueClientFrame {
            message: Some(pb::opaque_client_frame::Message::Data(chunk.data().clone())),
        }),
        super::Direction::FromWorker => encode(&pb::OpaqueServerFrame {
            message: Some(pb::opaque_server_frame::Message::Data(chunk.data().clone())),
        }),
    }
}

/// Decode an opaque service body after consuming worker controls. A single
/// bytes field prevents accidental concatenation of duplicate data cases.
pub fn decode_opaque(body: Bytes) -> Result<Chunk, Error> {
    reject_controls(&body, OPAQUE_LIMITS.message)?;
    let field = scan::single(&body)?;
    if field.number != 1 {
        return Err(Error::UnknownOperation);
    }
    let chunk = Chunk::new(body.slice_ref(field.bytes()?))?;
    protobuf::decode::<pb::OpaqueClientFrame>(body, &OPAQUE_LIMITS)?;
    Ok(chunk)
}

/// Checks client half-close ordering. Signals remain valid after stdin closes.
/// Any framing error makes this decoder terminal; its adapter closes the stream.
#[derive(Default)]
pub struct ClientDecoder {
    input_complete: bool,
    ended: bool,
}

impl ClientDecoder {
    pub fn decode(&mut self, body: Bytes, limits: &Limits) -> Result<Frame<ClientControl>, Error> {
        if self.ended {
            return Err(Error::Ended);
        }
        let result = decode_client(body, limits).and_then(|frame| {
            match &frame {
                Frame::Service(_) | Frame::Control(ClientControl::InputComplete)
                    if self.input_complete =>
                {
                    return Err(Error::InputComplete);
                }
                Frame::Control(ClientControl::InputComplete) => self.input_complete = true,
                _ => {}
            }
            Ok(frame)
        });
        self.ended = result.is_err();
        result
    }
}

/// An exit is the last server frame, after its bounded stdout/stderr drain.
#[derive(Default)]
pub struct ServerDecoder {
    ended: bool,
}

impl ServerDecoder {
    pub fn decode(&mut self, body: Bytes, limits: &Limits) -> Result<Frame<ServerControl>, Error> {
        if self.ended {
            return Err(Error::Ended);
        }
        let result = decode_server(body, limits);
        self.ended =
            result.is_err() || matches!(&result, Ok(Frame::Control(ServerControl::Exit(_))));
        result
    }
}
