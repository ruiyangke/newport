use super::*;
const ID: &str = "0123456789abcdef0123456789abcdef";
fn attach() -> Attach {
    Attach {
        version: VERSION,
        columns: 80,
        rows: 24,
        control: false,
        opened: true,
        computer: Some(Computer {
            id: "10000000-0000-4000-8000-000000000001".into(),
            name: "Studio 工作室".into(),
        }),
        colors: Some([[1, 2, 3], [4, 5, 6], [7, 8, 9]]),
    }
}
#[test]
fn every_directional_message_round_trips_and_fragmentation_retains_bytes() {
    let clients = [
        ClientMessage::Attach(attach()),
        ClientMessage::Attach(Attach {
            control: true,
            opened: false,
            computer: None,
            ..attach()
        }),
        ClientMessage::Input(Bytes::from_static(&[0, 255])),
        ClientMessage::Resize {
            columns: 1,
            rows: u16::MAX,
        },
        ClientMessage::End,
        ClientMessage::Rename("build".into()),
        ClientMessage::Activate,
    ];
    for message in clients {
        let frame = encode_client(&message).unwrap();
        assert_eq!(read_client(&mut frame.as_slice()).unwrap(), message);
        for end in 0..frame.len() {
            assert!(read_client(&mut &frame[..end]).is_err());
        }
    }
    let servers = [
        ServerMessage::Hello(Hello {
            version: VERSION,
            terminal_id: ID.into(),
        }),
        ServerMessage::Attached { prompted: true },
        ServerMessage::Screen(Bytes::new()),
        ServerMessage::Full(Bytes::from_static(&[3, 0])),
        ServerMessage::Output(Bytes::from_static(&[0, 255])),
        ServerMessage::Exit(Exit::Code(0)),
        ServerMessage::Exit(Exit::Signal(9)),
        ServerMessage::Active(false),
        ServerMessage::Failed(reopen_required()),
        ServerMessage::Renamed("build".into()),
        ServerMessage::Renamed(String::new()),
    ];
    for message in servers {
        let frame = encode_server(&message).unwrap();
        assert_eq!(read_server(&mut frame.as_slice()).unwrap(), message);
        let mut decoder = protobuf::FrameDecoder::new(SERVER_LIMITS.message);
        let mut decoded = None;
        for byte in &frame {
            let bytes = [*byte];
            if let Some(frame) = decoder.consume(&mut bytes.as_slice()).unwrap() {
                decoded = Some(decode_server(frame.body()).unwrap());
            }
        }
        decoder.finish().unwrap();
        assert_eq!(decoded, Some(message));
    }
    assert_eq!(encode_client(&ClientMessage::End).unwrap(), [2, 34, 0]);
}
#[test]
fn malformed_ambiguous_unknown_operations_and_invalid_domains_are_rejected() {
    for body in [
        &[34, 0, 34, 0][..],
        &[58, 0],
        &[26, 4, 8, 1, 8, 2],
        &[10, 4, 24, 2, 8, 1],
    ] {
        assert!(decode_client(Bytes::copy_from_slice(body)).is_err());
    }
    for body in [&[50, 4, 8, 1, 16, 2][..], &[58, 2, 8, 2], &[66, 2, 24, 9]] {
        assert!(decode_server(Bytes::copy_from_slice(body)).is_err());
    }
    assert!(encode_client(&ClientMessage::Input(Bytes::from(vec![0; MAX_DATA + 1]))).is_err());
    assert!(
        encode_server(&ServerMessage::Screen(Bytes::from(vec![
            0;
            MAX_SNAPSHOT + 1
        ])))
        .is_err()
    );
    assert!(
        encode_server(&ServerMessage::Full(Bytes::from(vec![
            0;
            MAX_COMPRESSED_SNAPSHOT
                + 1
        ])))
        .is_err()
    );
    assert!(
        encode_client(&ClientMessage::Attach(Attach {
            version: 2,
            ..attach()
        }))
        .is_err()
    );
    assert!(
        encode_client(&ClientMessage::Attach(Attach {
            columns: 0,
            ..attach()
        }))
        .is_err()
    );
    assert!(encode_client(&ClientMessage::Rename("bad\nname".into())).is_err());
    assert!(encode_server(&ServerMessage::Renamed(" bad ".into())).is_err());
    assert!(decode_server(Bytes::from_static(&[74, 6, 10, 1, 97, 10, 1, 98])).is_err());
    for signal in [0, u32::MAX] {
        assert!(encode_server(&ServerMessage::Exit(Exit::Signal(signal))).is_err());
    }
    assert!(decode_server(Bytes::from_static(&[50, 2, 16, 0])).is_err());
    assert!(
        Hello {
            version: 1,
            terminal_id: ID.into()
        }
        .require("ffffffffffffffffffffffffffffffff")
        .is_err()
    );
    // Unknown bounded fields in a known operation remain forward-compatible.
    assert_eq!(
        decode_client(Bytes::from_static(&[34, 2, 8, 1])).unwrap(),
        ClientMessage::End
    );
}
#[test]
fn payloads_share_the_decoded_frame_and_maximum_snapshot_is_bounded() {
    let message = ServerMessage::Screen(Bytes::from(vec![42; MAX_SNAPSHOT]));
    let frame = encode_server(&message).unwrap();
    let body = protobuf::read_frame(&mut frame.as_slice(), SERVER_LIMITS.message)
        .unwrap()
        .unwrap();
    let base = body.as_ptr() as usize;
    let end = base + body.len();
    let ServerMessage::Screen(bytes) = decode_server(body).unwrap() else {
        panic!("screen missing")
    };
    assert!(bytes.as_ptr() as usize >= base && bytes.as_ptr() as usize + bytes.len() <= end);
    assert_eq!(bytes.len(), MAX_SNAPSHOT);
}
#[test]
fn malformed_io_retains_typed_transport_causes() {
    #[derive(Debug, thiserror::Error)]
    #[error("root cause")]
    struct Cause;
    struct Broken;
    impl Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::new(io::ErrorKind::ConnectionReset, Cause))
        }
    }
    let error = io::Error::from(read_server(&mut Broken).unwrap_err());
    assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
    assert!(error.get_ref().unwrap().is::<Cause>());
}

#[test]
fn terminal_payload_debug_and_trace_do_not_print_contents() {
    let message = ClientMessage::Input(Bytes::from_static(b"terminal-secret"));
    assert!(!format!("{message:?}").contains("terminal-secret"));
    #[cfg(feature = "trace")]
    {
        let mut output = Vec::new();
        protobuf::trace::trace(
            protobuf::DESCRIPTORS,
            "newport.terminal.holder.ClientFrame",
            &mut encode_client(&message).unwrap().as_slice(),
            &mut output,
        )
        .unwrap();
        assert!(
            !String::from_utf8(output)
                .unwrap()
                .contains("terminal-secret")
        );
    }
}
