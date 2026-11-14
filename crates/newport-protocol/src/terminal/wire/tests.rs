use super::*;
use prost::Message;

fn computer() -> Computer {
    Computer {
        id: "11111111-1111-4111-8111-111111111111".into(),
        name: "Desktop".into(),
    }
}
fn holder() -> Holder {
    Holder {
        id: "22222222222242228222222222222222".into(),
        socket: "/run/newport/terminal.sock".into(),
        version: 1,
        columns: 80,
        rows: 24,
        started: 7,
        name: None,
        title: Some(String::new()),
        agent: Some("state=blocked:app=claude-code:kind=permission".into()),
        creator: computer(),
        attached: vec![Attached {
            id: computer().id,
            name: "Laptop".into(),
            active: true,
        }],
        exited: Some(Exit::Code(0)),
    }
}
fn start() -> Request {
    Request::Start {
        id: holder().id,
        columns: 80,
        rows: 24,
        command: "private shell command".into(),
        creator: computer(),
        stable_display: true,
    }
}

#[test]
fn operations_and_results_round_trip_with_optional_fields_and_zero_outcomes() {
    for (request, reply) in [
        (
            start(),
            Reply::Started {
                socket: holder().socket,
            },
        ),
        (Request::List, Reply::Listed(vec![holder()])),
        (Request::Watch, Reply::Listed(Vec::new())),
        (
            Request::Rename {
                id: holder().id,
                name: String::new(),
            },
            Reply::Renamed { renamed: false },
        ),
        (
            Request::End { id: holder().id },
            Reply::Ended { ended: true },
        ),
    ] {
        assert_eq!(
            read_request(&mut encode_request(&request).unwrap().as_slice()).unwrap(),
            request
        );
        assert_eq!(
            read_reply(&mut encode_reply(&reply).unwrap().as_slice()).unwrap(),
            reply
        );
        reply.validate_for(&request).unwrap();
    }
    assert!(!format!("{:?}", start()).contains("private shell"));
    let mut exited = holder();
    exited.exited = Some(Exit::Signal(15));
    let value = Reply::Listed(vec![exited]);
    assert_eq!(
        read_reply(&mut encode_reply(&value).unwrap().as_slice()).unwrap(),
        value
    );
    let failed = Reply::Failed(common::Error {
        code: "busy".into(),
        message: "Try again".into(),
        retry: common::Retry::Yes,
    });
    assert_eq!(
        read_reply(&mut encode_reply(&failed).unwrap().as_slice()).unwrap(),
        failed
    );
    assert!(
        Reply::Ended { ended: true }
            .validate_for(&Request::List)
            .is_err()
    );
}

#[test]
fn request_sizes_and_identities_are_checked_on_both_sides() {
    let Request::Start {
        mut id,
        columns,
        rows,
        command: _,
        creator,
        stable_display,
    } = start()
    else {
        unreachable!()
    };
    let mut command = "x".repeat(MAX_COMMAND);
    let mut value = Request::Start {
        id: id.clone(),
        columns,
        rows,
        command: command.clone(),
        creator: creator.clone(),
        stable_display,
    };
    assert_eq!(
        read_request(&mut encode_request(&value).unwrap().as_slice()).unwrap(),
        value
    );
    command.push('x');
    value = Request::Start {
        id: id.clone(),
        columns,
        rows,
        command,
        creator,
        stable_display,
    };
    assert!(encode_request(&value).is_err());
    id.push('/');
    assert!(encode_request(&Request::End { id }).is_err());
    for columns in [0, u32::from(u16::MAX) + 1] {
        let message = pb::ClientFrame {
            message: Some(pb::client_frame::Message::Start(pb::Start {
                id: holder().id,
                columns,
                rows: 24,
                command: "sh".into(),
                creator: Some(computer_to_wire(&computer())),
                stable_display: false,
            })),
        };
        assert!(decode_request(message.encode_to_vec().into()).is_err());
    }
    let mut invalid = holder();
    invalid.exited = Some(Exit::Signal(0));
    assert!(encode_reply(&Reply::Listed(vec![invalid])).is_err());
    assert!(encode_reply(&Reply::Listed(vec![holder(), holder()])).is_err());
}

#[test]
fn a_holders_agent_status_is_bounded_printable_report_text() {
    for agent in [
        "x".repeat(crate::terminal::MAX_AGENT + 1),
        "state=done:msg=\x1b[31m".into(),
        "state=done msg".into(),
    ] {
        let mut invalid = holder();
        invalid.agent = Some(agent);
        assert!(encode_reply(&Reply::Listed(vec![invalid])).is_err());
    }
    let frame = encode_reply(&Reply::Listed(vec![holder()])).unwrap();
    assert_eq!(
        read_reply(&mut frame.as_slice()).unwrap(),
        Reply::Listed(vec![holder()]),
        "the report travels unchanged"
    );
}

