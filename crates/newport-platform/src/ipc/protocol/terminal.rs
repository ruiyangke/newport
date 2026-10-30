//! A terminal tab's I/O, carried both ways on its lease socket after the open
//! reply, as frames: `[kind: u8][length: u32 BE][payload]`.
//!
//! Output flows against credit: the backend sends output while less than
//! [`OUTPUT_CREDIT`] bytes of it are unacknowledged, and the app acknowledges
//! output as its view consumes it. Input flows against a window: the app sends
//! input while less than [`INPUT_WINDOW`] bytes of it wait for the shell, and
//! the backend reports input as the shell's queue takes it. Neither direction
//! waits for the other, so a slow view and a shell that isn't reading its input
//! never stall each other, and nothing on the stream times out.
//!
//! The epoch counts the shells the stream has left behind (a reconnect, an
//! exit or a failure). Input names the epoch it was typed for, so keystrokes
//! queued for one shell never reach its replacement.
use serde::{Deserialize, Serialize};
use std::io::Read;

/// Output bytes the backend sends before the app acknowledges them.
pub const OUTPUT_CREDIT: u64 = 1 << 20;
/// Input bytes the app sends before the backend reports them taken.
pub const INPUT_WINDOW: u64 = 256 << 10;
/// The most input or output one frame carries.
pub const MAX_DATA: usize = 64 << 10;
/// Events carry error text; nothing legitimate comes close to this.
const MAX_PAYLOAD: usize = 1 << 20;
pub const HEADER: usize = 5;

const INPUT: u8 = 0;
const RESIZE: u8 = 1;
const ACK: u8 = 2;
const OUTPUT: u8 = 0;
const EVENT: u8 = 1;
const TAKEN: u8 = 2;

/// App to backend.
#[derive(Debug, PartialEq)]
pub enum ToBackend {
    Input {
        epoch: u64,
        data: Vec<u8>,
    },
    Resize {
        cols: u16,
        rows: u16,
    },
    /// Output bytes the view has consumed.
    Ack(u64),
}

/// Backend to app.
#[derive(Debug, PartialEq)]
pub enum ToApp {
    Output(Vec<u8>),
    Event(Event),
    /// Input bytes the shell's queue has taken, or dropped as stale.
    Taken(u64),
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "camelCase")]
pub enum Event {
    Ready,
    Reconnecting { attempt: u32, message: String },
    Exit(Option<u32>),
    Error(String),
}
impl Event {
    /// The shell before this event is gone; input typed for it is stale.
    pub fn ends_shell(&self) -> bool {
        !matches!(self, Self::Ready)
    }
    /// Nothing follows this event on the stream.
    pub fn ends_stream(&self) -> bool {
        matches!(self, Self::Exit(_) | Self::Error(_))
    }
}

fn frame(kind: u8, parts: &[&[u8]]) -> Vec<u8> {
    let length: usize = parts.iter().map(|part| part.len()).sum();
    let mut bytes = Vec::with_capacity(HEADER + length);
    bytes.push(kind);
    bytes.extend_from_slice(&(length as u32).to_be_bytes());
    for part in parts {
        bytes.extend_from_slice(part);
    }
    bytes
}

fn number(payload: &[u8]) -> Result<u64, String> {
    Ok(u64::from_be_bytes(
        payload.try_into().map_err(|_| "Invalid terminal frame.")?,
    ))
}

fn data(payload: Vec<u8>) -> Result<Vec<u8>, String> {
    if payload.len() > MAX_DATA {
        return Err("Terminal frame is too large.".into());
    }
    Ok(payload)
}

impl ToBackend {
    pub fn encode(&self) -> Vec<u8> {
        match self {
            Self::Input { epoch, data } => frame(INPUT, &[&epoch.to_be_bytes(), data]),
            Self::Resize { cols, rows } => {
                frame(RESIZE, &[&cols.to_be_bytes(), &rows.to_be_bytes()])
            }
            Self::Ack(bytes) => frame(ACK, &[&bytes.to_be_bytes()]),
        }
    }
    pub fn decode(kind: u8, mut payload: Vec<u8>) -> Result<Self, String> {
        match kind {
            INPUT if payload.len() >= 8 => {
                let epoch = number(&payload[..8])?;
                payload.drain(..8);
                Ok(Self::Input {
                    epoch,
                    data: data(payload)?,
                })
            }
            RESIZE if payload.len() == 4 => Ok(Self::Resize {
                cols: u16::from_be_bytes([payload[0], payload[1]]),
                rows: u16::from_be_bytes([payload[2], payload[3]]),
            }),
            ACK => Ok(Self::Ack(number(&payload)?)),
            _ => Err("Invalid terminal frame.".into()),
        }
    }
}

