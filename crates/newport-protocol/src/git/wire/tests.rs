use super::*;
use crate::{git::*, worker};

const ID: &str = "11111111-1111-4111-8111-111111111111";
const OTHER: &str = "22222222-2222-4222-8222-222222222222";

fn body<T: Message>(message: &T) -> Vec<u8> {
    let frame = message.encode().unwrap();
    protobuf::read_frame(&mut frame.as_slice(), MAX_FRAME)
        .unwrap()
        .unwrap()
        .to_vec()
}
fn field(number: u32, value: &[u8]) -> Vec<u8> {
    let mut bytes = vec![];
    prost::encoding::encode_varint((u64::from(number) << 3) | 2, &mut bytes);
    prost::encoding::encode_varint(value.len() as u64, &mut bytes);
    bytes.extend_from_slice(value);
    bytes
}
fn scalar(number: u32, value: u64) -> Vec<u8> {
    let mut bytes = vec![];
    prost::encoding::encode_varint(u64::from(number) << 3, &mut bytes);
    prost::encoding::encode_varint(value, &mut bytes);
    bytes
}

#[test]
fn configuration_responses_logs_and_flow_control_round_trip() {
    let clients = [
        ClientMessage::Configure(Configure {
            id: ID.into(),
            client_id: OTHER.into(),
            client_version: "test".into(),
            command_logs: false,
        }),
        ClientMessage::Call(Call {
            id: ID.into(),
            request: Box::new(Request::Open {
                path: Path::new(b"/repository-\xff"),
            }),
        }),
        ClientMessage::Ping("ping".into()),
        ClientMessage::Ack(Ack {
            stream_id: ID.into(),
            seq: 1,
        }),
    ];
    for value in clients {
        assert_eq!(
            read::<ClientMessage>(&mut value.encode().unwrap().as_slice()).unwrap(),
            value
        );
        assert!(ServerMessage::decode(Bytes::from(body(&value))).is_err());
    }
    let servers = [
        ServerMessage::Ready(Ready {
            id: ID.into(),
            limits: crate::git::Limits::default(),
            capabilities: Capabilities {
                methods: vec!["repo.open".into()],
                object_formats: vec!["sha1".into(), "sha256".into()],
                features: vec![],
                actions: vec![],
            },
        }),
        ServerMessage::Response(Response {
            id: ID.into(),
            result: Ok(Reply::Close(Box::new(Closed { closed: true }))),
        }),
        ServerMessage::Response(Response {
            id: ID.into(),
            result: Err(crate::git::Error::new(
                "OUTCOME_UNKNOWN",
                "Query the existing operation.",
            )),
        }),
        ServerMessage::Pong("ping".into()),
        ServerMessage::Begin(Begin {
            id: ID.into(),
            stream_id: OTHER.into(),
            snapshot: "snapshot".into(),
        }),
        ServerMessage::Chunk(Chunk {
            id: ID.into(),
            stream_id: OTHER.into(),
            seq: 1,
            data: Binary(vec![0, 255]),
        }),
        ServerMessage::Log(Log {
            id: ID.into(),
            entry: CommandLog {
                command: "git status".into(),
                duration_ms: u64::MAX,
                exit_code: Some(-1),
                output: "failed".into(),
                interrupted: false,
            },
        }),
        ServerMessage::Failed(crate::git::Error::new("GIT_ERROR", "Git unavailable.")),
    ];
    for value in servers {
        assert_eq!(
            read::<ServerMessage>(&mut value.encode().unwrap().as_slice()).unwrap(),
            value
        );
        assert!(ClientMessage::decode(Bytes::from(body(&value))).is_err());
    }
}

#[test]
fn optional_lists_and_defaults_preserve_durable_request_serialization() {
    for lines in [None, Some(vec![]), Some(vec!["line".into()])] {
        let value = ClientMessage::Call(Call {
            id: ID.into(),
            request: Box::new(Request::Start {
                operation_id: OTHER.into(),
                repo_id: "repo".into(),
                expected_snapshot: "snapshot".into(),
                action: Action::Stage {
                    entry_ids: vec!["entry".into()],
                    hunks: Some(HunkSelection {
                        ids: vec!["hunk".into()],
                        lines,
                        context_lines: 0,
                    }),
                },
            }),
        });
        let result = ClientMessage::decode(Bytes::from(body(&value))).unwrap();
        assert_eq!(value, result);
        let (ClientMessage::Call(before), ClientMessage::Call(after)) = (&value, result) else {
            panic!()
        };
        assert_eq!(
            serde_json::to_vec(&before.request).unwrap(),
            serde_json::to_vec(&after.request).unwrap()
        );
    }
}

