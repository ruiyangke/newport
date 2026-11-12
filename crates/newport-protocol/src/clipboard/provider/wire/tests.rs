use super::*;
fn id() -> Identity {
    Identity {
        sequence: 1,
        owner: u64::MAX,
    }
}
fn roundtrip(packet: Packet) {
    let expected = packet.clone();
    let bytes = encode(packet).unwrap();
    let body = protobuf::read_frame(&mut bytes.as_slice(), LIMITS.message)
        .unwrap()
        .unwrap();
    assert_eq!(decode(body).unwrap(), expected);
}
#[test]
fn every_operation_and_native_receipt_roundtrips() {
    let nonce = "10000000-0000-4000-8000-000000000001".to_owned();
    let messages = [
        Message::Hello {
            version: VERSION,
            nonce: nonce.clone(),
        },
        Message::Ready {
            version: VERSION,
            nonce,
        },
        Message::Snapshot { id: 1 },
        Message::Read {
            id: 2,
            revision: i64::MIN,
            format: "text/plain".into(),
        },
        Message::Publish {
            id: 3,
            expected: i64::MAX,
            formats: vec![],
        },
        Message::Attach {
            id: 4,
            generation: 3,
            folder: NativePath(vec![67, 58, 92, 0xd800, 0xdcff]),
        },
        Message::Done {
            id: 5,
            outcome: None,
        },
        Message::Admit {
            id: 6,
            source: 3,
            preparation: true,
        },
        Message::Admitted {
            id: 6,
            accepted: false,
        },
        Message::Demand {
            id: 7,
            source: 3,
            format: "image/png".into(),
        },
        Message::Data {
            id: 7,
            present: false,
        },
        Message::Data {
            id: 8,
            present: true,
        },
        Message::Consumed { id: 8 },
        Message::Revoke { source: 3 },
        Message::Revoked { source: 3 },
        Message::Stop,
    ];
    for message in messages {
        roundtrip(message.into());
    }
    let outcomes = [
        Outcome::PublicationRejected(PublicationRejection {
            stage: PublicationStage::Empty,
            expected: i64::MIN,
            observed: Some(i64::MAX),
            native_error: Some(u32::MAX),
        }),
        Outcome::Snapshot {
            revision: -1,
            identity: id(),
            formats: vec!["text/plain".into()],
            origin: Some(3),
        },
        Outcome::Read {
            revision: 0,
            identity: id(),
        },
        Outcome::Published {
            generation: 1,
            owner: u64::MAX,
            preparation: None,
        },
        Outcome::Published {
            generation: 1,
            owner: u64::MAX,
            preparation: Some((-1, id())),
        },
        Outcome::Attached {
            result: FileWrite::Unchanged,
        },
        Outcome::Attached {
            result: FileWrite::Uncertain,
        },
        Outcome::Attached {
            result: FileWrite::Committed {
                revision: i64::MIN,
                complete: false,
            },
        },
    ];
    for outcome in outcomes {
        roundtrip(
            Message::Done {
                id: 1,
                outcome: Some(outcome),
            }
            .into(),
        );
    }
    assert_eq!(encode(Message::Stop.into()).unwrap(), [2, 122, 0]);
}
#[test]
fn absent_empty_and_maximum_payloads_are_distinct_and_bounded() {
    roundtrip(Packet {
        message: Message::Data {
            id: 1,
            present: true,
        },
        data: vec![42; MAX_BYTES],
    });
    roundtrip(Packet {
        message: Message::Done {
            id: 2,
            outcome: Some(Outcome::Read {
                revision: 0,
                identity: id(),
            }),
        },
        data: vec![],
    });
    assert!(
        encode(Packet {
            message: Message::Data {
                id: 1,
                present: false
            },
            data: vec![1]
        })
        .is_err()
    );
    assert!(
        encode(Packet {
            message: Message::Data {
                id: 1,
                present: true
            },
            data: vec![0; MAX_BYTES + 1]
        })
        .is_err()
    );
    assert!(
        encode(Packet {
            message: Message::Done {
                id: 1,
                outcome: None
            },
            data: vec![1]
        })
        .is_err()
    );
}
#[test]
fn native_path_units_are_lossless_and_odd_wire_lengths_are_rejected() {
    let units = vec![67, 58, 92, 0xd800, 0xdcff, 0xdfff];
    assert_eq!(
        read_path(path(&NativePath(units.clone()))).unwrap().0,
        units
    );
    for bytes in [vec![65], vec![0, 0], vec![]] {
        assert!(read_path(bytes.into()).is_err());
    }
    assert!(NativePath(vec![1; MAX_PATH_UNITS + 1]).validate().is_err());
}
#[test]
fn ambiguous_fields_unknown_operations_and_invalid_domains_fail_before_use() {
    for body in [
        &[122, 0, 122, 0][..],
        &[130, 1, 0],
        &[74, 4, 16, 0, 16, 1],
        &[74, 2, 16, 2],
    ] {
        assert!(decode(Bytes::copy_from_slice(body)).is_err());
    }
    assert!(decode(Bytes::from_static(&[122, 2, 8, 1])).is_ok());
    assert!(
        encode(
            Message::Hello {
                version: VERSION,
                nonce: "not-a-nonce".into()
            }
            .into()
        )
        .is_err()
    );
    assert!(
        encode(
            Message::Publish {
                id: 1,
                expected: -1,
                formats: vec!["text/plain".into(); MAX_FORMATS + 1]
            }
            .into()
        )
        .is_err()
    );
}
#[test]
fn nonce_and_payload_debug_and_traces_are_redacted() {
    let nonce = "10000000-0000-4000-8000-000000000001".to_owned();
    let packet = Packet::from(Message::Hello {
        version: VERSION,
        nonce: nonce.clone(),
    });
    assert!(!format!("{packet:?}").contains(&nonce));
    #[cfg(feature = "trace")]
    {
        let mut output = Vec::new();
        protobuf::trace::trace(
            protobuf::DESCRIPTORS,
            "newport.clipboard.provider.Frame",
            &mut encode(packet).unwrap().as_slice(),
            &mut output,
        )
        .unwrap();
        assert!(!String::from_utf8(output).unwrap().contains(&nonce));
    }
}
