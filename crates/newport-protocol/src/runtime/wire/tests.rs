use super::*;

const RUNTIME: &str = "11111111-1111-4111-8111-111111111111";
const WORKER: &str = "22222222-2222-4222-8222-222222222222";
const SESSION: &str = "33333333-3333-4333-8333-333333333333";

fn hello() -> Hello {
    Hello {
        versions: vec![VERSION],
        instance_id: RUNTIME.into(),
        build_sha256: "a".repeat(64),
        agent_version: None,
        worker_socket: "/run/newport/worker.sock".into(),
        product: product::Variant::Core,
        services: [(
            "git".into(),
            Capability {
                contract_id: None,
                versions: vec![1],
                unavailable: Some("Install Git".into()),
            },
        )]
        .into(),
        dependencies: [("git".into(), false)].into(),
        limits: Limits::default(),
    }
}
fn permissions() -> Permissions {
    Permissions {
        revision: 2,
        clipboard: true,
        browser: false,
    }
}
fn error() -> common::Error {
    common::Error {
        code: "busy".into(),
        message: "Retry after the running worker finishes".into(),
        retry: common::Retry::Yes,
    }
}
fn open() -> admission::Open {
    admission::Open {
        version: 1,
        runtime_id: RUNTIME.into(),
        worker_id: WORKER.into(),
        build_sha256: "a".repeat(64),
        ticket: common::Secret {
            value: "b".repeat(64).into_bytes().into(),
        },
        service: "monitor".into(),
        service_version: 1,
        contract_id: service_contract_id("monitor").map(str::to_owned),
        arguments: vec!["literal ' $ \n argument".into()],
    }
}
fn round_trip<T: Message + fmt::Debug>(value: &T) -> T {
    let bytes = value.encode().unwrap();
    let mut input = bytes.as_slice();
    let read = read::<T>(&mut input).unwrap();
    assert!(input.is_empty());
    read
}
fn frame_body(bytes: &[u8]) -> Bytes {
    protobuf::read_frame(&mut &*bytes, admission::LIMITS.message)
        .unwrap()
        .unwrap()
}
fn bytes_field(tag: u32, value: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    prost::encoding::encode_varint(u64::from(tag) << 3 | 2, &mut bytes);
    prost::encoding::encode_varint(value.len() as u64, &mut bytes);
    bytes.extend_from_slice(value);
    bytes
}

#[test]
fn announcements_keep_product_availability_and_versions_separate() {
    let value = hello();
    assert_eq!(round_trip(&value), value);
    assert!(value.require_compatible("git", 1).is_ok());
    assert_eq!(value.require("git", 1).unwrap_err().code, "unavailable");
    assert_eq!(value.require("git", 2).unwrap_err().code, "incompatible");
    let mut newer = value;
    newer.versions = vec![2];
    newer.services.get_mut("git").unwrap().versions = vec![2];
    assert_eq!(round_trip(&newer), newer);
}

#[test]
fn announcements_carry_the_semantic_product_version() {
    for version in [None, Some("0.6.4"), Some("0.7.0-rc.1+local")] {
        let value = Hello {
            agent_version: version.map(str::to_owned),
            ..hello()
        };
        assert_eq!(round_trip(&value), value);
    }
    let mut value = hello();
    value.agent_version = Some("812297d".into());
    assert!(value.encode().is_err());
    let mut inner = convert::hello(&hello()).unwrap().encode_to_vec();
    inner.extend(bytes_field(9, b"812297d"));
    let body = bytes_field(1, &inner);
    let mut framed = Vec::new();
    protobuf::write_frame(&mut framed, &body).unwrap();
    assert!(matches!(
        read::<Hello>(&mut framed.as_slice()),
        Err(Error::Invalid(_))
    ));
}

