use super::*;
use crate::worker::{Direction, MAX_CHUNK};

#[cfg(feature = "trace")]
#[test]
fn carried_schemas_share_the_reserved_control_field_and_direction() {
    use prost_reflect::{DescriptorPool, Kind};
    let pool = DescriptorPool::decode(protobuf::DESCRIPTORS).unwrap();
    for package in [
        "newport.worker.Opaque",
        "newport.remote.apps.",
        "newport.remote.catalogue.",
        "newport.terminal.",
        "newport.integration.",
        "newport.git.",
        "newport.monitor.",
    ] {
        for direction in ["Client", "Server"] {
            let name = format!("{package}{direction}Frame");
            let message = pool.get_message_by_name(&name).unwrap();
            let control = message.get_field(CONTROL_FIELD).unwrap();
            assert_eq!(control.name(), "worker_control");
            assert_eq!(control.containing_oneof().unwrap().name(), "message");
            let Kind::Message(target) = control.kind() else {
                panic!("control message")
            };
            assert_eq!(
                target.full_name(),
                format!("newport.worker.{direction}Control")
            );
        }
    }
    let client = pool
        .get_message_by_name("newport.worker.ClientControl")
        .unwrap();
    let server = pool
        .get_message_by_name("newport.worker.ServerControl")
        .unwrap();
    assert!(
        client
            .fields()
            .all(|field| server.get_field(field.number()).is_none())
    );
}

#[cfg(feature = "trace")]
#[test]
fn worker_trace_shows_lengths_without_exposing_external_bytes() {
    for (message, wire) in [
        (
            "newport.worker.OpaqueClientFrame",
            encode_opaque(
                &Chunk::new(Bytes::from_static(b"private external bytes")).unwrap(),
                Direction::ToWorker,
            )
            .unwrap(),
        ),
        (
            "newport.worker.OpaqueServerFrame",
            encode_server(&ServerControl::Stderr(
                Chunk::new(Bytes::from_static(b"private diagnostic")).unwrap(),
            ))
            .unwrap(),
        ),
    ] {
        let mut output = Vec::new();
        protobuf::trace::trace(
            protobuf::DESCRIPTORS,
            message,
            &mut wire.as_slice(),
            &mut output,
        )
        .unwrap();
        let output = String::from_utf8(output).unwrap();
        assert!(!output.contains("private"));
        let json: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert!(
            json["data"].is_number() || json["workerControl"]["stderr"].is_number(),
            "{output}"
        );
    }
}

fn body(frame: &[u8]) -> Bytes {
    protobuf::read_frame(&mut &*frame, OPAQUE_LIMITS.message)
        .unwrap()
        .unwrap()
}

fn bytes_field(number: u32, data: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    prost::encoding::encode_varint(u64::from(number) << 3 | 2, &mut bytes);
    prost::encoding::encode_varint(data.len() as u64, &mut bytes);
    bytes.extend_from_slice(data);
    bytes
}

fn integer(number: u32, value: u64) -> Vec<u8> {
    let mut bytes = Vec::new();
    prost::encoding::encode_varint(u64::from(number) << 3, &mut bytes);
    prost::encoding::encode_varint(value, &mut bytes);
    bytes
}

fn client() -> Bytes {
    body(&encode_client(ClientControl::InputComplete).unwrap())
}

fn server() -> Bytes {
    body(&encode_server(&ServerControl::Exit(Exit::Status(0))).unwrap())
}

#[test]
fn controls_round_trip_with_present_zero_exit_status() {
    for control in [
        ClientControl::InputComplete,
        ClientControl::Signal(Signal::new(64).unwrap()),
    ] {
        assert_eq!(
            decode_client(body(&encode_client(control).unwrap()), &CONTROL_LIMITS).unwrap(),
            Frame::Control(control)
        );
    }
    for control in [
        ServerControl::Exit(Exit::Status(0)),
        ServerControl::Exit(Exit::Status(u32::MAX)),
        ServerControl::Exit(Exit::Signal(Signal::new(15).unwrap())),
        ServerControl::Stderr(Chunk::new(vec![255; MAX_CHUNK].into()).unwrap()),
    ] {
        assert_eq!(
            decode_server(body(&encode_server(&control).unwrap()), &CONTROL_LIMITS).unwrap(),
            Frame::Control(control)
        );
    }
}

