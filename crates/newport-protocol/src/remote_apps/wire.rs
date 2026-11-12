//! The Remote Apps wire: each message is one `ClientFrame` or `ServerFrame`
//! in newport-protocol's length-delimited framing, read within the direction's
//! [`Limits`] before it is decoded.
pub use crate::encoding::protobuf::Limits;
use crate::encoding::protobuf::generated::newport::remote::apps::{ClientFrame, ServerFrame};
use crate::encoding::protobuf::{FrameError, Layout};
use crate::remote_apps::v1::{ClientMessage, ServerMessage};
use prost::Message;
use std::io::{self, Read, Write};

/// Why a message could not be read, including whether its entire frame was consumed.
#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    /// The peer stopped or the transport failed; the transport owner knows its outcome.
    #[error("Display disconnected: {0}")]
    Disconnected(#[source] io::Error),
    /// A complete frame was consumed. Reading may continue at the next frame.
    #[error("Unreadable display message: {0}")]
    Unreadable(#[source] MessageError),
    /// Framing failed. The stream cannot be resumed at a known boundary.
    #[error("{0}")]
    Frame(#[from] FrameError),
    /// Audio payload bounds are enforced even while the session is stopping.
    #[error("Invalid audio payload length")]
    InvalidAudioPayload,
}

/// Failure decoding a fully consumed Remote Apps frame.
#[derive(Debug, thiserror::Error)]
pub enum MessageError {
    #[error("{0}")]
    Protobuf(#[from] FrameError),
    #[error("{0}")]
    WorkerControl(#[from] crate::worker::wire::Error),
    #[error("{0}")]
    Invalid(String),
}

/// A direction's top-level message.
trait Frame: Message + Default + Layout {
    type Message: TryFrom<Self::Case, Error = String>;
    type Case;
    /// Its case, or None for one this build doesn't know.
    fn case(self) -> Option<Self::Case>;
}
impl Frame for ServerFrame {
    type Message = ServerMessage;
    type Case = crate::encoding::protobuf::generated::newport::remote::apps::server_frame::Message;
    fn case(self) -> Option<Self::Case> {
        self.message
    }
}
impl Frame for ClientFrame {
    type Message = ClientMessage;
    type Case = crate::encoding::protobuf::generated::newport::remote::apps::client_frame::Message;
    fn case(self) -> Option<Self::Case> {
        self.message
    }
}

/// One message, and the bytes its frame took on the wire. A frame of a
/// message this build doesn't know is skipped, as newer peers' are.
fn read<F: Frame>(
    reader: &mut impl Read,
    limits: &Limits,
) -> Result<(F::Message, usize), ReadError> {
    loop {
        let frame = match crate::encoding::protobuf::read_frame(reader, limits.message) {
            Ok(Some(frame)) => frame,
            Ok(None) => {
                return Err(ReadError::Disconnected(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "the stream ended",
                )));
            }
            Err(FrameError::Io(error)) => return Err(ReadError::Disconnected(error)),
            Err(error) => return Err(ReadError::Frame(error)),
        };
        crate::worker::wire::reject_controls(&frame, limits.message)
            .map_err(|error| ReadError::Unreadable(MessageError::WorkerControl(error)))?;
        let size = prost::length_delimiter_len(frame.len()) + frame.len();
        let frame = crate::encoding::protobuf::decode::<F>(frame, limits)
            .map_err(|error| ReadError::Unreadable(MessageError::Protobuf(error)))?;
        if let Some(case) = frame.case() {
            return F::Message::try_from(case)
                .map(|message| (message, size))
                .map_err(|error| ReadError::Unreadable(MessageError::Invalid(error)));
        }
    }
}

/// Reads one agent message within [`SERVER_LIMITS`](crate::remote_apps::SERVER_LIMITS).
pub fn read_server(reader: &mut impl Read) -> Result<(ServerMessage, usize), ReadError> {
    read::<ServerFrame>(reader, &crate::remote_apps::SERVER_LIMITS)
}
/// Reads one client message within [`CLIENT_LIMITS`](crate::remote_apps::CLIENT_LIMITS).
pub fn read_client(reader: &mut impl Read) -> Result<(ClientMessage, usize), ReadError> {
    read::<ClientFrame>(reader, &crate::remote_apps::CLIENT_LIMITS)
}

fn encode<F: Message + Layout>(frame: &F, limits: &Limits) -> Result<Vec<u8>, String> {
    let mut encoded = Vec::new();
    crate::encoding::protobuf::write_message(&mut encoded, frame, limits)
        .map_err(|e| e.to_string())?;
    Ok(encoded)
}
/// `message` as one frame, if it is within what the client reads.
pub fn encode_server(message: &ServerMessage) -> Result<Vec<u8>, String> {
    encode(
        &ServerFrame::from(message),
        &crate::remote_apps::SERVER_LIMITS,
    )
}
/// `message` as one frame, if it is within what the agent reads.
pub fn encode_client(message: &ClientMessage) -> Result<Vec<u8>, String> {
    encode(
        &ClientFrame::from(message),
        &crate::remote_apps::CLIENT_LIMITS,
    )
}
/// Writes one client message, in one write.
pub fn write_client(writer: &mut impl Write, message: &ClientMessage) -> Result<(), String> {
    let encoded = encode_client(message)?;
    writer.write_all(&encoded).map_err(|e| e.to_string())
}

#[cfg(all(test, feature = "protobuf"))]
mod tests {
    use super::*;
    use crate::remote_apps::v1::*;
    use std::io::Cursor;

    fn heartbeat(session: u64) -> ClientMessage {
        ClientMessage::Heartbeat { session }
    }

    #[test]
    fn scroll_requires_an_explicit_pointer_position() {
        use crate::encoding::protobuf::generated::newport::remote::apps as pb;
        let input = pb::Input {
            kind: Some(pb::input::Kind::Scroll(pb::input::Scroll::default())),
        };
        assert!(Input::try_from(Some(input)).is_err());
    }

    #[test]
    fn scroll_position_survives_protobuf_including_the_origin() {
        for position in [[0, 0], [21, 31], [u32::MAX, u32::MAX]] {
            let message = ClientMessage::Input {
                session: 7,
                window: WindowId {
                    id: 3,
                    generation: 2,
                },
                surface: 1,
                input: Input::Scroll {
                    position,
                    dx: 0.5,
                    dy: -12.25,
                    discrete: None,
                    phase: ScrollPhase::Update,
                    modifiers: Modifiers::default(),
                },
            };
            let encoded = encode_client(&message).unwrap();
            assert!(matches!(
                read_client(&mut encoded.as_slice()).unwrap().0,
                ClientMessage::Input { input: Input::Scroll { position: actual, .. }, .. }
                    if actual == position
            ));
        }
    }

    #[test]
    fn direct_endpoints_reject_worker_controls_before_oneof_decoding() {
        use crate::worker::{ClientControl, Exit, ServerControl, wire as worker};
        let client_control = worker::encode_client(ClientControl::InputComplete).unwrap();
        let server_control = worker::encode_server(&ServerControl::Exit(Exit::Status(0))).unwrap();
        let heartbeat = encode_client(&heartbeat(7)).unwrap();
        let ended = encode_server(&ServerMessage::Closed { session: 7 }).unwrap();
        for (control, message, client) in [
            (&client_control, &heartbeat, true),
            (&server_control, &ended, false),
        ] {
            let mut capture = [control.as_slice(), message.as_slice()].concat();
            for mixed in [false, true] {
                if mixed {
                    let control =
                        crate::encoding::protobuf::read_frame(&mut control.as_slice(), 1024)
                            .unwrap()
                            .unwrap();
                    let message =
                        crate::encoding::protobuf::read_frame(&mut message.as_slice(), 1024)
                            .unwrap()
                            .unwrap();
                    capture.clear();
                    crate::encoding::protobuf::write_frame(
                        &mut capture,
                        &[control.as_ref(), message.as_ref()].concat(),
                    )
                    .unwrap();
                }
                let error = if client {
                    read_client(&mut capture.as_slice()).unwrap_err()
                } else {
                    read_server(&mut capture.as_slice()).unwrap_err()
                };
                assert!(matches!(error, ReadError::Unreadable(_)), "{error}");
            }
        }
    }

    #[test]
    fn frames_round_trip_and_name_their_size() {
        let mut wire = Vec::new();
        write_client(&mut wire, &heartbeat(7)).unwrap();
        write_client(&mut wire, &heartbeat(8)).unwrap();
        let mut reader = Cursor::new(&wire);
        let (first, size) = read_client(&mut reader).unwrap();
        assert!(matches!(first, ClientMessage::Heartbeat { session: 7 }));
        assert_eq!(size, wire.len() / 2);
        assert!(matches!(
            read_client(&mut reader).unwrap().0,
            ClientMessage::Heartbeat { session: 8 }
        ));
        assert!(matches!(
            read_client(&mut reader),
            Err(ReadError::Disconnected(_))
        ));
    }

    #[test]
    fn a_frame_over_the_limit_is_refused_before_its_body_is_read() {
        let mut length = Vec::new();
        prost::encoding::encode_varint(
            crate::remote_apps::CLIENT_LIMITS.message as u64 + 1,
            &mut length,
        );
        let error = read_client(&mut length.as_slice()).unwrap_err();
        assert!(
            matches!(error, ReadError::Frame(FrameError::TooLarge { .. })),
            "{error}"
        );
        // A key longer than the agent reads fails where it is made.
        let long = ClientMessage::Input {
            session: 1,
            window: WindowId {
                id: 1,
                generation: 1,
            },
            surface: 1,
            input: Input::Key {
                key: "x".repeat(crate::remote_apps::CLIENT_LIMITS.message),
                code: 1,
                down: true,
                repeat: false,
                modifiers: Modifiers::default(),
            },
        };
        assert!(encode_client(&long).is_err());
    }

    #[test]
    fn an_unreadable_frame_leaves_the_stream_at_the_next() {
        let mut wire = Vec::new();
        // A Hello whose version is past what a u16 holds.
        let hello = ClientFrame {
            message: Some(
                crate::encoding::protobuf::generated::newport::remote::apps::client_frame::Message::Hello(
                    crate::encoding::protobuf::generated::newport::remote::apps::client_frame::Hello {
                        versions: vec![70_000],
                        ..Default::default()
                    },
                ),
            ),
        };
        crate::encoding::protobuf::write_message(
            &mut wire,
            &hello,
            &crate::remote_apps::CLIENT_LIMITS,
        )
        .unwrap();
        crate::encoding::protobuf::write_frame(&mut wire, &[0xff]).unwrap();
        write_client(&mut wire, &heartbeat(3)).unwrap();
        let mut reader = Cursor::new(&wire);
        for _ in 0..2 {
            assert!(matches!(
                read_client(&mut reader),
                Err(ReadError::Unreadable(_))
            ));
        }
        assert!(matches!(
            read_client(&mut reader).unwrap().0,
            ClientMessage::Heartbeat { session: 3 }
        ));
    }

    #[test]
    fn read_errors_preserve_sources_and_frame_boundary_classification() {
        use std::error::Error as _;

        let ended = read_client(&mut [].as_slice()).unwrap_err();
        let ReadError::Disconnected(source) = &ended else {
            panic!("{ended}")
        };
        assert_eq!(source.kind(), io::ErrorKind::UnexpectedEof);
        assert!(ended.source().unwrap().is::<io::Error>());

        struct Reset;
        impl Read for Reset {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::new(io::ErrorKind::ConnectionReset, "peer reset"))
            }
        }
        let reset = read_client(&mut Reset).unwrap_err();
        let source = reset.source().unwrap().downcast_ref::<io::Error>().unwrap();
        assert_eq!(source.kind(), io::ErrorKind::ConnectionReset);
        assert_eq!(source.to_string(), "peer reset");

        for mut incomplete in [&[0x80][..], &[2, 0][..]] {
            let error = read_client(&mut incomplete).unwrap_err();
            assert!(matches!(error, ReadError::Frame(FrameError::Truncated)));
            assert!(error.source().unwrap().is::<FrameError>());
        }
        let error = read_client(&mut &[1, 0xff][..]).unwrap_err();
        let source = error
            .source()
            .unwrap()
            .downcast_ref::<MessageError>()
            .unwrap();
        assert!(matches!(source, MessageError::WorkerControl(_)));
        assert!(source.source().unwrap().is::<crate::worker::wire::Error>());

        let error = read_server(&mut &[2, 8, 1][..]).unwrap_err();
        let source = error
            .source()
            .unwrap()
            .downcast_ref::<MessageError>()
            .unwrap();
        assert!(matches!(source, MessageError::Protobuf(_)));
        assert!(source.source().unwrap().is::<FrameError>());
    }

    #[test]
    fn a_message_this_build_does_not_know_is_skipped() {
        let mut wire = Vec::new();
        // A `ClientFrame` whose message is a case numbered after every
        // known one.
        let mut unknown = Vec::new();
        prost::encoding::encode_key(99, prost::encoding::WireType::LengthDelimited, &mut unknown);
        prost::encoding::encode_varint(0, &mut unknown);
        crate::encoding::protobuf::write_frame(&mut wire, &unknown).unwrap();
        write_client(&mut wire, &heartbeat(4)).unwrap();
        let (message, size) = read_client(&mut wire.as_slice()).unwrap();
        assert!(matches!(message, ClientMessage::Heartbeat { session: 4 }));
        assert_eq!(size, wire.len() - unknown.len() - 1);
    }

    #[test]
    fn metadata_is_bounded_apart_from_tile_data() {
        let frame = |data: usize| ServerMessage::Frame {
            session: 1,
            window: WindowId {
                id: 1,
                generation: 1,
            },
            surface: 1,
            sequence: 1,
            keyframe: true,
            tiles: vec![crate::remote_apps::tile::Tile::new(
                0,
                crate::remote_apps::tile::Codec::RawBgra,
                vec![0; data],
            )],
            captured: None,
            diagnostics: None,
        };
        let large = encode_server(&frame(crate::remote_apps::MAX_HEADER_BYTES * 4)).unwrap();
        let (message, _) = read_server(&mut large.as_slice()).unwrap();
        assert_eq!(message.data_len(), crate::remote_apps::MAX_HEADER_BYTES * 4);
        // The same size in a string is metadata, and over its limit.
        let error = ServerMessage::Error {
            session: 1,
            message: "x".repeat(crate::remote_apps::MAX_HEADER_BYTES + 1),
        };
        assert!(encode_server(&error).is_err());
    }
}