#[test]
fn monitor_admission_requires_the_announced_schema_in_both_directions() {
    let expected = service_contract_id("monitor").unwrap();
    let different = "0".repeat(64);
    assert_ne!(expected, different);
    for announced in [None, Some(expected), Some(different.as_str())] {
        let mut value = hello();
        value.services.insert(
            "monitor".into(),
            Capability {
                versions: vec![crate::monitor::VERSION],
                unavailable: None,
                contract_id: announced.map(str::to_owned),
            },
        );
        let decoded = round_trip(&value);
        assert_eq!(decoded, value);
        for requested in [None, Some(expected), Some(different.as_str())] {
            let result = decoded.require_contract("monitor", crate::monitor::VERSION, requested);
            assert_eq!(result.is_ok(), announced == requested);
            if let Err(error) = result {
                assert_eq!(error.code, "incompatible");
                assert_eq!(error.retry, common::Retry::No);
            }
        }
    }
}

#[test]
fn contract_identifiers_round_trip_or_fail_shape_validation() {
    for id in [None, Some(crate::monitor::CONTRACT_ID.into())] {
        let mut value = open();
        value.contract_id = id.clone();
        assert_eq!(round_trip(&value).contract_id, id);
        let request = Request {
            id: 1,
            runtime_id: RUNTIME.into(),
            operation: Operation::PrepareWorkerStream {
                worker_id: WORKER.into(),
                service: "monitor".into(),
                version: 1,
                contract_id: id,
            },
        };
        assert_eq!(round_trip(&request), request);
    }
    for id in [
        String::new(),
        "a".repeat(63),
        "a".repeat(65),
        "A".repeat(64),
        "g".repeat(64),
    ] {
        let mut value = open();
        value.contract_id = Some(id.clone());
        assert!(value.encode().is_err());
        let inner = convert::open(&value).unwrap().encode_to_vec();
        assert!(admission::Open::decode(bytes_field(1, &inner).into()).is_err());
        let mut value = hello();
        value.services.get_mut("git").unwrap().contract_id = Some(id.clone());
        assert!(value.encode().is_err());
        let inner = convert::hello(&value).unwrap().encode_to_vec();
        assert!(Hello::decode(bytes_field(1, &inner).into()).is_err());
        let value = Request {
            id: 1,
            runtime_id: RUNTIME.into(),
            operation: Operation::PrepareWorkerStream {
                worker_id: WORKER.into(),
                service: "monitor".into(),
                version: 1,
                contract_id: Some(id),
            },
        };
        assert!(value.encode().is_err());
        let inner = convert::request(&value).unwrap().encode_to_vec();
        assert!(Request::decode(bytes_field(1, &inner).into()).is_err());
    }
}

#[test]
fn every_runtime_operation_and_correlated_result_round_trips() {
    let route = clipboard::Route {
        revision: 4,
        worker_id: Some(WORKER.into()),
        exports: true,
        focused: true,
    };
    let operations = [
        (
            Operation::Initialize {
                version: 1,
                session_id: SESSION.into(),
                transport_generation: 1,
            },
            ReplyValue::Accepted,
        ),
        (
            Operation::PrepareWorkerStream {
                worker_id: WORKER.into(),
                service: "monitor".into(),
                version: 1,
                contract_id: service_contract_id("monitor").map(str::to_owned),
            },
            ReplyValue::WorkerStream(admission::Endpoint {
                path: "/run/newport/stream.sock".into(),
                ticket: open().ticket,
            }),
        ),
        (
            Operation::Permissions {
                worker_id: WORKER.into(),
                permissions: permissions(),
            },
            ReplyValue::Permissions(Applied {
                permissions: permissions(),
                error: None,
            }),
        ),
        (
            Operation::ClipboardRoute(route.clone()),
            ReplyValue::ClipboardRoute(clipboard::RouteReply {
                revision: route.revision,
                worker_id: route.worker_id.clone(),
                outcome: clipboard::RouteOutcome::Applied,
            }),
        ),
        (Operation::Ping, ReplyValue::Accepted),
        (Operation::Shutdown, ReplyValue::Accepted),
    ];
    for (index, (operation, result)) in operations.into_iter().enumerate() {
        let request = Request {
            id: index as u64 + 1,
            runtime_id: RUNTIME.into(),
            operation,
        };
        assert_eq!(round_trip(&request), request);
        for result in [Ok(result), Err(error())] {
            let reply = Reply {
                id: request.id,
                runtime_id: RUNTIME.into(),
                result,
            };
            let decoded = round_trip(&reply);
            decoded.validate_for(&request).unwrap();
            assert_eq!(decoded.encode().unwrap(), reply.encode().unwrap());
        }
    }
}