#[test]
fn service_frames_are_unchanged_and_controls_inside_payloads_are_data() {
    let mut raw = bytes_field(1, &client());
    raw.extend(integer(500, u64::MAX));
    let raw = Bytes::from(raw);
    for result in [
        decode_client(raw.clone(), &OPAQUE_LIMITS).map(|frame| match frame {
            Frame::Service(data) => data,
            _ => panic!("service"),
        }),
        decode_server(raw.clone(), &OPAQUE_LIMITS).map(|frame| match frame {
            Frame::Service(data) => data,
            _ => panic!("service"),
        }),
    ] {
        let frame = result.unwrap();
        assert_eq!(frame, raw);
        assert_eq!(frame.as_ptr(), raw.as_ptr());
    }
    reject_controls(&raw, raw.len()).unwrap();
}

#[test]
fn every_mixed_control_order_is_refused_including_unknown_fields() {
    for control in [client(), server()] {
        for service in [
            bytes_field(1, b"data"),
            integer(900, 0),
            bytes_field(500, b""),
        ] {
            for raw in [
                [control.as_ref(), &service].concat(),
                [&service, control.as_ref()].concat(),
            ] {
                assert!(matches!(
                    decode_client(raw.clone().into(), &OPAQUE_LIMITS),
                    Err(Error::MixedControl)
                ));
                assert!(matches!(
                    decode_server(raw.clone().into(), &OPAQUE_LIMITS),
                    Err(Error::MixedControl)
                ));
                assert!(reject_controls(&raw, OPAQUE_LIMITS.message).is_err());
            }
        }
    }
}

#[test]
fn duplicate_controls_and_operations_cannot_use_last_one_wins() {
    for first in [client(), server()] {
        for second in [client(), server()] {
            let raw = [first.as_ref(), second.as_ref()].concat();
            assert!(matches!(
                decode_client(raw.clone().into(), &OPAQUE_LIMITS),
                Err(Error::DuplicateControl)
            ));
            assert!(matches!(
                decode_server(raw.into(), &OPAQUE_LIMITS),
                Err(Error::DuplicateControl)
            ));
        }
    }
    let operations = [
        bytes_field(1, b""),
        integer(2, 15),
        bytes_field(3, b"stderr"),
        bytes_field(4, &integer(1, 0)),
    ];
    for first in &operations {
        for second in &operations {
            let raw = bytes_field(
                CONTROL_FIELD,
                &[first.as_slice(), second.as_slice()].concat(),
            );
            assert!(matches!(
                decode_client(raw.clone().into(), &OPAQUE_LIMITS),
                Err(Error::InvalidOperation)
            ));
            assert!(matches!(
                decode_server(raw.into(), &OPAQUE_LIMITS),
                Err(Error::InvalidOperation)
            ));
        }
    }
    let exit = [integer(1, 0), integer(2, 9)].concat();
    let raw = bytes_field(CONTROL_FIELD, &bytes_field(4, &exit));
    assert!(matches!(
        decode_server(raw.into(), &OPAQUE_LIMITS),
        Err(Error::InvalidOperation)
    ));
}

#[test]
fn controls_have_disjoint_directions_and_cannot_reach_service_endpoints() {
    assert!(matches!(
        decode_server(client(), &CONTROL_LIMITS),
        Err(Error::WrongDirection)
    ));
    assert!(matches!(
        decode_client(server(), &CONTROL_LIMITS),
        Err(Error::WrongDirection)
    ));
    for raw in [client(), server()] {
        assert!(matches!(
            reject_controls(&raw, CONTROL_LIMITS.message),
            Err(Error::UnexpectedControl)
        ));
    }
}

#[test]
fn malformed_fields_cannot_hide_a_control() {
    let malformed = [
        vec![0],
        vec![1; 9],
        vec![0x0a, 0xff],
        vec![0x08],
        vec![0x0b],
        vec![0x0c],
        vec![0x0e],
        vec![0x0f],
        vec![0x0d, 1, 2, 3],
        vec![0x09, 1, 2, 3, 4, 5, 6, 7],
        vec![0xff; 11],
        vec![
            0x08, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 2,
        ],
        bytes_field(1 << 29, b""),
        integer(CONTROL_FIELD, 0),
        bytes_field(0, b""),
    ];
    for bad in malformed {
        for raw in [
            bad.clone(),
            [client().as_ref(), &bad].concat(),
            [&bad, client().as_ref()].concat(),
        ] {
            assert!(
                decode_client(raw.clone().into(), &OPAQUE_LIMITS).is_err(),
                "{raw:?}"
            );
            assert!(decode_server(raw.into(), &OPAQUE_LIMITS).is_err());
        }
    }
}

