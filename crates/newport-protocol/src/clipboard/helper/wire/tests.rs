use super::*;
fn roundtrip<T: Message + PartialEq + std::fmt::Debug>(message: T, data: &[u8]) {
    let encoded = encode(&message, data).unwrap();
    let body = protobuf::read_frame(&mut encoded.as_slice(), LIMITS.message)
        .unwrap()
        .unwrap();
    let frame = decode::<T>(body).unwrap();
    assert_eq!(frame.header, message);
    assert_eq!(frame.bytes, data);
    for end in [1, encoded.len() / 2, encoded.len() - 1] {
        if end > 0 {
            assert!(protobuf::read_frame(&mut &encoded[..end], LIMITS.message).is_err());
        }
    }
}
#[test]
fn every_direction_and_optional_revision_roundtrips() {
    let commands = [
        Command::Hello {
            version: VERSION,
            board: Board::General {},
        },
        Command::Hello {
            version: VERSION,
            board: Board::Test {
                name: "fixture".into(),
            },
        },
        Command::Publish {
            generation: 1,
            expected_revision: i64::MIN,
            formats: vec!["text/plain".into()],
        },
        Command::AttachFiles {
            generation: 1,
            expected_revision: i64::MAX,
            folder: "folder".into(),
        },
        Command::Revoke { generation: 1 },
        Command::Data {
            generation: 1,
            id: 1,
            offset: 0,
            eof: true,
        },
        Command::Failed {
            generation: 1,
            id: 1,
        },
        Command::ReadLocal {
            id: 1,
            revision: -1,
            format: "text/plain".into(),
        },
        Command::CancelLocal { id: 1 },
        Command::Stop {},
    ];
    for value in commands {
        roundtrip(value, &[]);
    }
    let events = [
        Event::Ready { version: VERSION },
        Event::Snapshot {
            revision: -1,
            formats: vec![],
            origin: None,
        },
        Event::Published {
            generation: 1,
            revision: i64::MAX,
        },
        Event::Attached {
            generation: 1,
            revision: None,
        },
        Event::Attached {
            generation: 1,
            revision: Some(0),
        },
        Event::Rejected { generation: 1 },
        Event::Revoked {
            generation: 1,
            revision: Some(i64::MIN),
        },
        Event::Read {
            generation: 1,
            id: 1,
            format: "text/plain".into(),
        },
        Event::Cancelled {
            generation: 1,
            id: 1,
        },
        Event::LocalData {
            id: 1,
            revision: -1,
            offset: 0,
            eof: true,
        },
        Event::LocalFailed { id: 1 },
    ];
    for value in events {
        roundtrip(value, &[]);
    }
    assert_eq!(encode(&Command::Stop {}, &[]).unwrap(), [2, 74, 0]);
}
#[test]
fn maximum_chunks_and_offsets_preserve_native_payload_capacity() {
    let data = vec![42; CHUNK];
    roundtrip(
        Command::Data {
            generation: 1,
            id: 1,
            offset: (MAX_BYTES - CHUNK) as u64,
            eof: true,
        },
        &data,
    );
    roundtrip(
        Event::LocalData {
            id: 1,
            revision: -1,
            offset: 0,
            eof: false,
        },
        &data,
    );
    assert!(
        encode(
            &Command::Data {
                generation: 1,
                id: 1,
                offset: u64::MAX,
                eof: true
            },
            &[1]
        )
        .is_err()
    );
    assert!(
        encode(
            &Event::LocalData {
                id: 1,
                revision: 0,
                offset: 0,
                eof: false
            },
            &[]
        )
        .is_err()
    );
    assert!(
        encode(
            &Command::Data {
                generation: 1,
                id: 1,
                offset: 0,
                eof: true
            },
            &vec![0; CHUNK + 1]
        )
        .is_err()
    );
    assert!(encode(&Command::Stop {}, b"secret").is_err());
}
#[test]
fn duplicate_oneofs_scalar_fields_boolean_and_unknown_operations_fail() {
    for body in [
        &[74, 0, 74, 0][..],
        &[82, 0],
        &[42, 4, 32, 1, 32, 0],
        &[42, 2, 32, 2],
    ] {
        assert!(decode::<Command>(Bytes::copy_from_slice(body)).is_err());
    }
    assert!(decode::<Event>(Bytes::from_static(&[8, 1])).is_err());
    assert!(matches!(
        decode::<Command>(Bytes::from_static(&[74, 2, 8, 1]))
            .unwrap()
            .header,
        Command::Stop {}
    ));
}
#[test]
fn incremental_frames_share_the_standard_framing_contract() {
    let encoded = encode(
        &Event::LocalData {
            id: 9,
            revision: -1,
            offset: 0,
            eof: true,
        },
        b"abc",
    )
    .unwrap();
    let mut decoder = protobuf::FrameDecoder::new(LIMITS.message);
    let mut result = None;
    for byte in encoded {
        let part = [byte];
        if let Some(frame) = decoder.consume(&mut part.as_slice()).unwrap() {
            result = Some(decode::<Event>(frame.body()).unwrap());
        }
    }
    decoder.finish().unwrap();
    let frame = result.unwrap();
    assert_eq!(frame.bytes, b"abc");
    assert!(matches!(
        frame.header,
        Event::LocalData {
            id: 9,
            revision: -1,
            ..
        }
    ));
}