#[test]
fn worker_lease_and_permission_updates_round_trip() {
    let attach = Attach {
        runtime_id: RUNTIME.into(),
        worker_id: WORKER.into(),
        build_sha256: "a".repeat(64),
        service: "integration".into(),
    };
    assert_eq!(round_trip(&attach), attach);
    for result in [
        Ok(Lease {
            session_id: SESSION.into(),
            clipboard: Some(clipboard::Access {
                path: "/run/newport/clipboard.sock".into(),
                runtime: RUNTIME.into(),
                worker: WORKER.into(),
                token: SESSION.into(),
            }),
        }),
        Err(error()),
    ] {
        let attached = Attached {
            runtime_id: RUNTIME.into(),
            worker_id: WORKER.into(),
            result,
        };
        attached
            .validate_for(&attach, "/run/newport/worker.sock")
            .unwrap();
        assert_eq!(
            round_trip(&attached).encode().unwrap(),
            attached.encode().unwrap()
        );
    }
    let update = WorkerPermissions {
        runtime_id: RUNTIME.into(),
        worker_id: WORKER.into(),
        permissions: permissions(),
    };
    assert_eq!(round_trip(&update), update);
    let applied = Applied {
        permissions: permissions(),
        error: Some(error()),
    };
    assert_eq!(round_trip(&applied), applied);
}

#[test]
fn lease_response_cannot_authorize_another_worker_or_clipboard_endpoint() {
    let request = Attach {
        runtime_id: RUNTIME.into(),
        worker_id: WORKER.into(),
        build_sha256: "a".repeat(64),
        service: "integration".into(),
    };
    let value = Attached {
        runtime_id: RUNTIME.into(),
        worker_id: WORKER.into(),
        result: Ok(Lease {
            session_id: SESSION.into(),
            clipboard: Some(clipboard::Access {
                path: "/run/newport/clipboard.sock".into(),
                runtime: RUNTIME.into(),
                worker: WORKER.into(),
                token: SESSION.into(),
            }),
        }),
    };
    value
        .validate_for(&request, "/run/newport/worker.sock")
        .unwrap();
    for mutate in [
        |value: &mut Attached| value.worker_id = SESSION.into(),
        |value: &mut Attached| value.runtime_id = SESSION.into(),
        |value: &mut Attached| {
            value
                .result
                .as_mut()
                .unwrap()
                .clipboard
                .as_mut()
                .unwrap()
                .path = "/run/other/clipboard.sock".into()
        },
        |value: &mut Attached| {
            value
                .result
                .as_mut()
                .unwrap()
                .clipboard
                .as_mut()
                .unwrap()
                .token = uuid::Uuid::nil().to_string()
        },
    ] {
        let mut invalid = value.clone();
        mutate(&mut invalid);
        assert!(
            invalid
                .validate_for(&request, "/run/newport/worker.sock")
                .is_err()
        );
    }
    let mut value = value;
    let access = value.result.as_mut().unwrap().clipboard.as_mut().unwrap();
    access.path = r"/run/a\b/clipboard.sock".into();
    value
        .validate_for(&request, r"/run/a\b/worker.sock")
        .unwrap();
    assert!(
        value
            .validate_for(&request, "/run/a/b/worker.sock")
            .is_err()
    );
}

