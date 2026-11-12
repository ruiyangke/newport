use super::*;
use crate::browser::{MAX_TEXT, VERSION};

#[test]
fn announcement_is_protobuf_and_urls_and_warning_text_are_exact() {
    let hello = ServerMessage::Hello { version: VERSION };
    let mut encoded = Vec::new();
    write_server(&mut encoded, &hello).unwrap();
    assert_eq!(encoded, [4, 10, 2, 8, 1]);
    assert_eq!(read_server(&mut encoded.as_slice()).unwrap(), hello);
    let request = Request {
        url: "https://example.test/login?redirect_uri=http%3A%2F%2Flocalhost%2Fcallback#keep"
            .into(),
    };
    let mut encoded = Vec::new();
    write_request(&mut encoded, &request).unwrap();
    assert_eq!(read_request(&mut encoded.as_slice()).unwrap(), request);
    for reply in [
        Reply::Opened { warning: None },
        Reply::Opened {
            warning: Some("Opened without callback forwarding.".into()),
        },
        Reply::Failed(common::Error {
            code: "permission_denied".into(),
            message: "Disabled".into(),
            retry: common::Retry::No,
        }),
    ] {
        let message = ServerMessage::Reply(reply);
        let mut encoded = Vec::new();
        write_server(&mut encoded, &message).unwrap();
        assert_eq!(read_server(&mut encoded.as_slice()).unwrap(), message);
        for end in 0..encoded.len() {
            assert!(read_server(&mut &encoded[..end]).is_err(), "{end}");
        }
    }
}

#[test]
fn malformed_ambiguous_and_incompatible_messages_are_rejected() {
    for bytes in [
        vec![],
        vec![10, 2, 8, 1, 10, 2, 8, 1],
        vec![10, 4, 8, 1, 8, 1],
        vec![10, 2, 8, 2],
        vec![10, 2, 16, 1],
        vec![10, 2, 10, 0],
        vec![18, 2, 8, 1],
        vec![34, 0],
        vec![b'M', 0, 0, 0, 1, 0],
    ] {
        assert!(decode_server(bytes.into()).is_err());
    }
    for bytes in [vec![], vec![18, 0], vec![10, 0], vec![10, 4, 10, 0, 10, 0]] {
        assert!(decode_request(bytes.into()).is_err());
    }
    let mut oversized = vec![0xff, 0xff, 0xff, 0xff, 15];
    oversized.extend([0; 4]);
    assert!(matches!(
        read_request(&mut oversized.as_slice()),
        Err(Error::Frame(FrameError::TooLarge { .. }))
    ));
}

#[test]
fn bounded_unknown_nested_fields_do_not_change_a_known_operation() {
    assert_eq!(
        decode_server(Bytes::from_static(&[10, 4, 8, 1, 16, 7])).unwrap(),
        ServerMessage::Hello { version: VERSION }
    );
}

#[test]
fn only_bounded_http_urls_without_credentials_are_accepted() {
    for url in [
        "file:///tmp/file",
        "https://user:password@example.test/",
        "https://example.test/a b",
        "javascript:alert(1)",
        "http://",
        "https://example.test/\0",
    ] {
        assert!(Request { url: url.into() }.validate().is_err(), "{url}");
    }
    let mut url = "https://example.test/".to_owned();
    url.push_str(&"a".repeat(MAX_TEXT - url.len()));
    let mut bytes = Vec::new();
    write_request(&mut bytes, &Request { url: url.clone() }).unwrap();
    assert_eq!(read_request(&mut bytes.as_slice()).unwrap().url, url);
    url.push('a');
    assert!(write_request(&mut Vec::new(), &Request { url }).is_err());
    assert!(
        write_server(
            &mut Vec::new(),
            &ServerMessage::Reply(Reply::Opened {
                warning: Some("x".repeat(MAX_TEXT + 1))
            })
        )
        .is_err()
    );
}

#[test]
fn transport_errors_retain_their_io_source() {
    #[derive(Debug, thiserror::Error)]
    #[error("reset by peer")]
    struct Cause;
    #[derive(Debug, thiserror::Error)]
    #[error("peer reset")]
    struct Reset(#[source] Cause);
    struct Broken;
    impl Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::new(io::ErrorKind::ConnectionReset, Reset(Cause)))
        }
    }
    let error = read_request(&mut Broken).unwrap_err();
    assert!(matches!(
        &error,
        Error::Frame(FrameError::Io(error)) if error.kind() == io::ErrorKind::ConnectionReset
            && error.get_ref().unwrap().is::<Reset>()
    ));
    assert!(std::error::Error::source(&error).unwrap().is::<Cause>());
    let error = io::Error::from(error);
    assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
    assert_eq!(error.to_string(), "peer reset");
    assert!(error.get_ref().unwrap().is::<Reset>());
    assert!(std::error::Error::source(&error).unwrap().is::<Cause>());
}