#[test]
fn preflight_rejects_duplicate_known_fields_bad_values_and_ambiguous_operations() {
    let value = ClientMessage::Configure(Configure {
        id: ID.into(),
        client_id: OTHER.into(),
        client_version: "test".into(),
        command_logs: false,
    });
    let encoded = body(&value);
    let raw: pb::ClientFrame = ProstMessage::decode(encoded.as_slice()).unwrap();
    let Some(pb::client_frame::Message::Configure(config)) = raw.message else {
        panic!()
    };
    let original = config.encode_to_vec();
    for suffix in [
        field(1, ID.as_bytes()),
        scalar(4, 2),
        scalar(4, 1_u64 << 32),
    ] {
        let mut nested = original.clone();
        nested.extend(suffix);
        assert!(ClientMessage::decode(field(1, &nested).into()).is_err());
    }
    let invalid_bool = [
        field(1, ID.as_bytes()),
        field(2, OTHER.as_bytes()),
        field(3, b"test"),
        scalar(4, 2),
    ]
    .concat();
    assert!(ClientMessage::decode(field(1, &invalid_bool).into()).is_err());
    let mut duplicate = encoded.clone();
    duplicate.extend_from_slice(&encoded);
    assert!(ClientMessage::decode(duplicate.into()).is_err());
    let mut ambiguous = encoded;
    ambiguous.extend(field(3, b"ping"));
    assert!(ClientMessage::decode(ambiguous.into()).is_err());
    // Required false and empty fields still have presence on the transport.
    let missing = [
        field(1, ID.as_bytes()),
        field(2, OTHER.as_bytes()),
        field(3, b"test"),
    ]
    .concat();
    assert!(ClientMessage::decode(field(1, &missing).into()).is_err());
}

#[test]
fn preflight_bounds_collections_and_rejects_worker_controls() {
    let methods = field(1, b"repo.open").repeat(10_001);
    let ready = [field(1, ID.as_bytes()), field(3, &methods)].concat();
    assert!(ServerMessage::decode(field(17, &ready).into()).is_err());
    let chunk = [
        field(1, ID.as_bytes()),
        field(2, OTHER.as_bytes()),
        scalar(3, 1),
        field(4, &vec![0; CHUNK_SIZE + 1]),
    ]
    .concat();
    assert!(ServerMessage::decode(field(21, &chunk).into()).is_err());
    let control = worker::wire::encode_client(worker::ClientControl::InputComplete).unwrap();
    let raw = protobuf::read_frame(&mut control.as_slice(), MAX_FRAME)
        .unwrap()
        .unwrap();
    assert!(ClientMessage::decode(raw.clone()).is_err());
    let mut mixed = body(&ClientMessage::Ping("ping".into()));
    mixed.extend_from_slice(&raw);
    assert!(ClientMessage::decode(mixed.into()).is_err());
}

#[test]
fn unspecified_and_unknown_enums_do_not_select_a_git_operation() {
    let value = ClientMessage::Call(Call {
        id: ID.into(),
        request: Box::new(Request::Diff {
            repo_id: "repo".into(),
            snapshot: "snapshot".into(),
            entry_id: "entry".into(),
            side: Side::HeadToIndex,
            context_lines: 3,
        }),
    });
    let original: pb::ClientFrame = ProstMessage::decode(body(&value).as_slice()).unwrap();
    for side in [0, -1, i32::MAX] {
        let mut raw = original.clone();
        let Some(pb::client_frame::Message::Call(call)) = &mut raw.message else {
            panic!()
        };
        let Some(pb::request::Value::Diff(diff)) = &mut call.request.as_mut().unwrap().value else {
            panic!()
        };
        diff.side = Some(side);
        assert!(ClientMessage::decode(raw.encode_to_vec().into()).is_err());
    }
}

#[test]
fn framing_bounds_allocation_and_retains_empty_data_presence() {
    let original = ServerMessage::Response(Response {
        id: ID.into(),
        result: Ok(Reply::Blob(Box::new(Blob {
            oid: ObjectId {
                algorithm: "sha1".into(),
                hex: "1".repeat(40),
            },
            size: 0,
            bytes_b64: Some(Binary(vec![])),
            truncated: false,
        }))),
    });
    let mut frame = original.encode().unwrap();
    assert_eq!(
        read::<ServerMessage>(&mut frame.as_slice()).unwrap(),
        original
    );
    frame.pop();
    assert!(read::<ServerMessage>(&mut frame.as_slice()).is_err());
    let mut oversized = vec![];
    prost::encoding::encode_varint(MAX_FRAME as u64 + 1, &mut oversized);
    assert!(matches!(
        read::<ServerMessage>(&mut oversized.as_slice()),
        Err(super::Error::Frame(FrameError::TooLarge { .. }))
    ));
    let mut forward = body(&ClientMessage::Ping("ping".into()));
    forward.extend(field(100, b"future metadata"));
    assert_eq!(
        ClientMessage::decode(forward.into()).unwrap(),
        ClientMessage::Ping("ping".into())
    );
    assert_eq!(
        ClientMessage::Ping("ping".into()).encode().unwrap(),
        [6, 26, 4, b'p', b'i', b'n', b'g']
    );
}