#[test]
fn admission_is_bound_to_service_version_identity_and_build() {
    let open = open();
    assert_eq!(round_trip(&open).encode().unwrap(), open.encode().unwrap());
    let started = admission::Admission::Started(admission::Started {
        runtime_id: RUNTIME.into(),
        worker_id: WORKER.into(),
        build_sha256: "a".repeat(64),
    });
    assert_eq!(round_trip(&started), started);
    started.validate_for(&open).unwrap();
    for change in [
        |v: &mut admission::Open| v.runtime_id = SESSION.into(),
        |v: &mut admission::Open| v.worker_id = SESSION.into(),
        |v: &mut admission::Open| v.build_sha256 = "c".repeat(64),
    ] {
        let mut wrong = open.clone();
        change(&mut wrong);
        assert!(started.validate_for(&wrong).is_err());
    }
    let refused = admission::Admission::Refused(error());
    assert_eq!(round_trip(&refused), refused);
    let mut invalid = open;
    invalid.service_version = 0;
    assert!(invalid.encode().is_err());
    assert!(refused.validate_for(&invalid).is_err());
}

#[test]
fn request_and_reply_identity_and_result_shape_are_checked() {
    let request = Request {
        id: 1,
        runtime_id: RUNTIME.into(),
        operation: Operation::Ping,
    };
    let mut reply = Reply {
        id: 2,
        runtime_id: RUNTIME.into(),
        result: Ok(ReplyValue::Accepted),
    };
    assert!(reply.validate_for(&request).is_err());
    reply.id = 1;
    reply.runtime_id = SESSION.into();
    assert!(reply.validate_for(&request).is_err());
    reply.runtime_id = RUNTIME.into();
    reply.result = Ok(ReplyValue::Permissions(Applied {
        permissions: permissions(),
        error: None,
    }));
    assert!(reply.validate_for(&request).is_err());
    let request = Request {
        operation: Operation::Permissions {
            worker_id: WORKER.into(),
            permissions: Permissions {
                browser: true,
                clipboard: false,
                ..permissions()
            },
        },
        ..request
    };
    assert!(
        reply.validate_for(&request).is_err(),
        "an acknowledgement cannot add permission"
    );
    if let Ok(ReplyValue::Permissions(applied)) = &mut reply.result {
        applied.permissions = Permissions {
            clipboard: false,
            browser: false,
            ..permissions()
        };
    }
    reply.validate_for(&request).unwrap();
}

#[test]
fn ambiguous_top_level_frames_requests_and_results_are_rejected() {
    let hello = frame_body(&hello().encode().unwrap());
    assert!(Hello::decode([hello.as_ref(), hello.as_ref()].concat().into()).is_err());
    let request = Request {
        id: 1,
        runtime_id: RUNTIME.into(),
        operation: Operation::Ping,
    };
    let raw = convert::request(&request).unwrap().encode_to_vec();
    for extra in [bytes_field(7, &[]), bytes_field(8, &[])] {
        let inner = [raw.as_slice(), &extra].concat();
        assert!(Request::decode(bytes_field(1, &inner).into()).is_err());
    }
    let reply = Reply {
        id: 1,
        runtime_id: RUNTIME.into(),
        result: Ok(ReplyValue::Accepted),
    };
    let raw = convert::reply(&reply).unwrap().encode_to_vec();
    for extra in [
        bytes_field(3, &[]),
        bytes_field(7, &convert::error(&error()).encode_to_vec()),
    ] {
        let inner = [raw.as_slice(), &extra].concat();
        assert!(Reply::decode(bytes_field(2, &inner).into()).is_err());
    }
}