impl ToApp {
    pub fn encode(&self) -> Vec<u8> {
        match self {
            Self::Output(data) => frame(OUTPUT, &[data]),
            Self::Event(event) => frame(
                EVENT,
                &[&serde_json::to_vec(event).expect("terminal events serialize")],
            ),
            Self::Taken(bytes) => frame(TAKEN, &[&bytes.to_be_bytes()]),
        }
    }
    pub fn decode(kind: u8, payload: Vec<u8>) -> Result<Self, String> {
        match kind {
            OUTPUT => Ok(Self::Output(data(payload)?)),
            EVENT => serde_json::from_slice(&payload)
                .map(Self::Event)
                .map_err(|error| format!("Invalid terminal event: {error}")),
            TAKEN => Ok(Self::Taken(number(&payload)?)),
            _ => Err("Invalid terminal frame.".into()),
        }
    }
}

/// A frame's kind and payload length, from its header.
pub fn header(bytes: [u8; HEADER]) -> std::io::Result<(u8, usize)> {
    let length = u32::from_be_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]) as usize;
    if length > MAX_PAYLOAD {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Terminal frame is too large.",
        ));
    }
    Ok((bytes[0], length))
}

/// The next frame, or `None` when the stream ends between frames.
pub fn read(reader: &mut impl Read) -> std::io::Result<Option<(u8, Vec<u8>)>> {
    let mut bytes = [0; HEADER];
    let mut filled = 0;
    while filled < HEADER {
        match reader.read(&mut bytes[filled..]) {
            Ok(0) if filled == 0 => return Ok(None),
            Ok(0) => return Err(std::io::ErrorKind::UnexpectedEof.into()),
            Ok(count) => filled += count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    let (kind, length) = header(bytes)?;
    let mut payload = vec![0; length];
    reader.read_exact(&mut payload)?;
    Ok(Some((kind, payload)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip_to_backend(frame: ToBackend) {
        let bytes = frame.encode();
        let (kind, payload) = read(&mut bytes.as_slice()).unwrap().unwrap();
        assert_eq!(ToBackend::decode(kind, payload).unwrap(), frame);
    }

    #[test]
    fn frames_round_trip_binary_data_and_events() {
        round_trip_to_backend(ToBackend::Input {
            epoch: 3,
            data: vec![0, 255, 13, 10],
        });
        round_trip_to_backend(ToBackend::Resize { cols: 500, rows: 1 });
        round_trip_to_backend(ToBackend::Ack(OUTPUT_CREDIT));
        for frame in [
            ToApp::Output(vec![0, 255, 13, 10]),
            ToApp::Event(Event::Reconnecting {
                attempt: 2,
                message: "retry".into(),
            }),
            ToApp::Event(Event::Exit(None)),
            ToApp::Taken(7),
        ] {
            let bytes = frame.encode();
            let (kind, payload) = read(&mut bytes.as_slice()).unwrap().unwrap();
            assert_eq!(ToApp::decode(kind, payload).unwrap(), frame);
        }
    }

    #[test]
    fn a_stream_ends_cleanly_only_between_frames() {
        assert!(read(&mut [].as_slice()).unwrap().is_none());
        let bytes = ToApp::Taken(1).encode();
        assert!(read(&mut &bytes[..3]).is_err());
        assert!(read(&mut &bytes[..HEADER + 2]).is_err());
    }

    #[test]
    fn oversized_and_malformed_frames_are_refused() {
        let mut huge = vec![OUTPUT];
        huge.extend_from_slice(&u32::MAX.to_be_bytes());
        assert!(read(&mut huge.as_slice()).is_err());
        assert!(ToApp::decode(OUTPUT, vec![0; MAX_DATA + 1]).is_err());
        assert!(ToBackend::decode(INPUT, vec![0; 7]).is_err());
        assert!(ToBackend::decode(RESIZE, vec![0; 3]).is_err());
        assert!(ToBackend::decode(9, vec![]).is_err());
    }
}
