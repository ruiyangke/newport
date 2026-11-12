use super::*;
const RUNTIME: &str = "10000000-0000-4000-8000-000000000001";
const TOKEN: &str = "10000000-0000-4000-8000-000000000002";
fn access() -> Access {
    Access {
        path: "/run/newport/clipboard.sock".into(),
        runtime: RUNTIME.into(),
        worker: "worker".into(),
        token: TOKEN.into(),
    }
}
fn lease() -> ProviderLease {
    ProviderLease {
        runtime: RUNTIME.into(),
        worker: "worker".into(),
        epoch: 2,
    }
}
fn offer() -> Offer {
    Offer {
        runtime: RUNTIME.into(),
        revision: 3,
        origin: Origin::Native {
            worker: "worker".into(),
            revision: 4,
        },
        formats: vec!["text/plain".into()],
    }
}
fn export() -> ExportOffer {
    ExportOffer {
        offer: offer(),
        route_revision: 2,
    }
}
fn copy() -> CopyLease {
    CopyLease {
        id: TOKEN.into(),
        offer: export(),
    }
}
fn native() -> NativeRequest {
    NativeRequest {
        lease: lease(),
        id: 5,
        revision: 4,
        format: "text/plain".into(),
    }
}
fn chunk() -> Chunk {
    Chunk {
        bytes: vec![0, 1, 255],
        eof: true,
    }
}
fn requests() -> Vec<Request> {
    vec![
        Request::Authenticate(access()),
        Request::Offer,
        Request::Read {
            runtime: RUNTIME.into(),
            revision: 3,
            format: "text/plain".into(),
            offset: 0,
            limit: 4096,
            export: None,
        },
        Request::Read {
            runtime: RUNTIME.into(),
            revision: 3,
            format: "text/plain".into(),
            offset: 7,
            limit: 4096,
            export: Some(ExportRead::Focused(export())),
        },
        Request::Read {
            runtime: RUNTIME.into(),
            revision: 3,
            format: "text/plain".into(),
            offset: 7,
            limit: 4096,
            export: Some(ExportRead::Committed(copy())),
        },
        Request::Claim,
        Request::Enabled(false),
        Request::Enabled(true),
        Request::Publish {
            revision: i64::MIN,
            formats: vec!["text/plain".into()],
        },
        Request::Poll,
        Request::Reply(imports::Reply {
            id: 7,
            revision: i64::MIN,
            encoding: imports::Encoding::Raw,
            done: false,
            bytes: vec![],
        }),
        Request::Reply(imports::Reply {
            id: 7,
            revision: i64::MAX,
            encoding: imports::Encoding::Zlib,
            done: true,
            bytes: vec![1, 2, 3],
        }),
        Request::Reply(imports::Reply {
            id: 7,
            revision: 0,
            encoding: imports::Encoding::Failed,
            done: true,
            bytes: vec![],
        }),
        Request::NativePoll,
        Request::NativePublish {
            lease: lease(),
            expected_runtime: RUNTIME.into(),
            expected_revision: 3,
            revision: 4,
            formats: vec!["text/plain".into()],
        },
        Request::NativeReply {
            request: native(),
            chunk: Some(chunk()),
        },
        Request::NativeReply {
            request: native(),
            chunk: None,
        },
        Request::ExportOffer,
        Request::CommitCopy(export()),
        Request::CopyStatus,
        Request::ReleaseCopy(copy()),
    ]
}
fn responses() -> Vec<Response> {
    vec![
        Response::Hello(Hello {
            version: VERSION,
            runtime: RUNTIME.into(),
        }),
        Response::Ready,
        Response::Offer(offer()),
        Response::Offer(Offer {
            origin: Origin::Host,
            ..offer()
        }),
        Response::Offer(Offer {
            origin: Origin::Empty,
            formats: vec![],
            ..offer()
        }),
        Response::Chunk(chunk()),
        Response::Requests(vec![imports::Request {
            id: 1,
            revision: -42,
            format: "text/plain".into(),
        }]),
        Response::Provider(ProviderPoll {
            lease: Some(lease()),
            offer: offer(),
            requests: vec![native()],
        }),
        Response::Provider(ProviderPoll {
            lease: None,
            offer: offer(),
            requests: vec![],
        }),
        Response::Published(false),
        Response::Published(true),
        Response::Export(Some(export())),
        Response::Export(None),
        Response::Copy(Some(copy())),
        Response::Copy(None),
        Response::Error(common::Error {
            code: "denied".into(),
            message: "Permission revoked".into(),
            retry: common::Retry::No,
        }),
    ]
}
fn round_trip<T: Message + PartialEq + std::fmt::Debug>(value: T) {
    let bytes = value.encode().unwrap();
    assert_eq!(read::<T>(&mut bytes.as_slice()).unwrap(), value);
    let cuts: Vec<_> = if bytes.len() <= 4096 {
        (0..bytes.len()).collect()
    } else {
        vec![0, 1, 2, bytes.len() / 2, bytes.len() - 1]
    };
    for count in cuts {
        assert!(read::<T>(&mut &bytes[..count]).is_err(), "prefix {count}");
    }
}
#[test]
fn clipboard_access_uses_linux_paths_on_every_platform() {
    for path in [
        "/",
        "/run/newport/clipboard.sock",
        "/run/back\\slash/clipboard.sock",
    ] {
        let value = Access {
            path: path.into(),
            ..access()
        };
        round_trip(Request::Authenticate(value));
    }
    let longest = format!("/{}", "x".repeat(MAX_PATH - 1));
    round_trip(Request::Authenticate(Access {
        path: longest.into(),
        ..access()
    }));
    for path in [
        String::new(),
        "clipboard.sock".into(),
        r"C:\run\clipboard.sock".into(),
        r"\run\clipboard.sock".into(),
        r"\\server\share\clipboard.sock".into(),
        "/run/clipboard\0.sock".into(),
        format!("/{}", "x".repeat(MAX_PATH)),
    ] {
        let value = Access {
            path: path.clone().into(),
            ..access()
        };
        assert!(Request::Authenticate(value).encode().is_err());
        let mut generated = access_to_wire(&access()).unwrap();
        generated.path = path;
        assert!(access_from_wire(generated).is_err());
    }
}
#[test]
fn every_operation_round_trips_and_each_truncation_fails() {
    assert_eq!(Request::Offer.encode().unwrap(), [2, 18, 0]);
    assert_eq!(
        Response::Hello(Hello {
            version: VERSION,
            runtime: "r".into()
        })
        .encode()
        .unwrap(),
        [7, 10, 5, 8, 1, 18, 1, b'r']
    );
    for request in requests() {
        round_trip(request);
    }
    for response in responses() {
        round_trip(response);
    }
}
#[test]
fn malformed_oneofs_duplicate_fields_and_wrong_wire_types_are_rejected() {
    for body in [
        vec![],
        vec![18, 0, 18, 0],
        vec![130, 1, 0],
        vec![26, 4, 32, 0, 32, 1],
        vec![42, 2, 8, 2],
    ] {
        assert!(Request::decode(body.into()).is_err());
    }
    for body in [
        vec![],
        vec![18, 0, 18, 0],
        vec![90, 0],
        vec![10, 5, 8, 1, 8, 1, 0],
        vec![10, 2, 8, 2],
        vec![58, 2, 8, 2],
    ] {
        assert!(Response::decode(body.into()).is_err());
    }
    assert!(matches!(
        read::<Request>(&mut &[0xff, 0xff, 0xff, 0xff, 15][..]),
        Err(Error::Frame(FrameError::TooLarge { .. }))
    ));
    assert!(matches!(
        Request::decode(vec![0; LIMITS.message + 1].into()),
        Err(Error::Frame(FrameError::TooLarge { .. }))
    ));
    assert_eq!(
        Response::decode(Bytes::from_static(&[10, 7, 8, 1, 18, 1, b'r', 24, 7])).unwrap(),
        Response::Hello(Hello {
            version: 1,
            runtime: "r".into()
        })
    );
}
#[test]
fn metadata_and_chunk_bounds_cover_real_largest_messages() {
    let formats = (0..imports::MAX_FORMATS)
        .map(|index| format!("application/{}{:04}", "a".repeat(240), index))
        .collect::<Vec<_>>();
    let offer = Offer {
        formats: formats.clone(),
        ..offer()
    };
    round_trip(Response::Provider(ProviderPoll {
        lease: Some(lease()),
        offer,
        requests: (0..MAX_REQUESTS)
            .map(|index| NativeRequest {
                id: index as u64 + 1,
                format: formats[index].clone(),
                ..native()
            })
            .collect(),
    }));
    round_trip(Response::Chunk(Chunk {
        bytes: vec![42; imports::CHUNK],
        eof: false,
    }));
    assert!(
        Response::Chunk(Chunk {
            bytes: vec![0; imports::CHUNK + 1],
            eof: true
        })
        .encode()
        .is_err()
    );
    assert!(
        Response::Requests(vec![
            imports::Request {
                id: 1,
                revision: 1,
                format: "text/plain".into()
            };
            MAX_REQUESTS + 1
        ])
        .encode()
        .is_err()
    );
    for limit in [0, imports::CHUNK + 1] {
        assert!(
            Request::Read {
                runtime: RUNTIME.into(),
                revision: 1,
                format: "text/plain".into(),
                offset: 0,
                limit,
                export: None
            }
            .encode()
            .is_err()
        );
    }
    assert!(
        Request::Reply(imports::Reply {
            id: 0,
            revision: 1,
            encoding: imports::Encoding::Raw,
            done: true,
            bytes: vec![]
        })
        .encode()
        .is_err()
    );
}
#[test]
fn credentials_remain_redacted_in_semantic_generated_debug_and_trace() {
    let request = Request::Authenticate(access());
    assert!(!format!("{request:?}").contains(TOKEN));
    assert!(!format!("{:?}", access_to_wire(&access()).unwrap()).contains(TOKEN));
    #[cfg(feature = "trace")]
    {
        let mut output = vec![];
        protobuf::trace::trace(
            protobuf::DESCRIPTORS,
            "newport.clipboard.service.ClientFrame",
            &mut request.encode().unwrap().as_slice(),
            &mut output,
        )
        .unwrap();
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("[redacted]"));
        assert!(!output.contains(TOKEN));
    }
}
#[test]
fn io_errors_keep_their_kind_and_custom_source() {
    #[derive(Debug, thiserror::Error)]
    #[error("root cause")]
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
    let error = read::<Response>(&mut Broken).unwrap_err();
    assert!(std::error::Error::source(&error).unwrap().is::<Cause>());
    let error = io::Error::from(error);
    assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
    assert!(error.get_ref().unwrap().is::<Reset>());
}