#[test]
fn malformed_unknown_and_wrong_direction_messages_are_not_runtime_hellos() {
    for raw in [
        vec![],
        vec![0],
        bytes_field(50, &[]),
        bytes_field(1, &[]),
        bytes_field(1, &[0]),
        frame_body(
            &Request {
                id: 1,
                runtime_id: RUNTIME.into(),
                operation: Operation::Ping,
            }
            .encode()
            .unwrap(),
        )
        .to_vec(),
    ] {
        assert!(Hello::decode(raw.into()).is_err());
    }
    let mut generated = convert::hello(&hello()).unwrap();
    generated.product = 999;
    let frame = pb::ServerFrame {
        message: Some(pb::server_frame::Message::Hello(generated)),
    };
    assert!(Hello::decode(frame.encode_to_vec().into()).is_err());
}

#[test]
fn oversized_announcements_and_admission_arguments_fail_locally() {
    let mut invalid = hello();
    invalid.worker_socket = "/run/../worker.sock".into();
    assert!(invalid.encode().is_err());
    invalid = hello();
    invalid.instance_id = uuid::Uuid::nil().to_string();
    assert!(invalid.encode().is_err());
    invalid = hello();
    invalid.services.get_mut("git").unwrap().versions = vec![1, 1];
    assert!(invalid.encode().is_err());
    let mut request = open();
    request.arguments = vec!["\u{1}".repeat(16384); 4];
    round_trip(&request);
    request.arguments.push("x".into());
    assert!(request.encode().is_err());
    request = open();
    request.arguments = vec!["".into(); admission::MAX_ARGUMENTS + 1];
    assert!(request.encode().is_err());
    request = open();
    request.arguments = vec!["argument\0injection".into()];
    assert!(request.encode().is_err());
    assert!(matches!(
        Hello::decode(vec![0; MAX_FRAME + 1].into()),
        Err(Error::Frame(FrameError::TooLarge { .. }))
    ));
    assert!(matches!(
        Request::decode(vec![0; MAX_FRAME + 1].into()),
        Err(Error::Frame(FrameError::TooLarge { .. }))
    ));
    assert!(matches!(
        admission::Admission::decode(vec![0; MAX_FRAME + 1].into()),
        Err(Error::Frame(FrameError::TooLarge { .. }))
    ));
}

#[test]
fn admission_credentials_are_redacted_in_debug_and_trace() {
    let open = open();
    let ticket = String::from_utf8(open.ticket.value.to_vec()).unwrap();
    assert!(!format!("{open:?}").contains(&ticket));
    let endpoint = admission::Endpoint {
        path: "/run/newport/stream.sock".into(),
        ticket: open.ticket.clone(),
    };
    assert!(!format!("{endpoint:?}").contains(&ticket));
    let mut output = Vec::new();
    protobuf::trace::trace(
        protobuf::DESCRIPTORS,
        "newport.runtime.AdmissionClientFrame",
        &mut open.encode().unwrap().as_slice(),
        &mut output,
    )
    .unwrap();
    let output = String::from_utf8(output).unwrap();
    assert!(output.contains("[redacted]"));
    assert!(!output.contains(&ticket));
}

#[test]
fn io_adapters_preserve_transport_failure_kinds() {
    use std::io::{Error as IoError, ErrorKind};
    for kind in [
        ErrorKind::TimedOut,
        ErrorKind::PermissionDenied,
        ErrorKind::ConnectionReset,
    ] {
        let error = Error::Frame(FrameError::Io(IoError::new(kind, "transport")));
        assert_eq!(IoError::from(error).kind(), kind);
    }
    for error in [Error::Ended, Error::Frame(FrameError::Truncated)] {
        assert_eq!(IoError::from(error).kind(), ErrorKind::UnexpectedEof);
    }
    assert_eq!(
        IoError::from(Error::Unexpected).kind(),
        ErrorKind::InvalidData
    );
    assert_eq!(
        IoError::from(Error::Invalid(Invalid("lease"))).kind(),
        ErrorKind::InvalidData
    );
}