#[test]
fn a_frame_is_bounded_before_reading_or_decoding_its_body() {
    for limit in [CLIENT_LIMITS.message, SERVER_LIMITS.message] {
        let mut prefix = Vec::new();
        prost::encoding::encode_varint((limit + 1) as u64, &mut prefix);
        let result = if limit == CLIENT_LIMITS.message {
            read_request(&mut prefix.as_slice()).map(drop)
        } else {
            read_reply(&mut prefix.as_slice()).map(drop)
        };
        assert!(matches!(
            result,
            Err(Error::Frame(FrameError::TooLarge { .. }))
        ));
    }
    let frame = encode_reply(&Reply::Listed(vec![holder()])).unwrap();
    for cut in 1..frame.len() {
        assert!(read_reply(&mut &frame[..cut]).is_err());
    }
    struct Fragmented<'a>(&'a [u8]);
    impl Read for Fragmented<'_> {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            let length = bytes.len().min(1);
            self.0.read(&mut bytes[..length])
        }
    }
    assert_eq!(
        read_reply(&mut Fragmented(&frame)).unwrap(),
        Reply::Listed(vec![holder()])
    );
}

#[test]
fn oneofs_and_worker_controls_cannot_hide_operations_or_exit_outcomes() {
    let start = encode_request(&start()).unwrap();
    let start = protobuf::read_frame(&mut start.as_slice(), CLIENT_LIMITS.message)
        .unwrap()
        .unwrap();
    assert!(decode_request([start.as_ref(), start.as_ref()].concat().into()).is_err());
    let control =
        crate::worker::wire::encode_client(crate::worker::ClientControl::InputComplete).unwrap();
    let control = protobuf::read_frame(&mut control.as_slice(), CLIENT_LIMITS.message)
        .unwrap()
        .unwrap();
    for body in [
        control.to_vec(),
        [control.as_ref(), start.as_ref()].concat(),
        [start.as_ref(), control.as_ref()].concat(),
    ] {
        assert!(decode_request(body.into()).is_err());
    }
    let control = crate::worker::wire::encode_server(&crate::worker::ServerControl::Exit(
        crate::worker::Exit::Status(0),
    ))
    .unwrap();
    assert!(read_reply(&mut control.as_slice()).is_err());
    fn field(tag: u64, body: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        prost::encoding::encode_varint(tag << 3 | 2, &mut bytes);
        prost::encoding::encode_varint(body.len() as u64, &mut bytes);
        bytes.extend_from_slice(body);
        bytes
    }
    for duplicate in [false, true] {
        let mut record = holder_to_wire(&holder());
        if !duplicate {
            record.exited = None;
        }
        let mut body = record.encode_to_vec();
        body.extend(field(
            11,
            if duplicate {
                &[16, 15]
            } else {
                &[8, 0, 16, 15]
            },
        ));
        let frame = field(2, &field(1, &body));
        let error = decode_reply(frame.into()).unwrap_err();
        if duplicate {
            assert!(matches!(error, Error::Invalid(Invalid("duplicate exit"))));
        } else {
            assert!(matches!(error, Error::Frame(FrameError::Malformed)));
        }
    }
}

#[test]
fn repeated_records_are_counted_before_protobuf_allocates_them() {
    let frame = pb::ServerFrame {
        message: Some(pb::server_frame::Message::Listed(pb::Listed {
            holders: vec![pb::Holder::default(); MAX_HOLDERS + 1],
        })),
    };
    assert!(matches!(
        decode_reply(frame.encode_to_vec().into()),
        Err(Error::Invalid(Invalid("holder count")))
    ));
    let mut record = holder_to_wire(&holder());
    record.attached = vec![pb::Attached::default(); MAX_ATTACHED + 1];
    let frame = pb::ServerFrame {
        message: Some(pb::server_frame::Message::Listed(pb::Listed {
            holders: vec![record],
        })),
    };
    assert!(matches!(
        decode_reply(frame.encode_to_vec().into()),
        Err(Error::Invalid(Invalid("attachment count")))
    ));
}

#[test]
fn each_direction_reserves_the_shared_worker_control_field() {
    let descriptors = prost_types::FileDescriptorSet::decode(protobuf::DESCRIPTORS).unwrap();
    let schema = descriptors
        .file
        .iter()
        .find(|file| file.package.as_deref() == Some("newport.terminal"))
        .unwrap();
    for (name, control) in [
        ("ClientFrame", ".newport.worker.ClientControl"),
        ("ServerFrame", ".newport.worker.ServerControl"),
    ] {
        let message = schema
            .message_type
            .iter()
            .find(|message| message.name.as_deref() == Some(name))
            .unwrap();
        let field = message
            .field
            .iter()
            .find(|field| field.number == Some(127))
            .unwrap();
        assert_eq!(field.type_name.as_deref(), Some(control));
        assert_eq!(field.oneof_index, Some(0));
    }
}