#[test]
fn absent_unknown_and_malformed_control_operations_are_refused() {
    for operation in [
        vec![],
        bytes_field(1, &[8, 1]),
        bytes_field(2, b""),
        integer(50, 0),
    ] {
        assert!(
            decode_client(
                bytes_field(CONTROL_FIELD, &operation).into(),
                &CONTROL_LIMITS
            )
            .is_err()
        );
    }
    for operation in [
        vec![],
        bytes_field(3, b""),
        integer(3, 1),
        bytes_field(4, b""),
        bytes_field(4, &integer(50, 0)),
    ] {
        assert!(
            decode_server(
                bytes_field(CONTROL_FIELD, &operation).into(),
                &CONTROL_LIMITS
            )
            .is_err()
        );
    }
    for value in [0, 65, u64::from(u32::MAX) + 1, u64::MAX] {
        assert!(
            decode_client(
                bytes_field(CONTROL_FIELD, &integer(2, value)).into(),
                &CONTROL_LIMITS
            )
            .is_err()
        );
        assert!(
            decode_server(
                bytes_field(CONTROL_FIELD, &bytes_field(4, &integer(2, value))).into(),
                &CONTROL_LIMITS
            )
            .is_err()
        );
    }
    assert!(
        decode_server(
            bytes_field(CONTROL_FIELD, &bytes_field(4, &integer(1, u64::MAX))).into(),
            &CONTROL_LIMITS
        )
        .is_err()
    );
}

#[test]
fn chunk_and_metadata_limits_apply_to_controls() {
    let raw = bytes_field(CONTROL_FIELD, &bytes_field(3, &vec![0; MAX_CHUNK + 1]));
    assert!(matches!(
        decode_server(raw.into(), &CONTROL_LIMITS),
        Err(Error::Chunk(_))
    ));
    let raw = server();
    assert!(matches!(
        decode_server(
            raw.clone(),
            &Limits {
                message: raw.len() - 1,
                metadata: 64
            }
        ),
        Err(Error::Frame(FrameError::TooLarge { .. }))
    ));
    assert!(matches!(
        decode_server(
            raw,
            &Limits {
                message: CONTROL_LIMITS.message,
                metadata: 0
            }
        ),
        Err(Error::Frame(FrameError::MetadataTooLarge { .. }))
    ));
}

#[test]
fn stderr_and_service_diagnostics_never_dump_contents() {
    let frame = body(
        &encode_server(&ServerControl::Stderr(
            Chunk::new(Bytes::from_static(b"private diagnostic")).unwrap(),
        ))
        .unwrap(),
    );
    let decoded = decode_server(frame.clone(), &CONTROL_LIMITS).unwrap();
    assert!(!format!("{decoded:?}").contains("private"));
    let Frame::Control(ServerControl::Stderr(chunk)) = decoded else {
        panic!("stderr")
    };
    assert_eq!(
        chunk.data().as_ptr(),
        frame[frame.len() - chunk.data().len()..].as_ptr()
    );
    let service: Frame<ClientControl> = Frame::Service(Bytes::from_static(b"private service"));
    assert!(!format!("{service:?}").contains("private"));
}

#[test]
fn stdin_completion_allows_signals_but_no_more_data_or_completion() {
    let signal = body(&encode_client(ClientControl::Signal(Signal::new(15).unwrap())).unwrap());
    for next in [client(), Bytes::from(bytes_field(1, b"input"))] {
        let mut decoder = ClientDecoder::default();
        decoder.decode(client(), &CONTROL_LIMITS).unwrap();
        decoder.decode(signal.clone(), &CONTROL_LIMITS).unwrap();
        assert!(matches!(
            decoder.decode(next, &CONTROL_LIMITS),
            Err(Error::InputComplete)
        ));
        assert!(matches!(
            decoder.decode(signal.clone(), &CONTROL_LIMITS),
            Err(Error::Ended)
        ));
    }
}

#[test]
fn exit_is_final_and_errors_end_both_directions() {
    let stderr = body(
        &encode_server(&ServerControl::Stderr(
            Chunk::new(Bytes::from_static(b"last diagnostic")).unwrap(),
        ))
        .unwrap(),
    );
    for next in [
        server(),
        stderr.clone(),
        Bytes::from(bytes_field(1, b"output")),
    ] {
        let mut decoder = ServerDecoder::default();
        decoder.decode(stderr.clone(), &CONTROL_LIMITS).unwrap();
        decoder.decode(server(), &CONTROL_LIMITS).unwrap();
        assert!(matches!(
            decoder.decode(next, &CONTROL_LIMITS),
            Err(Error::Ended)
        ));
    }
    let mut client_decoder = ClientDecoder::default();
    let mut server_decoder = ServerDecoder::default();
    assert!(
        client_decoder
            .decode(Bytes::from_static(&[0]), &CONTROL_LIMITS)
            .is_err()
    );
    assert!(
        server_decoder
            .decode(Bytes::from_static(&[0]), &CONTROL_LIMITS)
            .is_err()
    );
    assert!(matches!(
        client_decoder.decode(client(), &CONTROL_LIMITS),
        Err(Error::Ended)
    ));
    assert!(matches!(
        server_decoder.decode(server(), &CONTROL_LIMITS),
        Err(Error::Ended)
    ));
}

#[test]
fn opaque_chunks_reassemble_external_headers_and_binary_bytes_exactly() {
    let input = b"Content-Length: 5\r\n\r\n\0\xff\r\nX";
    for split in 1..input.len() {
        for direction in [Direction::ToWorker, Direction::FromWorker] {
            let mut output = Vec::new();
            for bytes in [&input[..split], &input[split..]] {
                let chunk = Chunk::new(Bytes::copy_from_slice(bytes)).unwrap();
                let raw = body(&encode_opaque(&chunk, direction).unwrap());
                let decoded = decode_opaque(raw.clone()).unwrap();
                assert_eq!(
                    decoded.data().as_ptr(),
                    raw[raw.len() - bytes.len()..].as_ptr()
                );
                output.extend_from_slice(decoded.data());
            }
            assert_eq!(output, input);
        }
    }
}

#[test]
fn opaque_chunks_cannot_hide_extra_data_or_controls() {
    for raw in [
        client(),
        server(),
        Bytes::new(),
        bytes_field(1, b"").into(),
        bytes_field(2, b"data").into(),
        [bytes_field(1, b"one"), bytes_field(1, b"two")]
            .concat()
            .into(),
        bytes_field(1, &vec![0; MAX_CHUNK + 1]).into(),
    ] {
        assert!(decode_opaque(raw).is_err());
    }
}

#[test]
fn controls_match_generated_messages_in_both_directions() {
    let raw = client();
    let generated = pb::OpaqueClientFrame::decode(raw).unwrap();
    assert!(matches!(
        generated.message,
        Some(pb::opaque_client_frame::Message::WorkerControl(
            pb::ClientControl {
                operation: Some(pb::client_control::Operation::InputComplete(_))
            }
        ))
    ));
    let raw = pb::OpaqueServerFrame {
        message: Some(pb::opaque_server_frame::Message::WorkerControl(
            pb::ServerControl {
                operation: Some(pb::server_control::Operation::Exit(pb::Exit {
                    outcome: Some(pb::exit::Outcome::Status(0)),
                })),
            },
        )),
    }
    .encode_to_vec();
    assert_eq!(
        decode_server(raw.into(), &CONTROL_LIMITS).unwrap(),
        Frame::Control(ServerControl::Exit(Exit::Status(0)))
    );
}

#[tokio::test]
async fn fragmented_frames_keep_controls_separate_from_service_bytes() {
    use tokio::io::AsyncWriteExt;
    let (mut writer, reader) = tokio::io::duplex(1);
    let chunk = Chunk::new(Bytes::from_static(b"Content-Length: 2\r\n\r\n{}")).unwrap();
    let wire = [
        encode_opaque(&chunk, Direction::FromWorker).unwrap(),
        encode_server(&ServerControl::Exit(Exit::Status(0))).unwrap(),
    ]
    .concat();
    let task = tokio::spawn(async move {
        for byte in wire {
            writer.write_all(&[byte]).await.unwrap();
        }
    });
    let mut reader = protobuf::tokio::FrameReader::new(reader, OPAQUE_LIMITS);
    let mut decoder = ServerDecoder::default();
    let Frame::Service(raw) = decoder
        .decode(reader.next().await.unwrap().unwrap(), &OPAQUE_LIMITS)
        .unwrap()
    else {
        panic!("data")
    };
    assert_eq!(decode_opaque(raw).unwrap(), chunk);
    assert_eq!(
        decoder
            .decode(reader.next().await.unwrap().unwrap(), &OPAQUE_LIMITS)
            .unwrap(),
        Frame::Control(ServerControl::Exit(Exit::Status(0)))
    );
    assert!(reader.next().await.unwrap().is_none());
    task.await.unwrap();
}
