use super::*;
use crate::common::{Present, Retry};
use crate::desktop::{account as a, files as f, host as h, stream as s};
use prost::Message;

fn discovery_response(
    value: crate::encoding::protobuf::generated::newport::desktop::connection::Discovery,
) -> Bytes {
    use crate::encoding::protobuf::generated::newport::desktop as pb;
    pb::ServerMessage {
        value: Some(pb::server_message::Value::Response(Box::new(
            pb::Response {
                id: Some(1),
                outcome: Some(pb::response::Outcome::Completed(Box::new(pb::Reply {
                    value: Some(pb::reply::Value::Connection(Box::new(
                        pb::connection::Reply {
                            value: Some(pb::connection::reply::Value::ConnectionsQuery(Box::new(
                                pb::connection::PortsObservation {
                                    discovery: Some(value),
                                    fetching: Some(false),
                                    stale: Some(false),
                                    revision: Some(1),
                                    ..Default::default()
                                },
                            ))),
                        },
                    ))),
                }))),
            },
        ))),
    }
    .encode_to_vec()
    .into()
}

#[test]
fn active_discovery_reply_rejects_combined_allocations_before_generated_decoding() {
    use crate::encoding::protobuf::generated::newport::{
        desktop::connection::Discovery, discovery as pb,
    };
    use crate::{desktop::connection::discovery as limits, monitor::ports::discovery as contract};
    for kind in 0..5 {
        let limit = if kind == 4 {
            contract::MAX_DETAILS
        } else {
            limits::MAX_ROWS / 2
        };
        let snapshot = |count| {
            let mut value = pb::Snapshot::default();
            match kind {
                0 => value.sockets = vec![pb::Socket::default(); count],
                1 => value.processes = vec![pb::Process::default(); count],
                2 => value.services = vec![pb::Service::default(); count],
                3 => value.publications = vec![pb::Publication::default(); count],
                _ => {
                    value.processes = vec![
                        pb::Process {
                            details: Some(pb::Details::default()),
                            ..Default::default()
                        };
                        count
                    ]
                }
            }
            value
        };
        let wire = Discovery {
            current: Some(snapshot(limit)),
            retained: vec![snapshot(limit), snapshot(1)],
            retention: None,
        };
        assert!(
            matches!(
                decode_server(discovery_response(wire)),
                Err(Error::Invalid(Invalid(
                    "combined discovery allocation count"
                )))
            ),
            "kind {kind}"
        );
    }
}

#[test]
fn active_discovery_reply_shares_the_aggregate_field_budget_between_scans() {
    use crate::encoding::protobuf::generated::newport::{
        desktop::connection::Discovery, discovery as pb,
    };
    let snapshot = pb::Snapshot {
        processes: vec![
            pb::Process {
                details: Some(pb::Details {
                    arguments: vec![String::new(); 4096],
                    ..Default::default()
                }),
                ..Default::default()
            };
            15
        ],
        ..Default::default()
    };
    let wire = Discovery {
        current: Some(snapshot.clone()),
        retained: vec![snapshot; 3],
        retention: None,
    };
    assert!(matches!(
        decode_server(discovery_response(wire)),
        Err(Error::Invalid(Invalid("combined discovery field budget")))
    ));
}

#[test]
fn discovery_mapping_preserves_current_and_stale_source_generations() {
    use crate::{
        desktop::connection::discovery::Discovery,
        monitor::ports::discovery::{self as d, wire::Record},
    };
    let scope = d::Scope {
        network: Some(d::Namespace {
            device: 1,
            inode: 2,
        }),
        processes: Some(d::Namespace {
            device: 1,
            inode: 3,
        }),
        boot_id: Some("18f68fcb-71e9-47fe-a226-e9da27d82953".into()),
    };
    let current = d::Snapshot {
        generation: 2,
        scope: scope.clone(),
        ..Default::default()
    };
    let mut old = current.clone();
    old.generation = 1;
    old.coverage.ipv4 = d::Report::complete();
    old.sockets.push(d::Socket {
        id: 1,
        endpoint: d::Endpoint {
            address: std::net::Ipv4Addr::UNSPECIFIED.into(),
            port: 22,
            interface: None,
        },
        source: d::SocketSource::Diagnostic,
        inode: 3,
        cookie: Some(0),
        uid: 0,
        account: Some("root".into()),
        ipv6_only: None,
        cgroup: None,
        holders: Vec::new(),
        holder_state: d::HolderState::Unresolved,
        holders_truncated: false,
        services: Vec::new(),
    });
    let value = Discovery {
        current,
        retained: vec![old],
        retention: d::Report::complete(),
    };
    let encoded = value.to_proto().unwrap().encode_to_vec();
    let wire = <Discovery as Convert>::Proto::decode(encoded.as_slice()).unwrap();
    assert_eq!(Discovery::from_proto(wire.clone()).unwrap(), value);
    let mut invalid = wire.clone();
    invalid.current = None;
    assert!(Discovery::from_proto(invalid).is_err());
    let mut invalid = wire.clone();
    invalid.retention = None;
    assert!(Discovery::from_proto(invalid).is_err());
    let mut invalid = wire;
    invalid.retained.push(value.retained[0].to_wire());
    assert!(Discovery::from_proto(invalid).is_err());
    let measured = d::wire::measure(&value.retained[0]).unwrap();
    assert_eq!(measured.bytes, value.retained[0].to_wire().encoded_len());
    assert!(measured.fields > value.retained[0].sockets.len());
    let request = crate::desktop::Request {
        id: RequestId::new(1).unwrap(),
        operation: Operation::Connection(Box::new(
            crate::desktop::connection::Operation::ConnectionsQuery(Box::new(
                crate::desktop::connection::ConnectionsQueryArgs {
                    server: "11111111-1111-4111-8111-111111111111".parse().unwrap(),
                    refresh: true,
                },
            )),
        )),
    };
    let reply = ServerMessage::Response(Box::new(Response {
        id: request.id,
        outcome: Outcome::Completed(Box::new(Reply::Connection(Box::new(
            crate::desktop::connection::Reply::ConnectionsQuery(Box::new(
                crate::desktop::connection::PortsObservation {
                    data: Some(value.clone()),
                    error: Some("Inventory unavailable.".into()),
                    fetching: false,
                    stale: true,
                    revision: 1,
                    updated_at: Some(1),
                },
            )),
        )))),
    }));
    let decoded =
        decode_server_for(body(encode_server_for(&reply, &request).unwrap()), &request).unwrap();
    let ServerMessage::Response(response) = decoded else {
        panic!("response");
    };
    let Outcome::Completed(reply) = response.outcome else {
        panic!("completed");
    };
    let Reply::Connection(reply) = *reply else {
        panic!("connection");
    };
    let crate::desktop::connection::Reply::ConnectionsQuery(observation) = *reply else {
        panic!("discovery");
    };
    assert_eq!(observation.data, Some(value));
    assert!(observation.stale);
}

fn body(frame: Vec<u8>) -> Bytes {
    protobuf::read_frame(&mut frame.as_slice(), MAX_RESPONSE)
        .unwrap()
        .unwrap()
}
fn request(operation: Operation) -> ClientMessage {
    ClientMessage::Request(Box::new(Request {
        id: RequestId::new(7).unwrap(),
        operation,
    }))
}
fn activate() -> ClientMessage {
    request(Operation::Host(Box::new(h::Operation::Activate(Box::new(
        Present {},
    )))))
}
#[test]
fn permission_snapshots_preserve_unacknowledged_denial_without_granting_access() {
    use crate::{desktop::configuration::PermissionsSnapshot, runtime::Permissions};
    let mut snapshot = PermissionsSnapshot {
        desired: Permissions {
            revision: 7,
            clipboard: true,
            browser: true,
        },
        effective: Permissions {
            revision: 0,
            clipboard: false,
            browser: false,
        },
        error: None,
        clipboard_epoch: 0,
        browser_epoch: 0,
        session_epoch: 0,
    };
    // An observation can lack an acknowledgement; a runtime update cannot.
    assert!(snapshot.effective.to_proto().is_err());
    for revision in [0, 7] {
        snapshot.effective.revision = revision;
        snapshot.validate().unwrap();
        let decoded = PermissionsSnapshot::from_proto(snapshot.to_proto().unwrap()).unwrap();
        assert_eq!(decoded.desired, snapshot.desired);
        assert_eq!(decoded.effective, snapshot.effective);
    }
    snapshot.effective = snapshot.desired;
    let decoded = PermissionsSnapshot::from_proto(snapshot.to_proto().unwrap()).unwrap();
    assert_eq!(decoded.effective, snapshot.desired);

    snapshot.effective.revision = 0;
    for (clipboard, browser) in [(true, false), (false, true), (true, true)] {
        snapshot.effective.clipboard = clipboard;
        snapshot.effective.browser = browser;
        assert!(snapshot.validate().is_err());
        assert!(snapshot.to_proto().is_err());
        let mut valid = snapshot.clone();
        valid.effective.revision = valid.desired.revision;
        let mut malformed = valid.to_proto().unwrap();
        malformed.effective.as_mut().unwrap().revision = 0;
        assert!(PermissionsSnapshot::from_proto(malformed).is_err());
    }
    snapshot.effective.clipboard = false;
    snapshot.effective.browser = false;
    let valid = snapshot.to_proto().unwrap();
    let mut missing = valid.clone();
    missing.effective = None;
    assert!(PermissionsSnapshot::from_proto(missing).is_err());
    let mut invalid = valid;
    invalid.desired.as_mut().unwrap().revision = 0;
    assert!(PermissionsSnapshot::from_proto(invalid).is_err());
    snapshot.desired.revision = 0;
    assert!(snapshot.validate().is_err());
    assert!(snapshot.to_proto().is_err());
}
#[test]
fn worker_stream_arguments_keep_their_semantic_limits_on_encode_and_decode() {
    let apps = |arguments| {
        request(Operation::Stream(Box::new(s::Operation::RemoteAppsOpen(
            Box::new(s::RemoteAppsOpenArgs {
                server: uuid::Uuid::from_u128(1),
                arguments,
            }),
        ))))
    };
    let command = |command: String| {
        request(Operation::Stream(Box::new(s::Operation::CommandOpen(
            Box::new(s::CommandOpenArgs {
                server: uuid::Uuid::from_u128(1),
                command,
            }),
        ))))
    };
    for valid in [
        apps(vec![]),
        apps(vec![String::new(); s::MAX_REMOTE_ARGUMENTS]),
        apps(vec!["x".repeat(s::MAX_REMOTE_ARGUMENT_BYTES); 4]),
        apps(vec!["é".repeat(s::MAX_REMOTE_ARGUMENT_BYTES / 2)]),
        command("x".repeat(s::MAX_COMMAND_BYTES)),
        command(" printf 'literal command'\n".into()),
    ] {
        valid.validate().unwrap();
        decode_client(body(encode_client(&valid).unwrap()), false).unwrap();
    }
    for invalid in [
        apps(vec![String::new(); s::MAX_REMOTE_ARGUMENTS + 1]),
        apps(vec!["x".repeat(s::MAX_REMOTE_ARGUMENT_BYTES + 1)]),
        apps(vec!["é".repeat(s::MAX_REMOTE_ARGUMENT_BYTES / 2 + 1)]),
        apps(
            [
                vec!["x".repeat(s::MAX_REMOTE_ARGUMENT_BYTES); 4],
                vec!["x".into()],
            ]
            .concat(),
        ),
        apps(vec!["a\0b".into()]),
        command(String::new()),
        command(" \t\r\n".into()),
        command("x".repeat(s::MAX_COMMAND_BYTES + 1)),
        command("x\0".into()),
    ] {
        assert!(invalid.validate().is_err());
        assert!(encode_client(&invalid).is_err());
        assert!(decode_client(proto_body(&invalid), false).is_err());
    }
}
#[test]
fn lsp_open_checks_remote_paths_and_plain_identifiers_in_both_wire_directions() {
    use crate::desktop::configuration::{AuthMethod, ConnectionScope, ConnectionSettings};
    let lsp = |root: &str, name: &str| {
        let server = uuid::Uuid::from_u128(1);
        request(Operation::Stream(Box::new(s::Operation::LspOpen(
            Box::new(s::LspOpenArgs {
                server,
                expected: ConnectionScope {
                    settings: ConnectionSettings {
                        server_id: server,
                        ssh_host: "fixture".into(),
                        ssh_user: "user".into(),
                        ssh_port: 22,
                        auth_method: AuthMethod::PublicKey,
                        identity_file: None,
                        agent_source: None,
                        agent_key_fingerprint: None,
                        proxy: None,
                    },
                    connection_revision: 1,
                    backend_instance: uuid::Uuid::from_u128(2),
                },
                root: root.into(),
                server_name: name.into(),
                envrc: false,
            }),
        ))))
    };
    let valid = lsp("/srv/space 项目", "rust-analyzer");
    let decoded = decode_client(body(encode_client(&valid).unwrap()), false).unwrap();
    let ClientMessage::Request(decoded) = decoded else {
        panic!("expected request")
    };
    let Operation::Stream(decoded) = decoded.operation else {
        panic!("expected stream")
    };
    let s::Operation::LspOpen(decoded) = *decoded else {
        panic!("expected LSP open")
    };
    assert_eq!(decoded.root, "/srv/space 项目");
    assert_eq!(decoded.server_name, "rust-analyzer");
    assert!(!decoded.envrc);
    let oversized_root = format!("/{}", "x".repeat(4096));
    let oversized_name = "a".repeat(41);
    for (root, name) in [
        ("srv/app", "rust-analyzer"),
        (oversized_root.as_str(), "rust-analyzer"),
        ("/srv/app\0", "rust-analyzer"),
        ("/srv/app", ""),
        ("/srv/app", "rust analyzer; rm -rf /"),
        ("/srv/app", oversized_name.as_str()),
    ] {
        let invalid = lsp(root, name);
        assert!(
            encode_client(&invalid).is_err(),
            "encode {root:?}, {name:?}"
        );
        // Generated records alone must not bypass the semantic admission gate.
        assert!(
            decode_client(proto_body(&invalid), false).is_err(),
            "decode {root:?}, {name:?}"
        );
    }
}
fn proto_body<T: Convert>(value: &T) -> Bytes
where
    T::Proto: prost::Message,
{
    Bytes::from(value.to_proto().unwrap().encode_to_vec())
}
fn append_field(mut output: Vec<u8>, tag: u32, bytes: &[u8]) -> Vec<u8> {
    let mut key = u64::from(tag) << 3 | 2;
    while key >= 128 {
        output.push((key as u8 & 127) | 128);
        key >>= 7;
    }
    output.push(key as u8);
    let mut length = bytes.len();
    while length >= 128 {
        output.push((length as u8 & 127) | 128);
        length >>= 7;
    }
    output.push(length as u8);
    output.extend_from_slice(bytes);
    output
}

#[test]
fn desktop_contracts_must_match_before_requests_are_admitted() {
    let other = "0".repeat(64);
    assert_ne!(CONTRACT_ID, other);
    for hello_id in [None, Some(CONTRACT_ID), Some(other.as_str())] {
        for ready_id in [None, Some(CONTRACT_ID), Some(other.as_str())] {
            let hello = ClientMessage::Hello(Box::new(Hello {
                version: VERSION,
                contract_id: hello_id.map(str::to_owned),
            }));
            let hello = decode_hello(body(encode_client(&hello).unwrap())).unwrap();
            assert_eq!(hello.contract_id.as_deref(), hello_id);
            assert_eq!(
                require_contract_id(hello_id).is_ok(),
                hello_id == Some(CONTRACT_ID)
            );
            let mut exchange = Exchange::default();
            exchange
                .client(&ClientMessage::Hello(Box::new(hello)))
                .unwrap();
            let ready = ServerMessage::Ready(Box::new(Ready {
                version: VERSION,
                backend_instance: uuid::Uuid::from_u128(1),
                contract_id: ready_id.map(str::to_owned),
            }));
            let ready = decode_admission(body(encode_server(&ready).unwrap())).unwrap();
            let matches = hello_id.is_some() && hello_id == ready_id;
            assert_eq!(exchange.server(&ready).is_ok(), matches);
            assert_eq!(exchange.client(&activate()).is_ok(), matches);
        }
    }
    for id in [None, Some(other.as_str())] {
        let error = require_contract_id(id).unwrap_err();
        assert_eq!(error.code, "incompatible");
        assert_eq!(error.retry, Retry::No);
        let mut exchange = Exchange::default();
        exchange
            .client(&ClientMessage::Hello(Box::new(Hello {
                version: VERSION,
                contract_id: id.map(str::to_owned),
            })))
            .unwrap();
        exchange
            .server(&ServerMessage::Refused(Box::new(error)))
            .unwrap();
        exchange.eof().unwrap();
    }
}

#[test]
fn request_arguments_are_not_admitted_before_exact_ready() {
    let mut exchange = Exchange::default();
    assert!(exchange.client(&activate()).is_err());
    assert_eq!(exchange.phase(), Phase::Poisoned);
    let mut exchange = Exchange::default();
    exchange
        .client(&ClientMessage::Hello(Box::new(Hello {
            contract_id: Some(newport_protocol::desktop::CONTRACT_ID.into()),
            version: VERSION,
        })))
        .unwrap();
    assert!(
        exchange
            .server(&ServerMessage::Ready(Box::new(Ready {
                contract_id: Some(newport_protocol::desktop::CONTRACT_ID.into()),
                version: 2,
                backend_instance: uuid::Uuid::nil()
            })))
            .is_err()
    );
    assert_eq!(exchange.phase(), Phase::Poisoned);
}
#[test]
fn mandatory_false_is_distinct_from_an_absent_toggle() {
    let value = request(Operation::Host(Box::new(h::Operation::SetLaunchAtLogin(
        Box::new(h::SetLaunchAtLoginArgs { enabled: false }),
    ))));
    let decoded = decode_client(body(encode_client(&value).unwrap()), false).unwrap();
    let ClientMessage::Request(v) = decoded else {
        panic!()
    };
    let Operation::Host(v) = v.operation else {
        panic!()
    };
    let h::Operation::SetLaunchAtLogin(v) = *v else {
        panic!()
    };
    assert!(!v.enabled);
}

#[test]
fn boxed_empty_and_scalar_operations_preserve_their_payloads() {
    use crate::desktop::events;

    let account = request(Operation::Account(Box::new(a::Operation::Status(
        Box::new(Present {}),
    ))));
    let decoded = decode_client(body(encode_client(&account).unwrap()), false).unwrap();
    let ClientMessage::Request(decoded) = decoded else {
        panic!("expected request")
    };
    let Operation::Account(operation) = decoded.operation else {
        panic!("expected account operation")
    };
    assert!(matches!(*operation, a::Operation::Status(_)));

    let events = request(Operation::Events(Box::new(events::Operation::Poll(
        Box::new(events::PollArgs { after: u64::MAX }),
    ))));
    let decoded = decode_client(body(encode_client(&events).unwrap()), false).unwrap();
    let ClientMessage::Request(decoded) = decoded else {
        panic!("expected request")
    };
    let Operation::Events(operation) = decoded.operation else {
        panic!("expected events operation")
    };
    let events::Operation::Poll(args) = *operation;
    assert_eq!(args.after, u64::MAX);
}
#[test]
fn absent_toggle_never_becomes_a_revocation() {
    let frame = append_field(
        Vec::new(),
        2,
        &append_field(vec![8, 7], 26, &append_field(Vec::new(), 4, &[])),
    );
    assert!(decode_client(Bytes::from(frame), false).is_err());
}
#[test]
fn duplicate_nested_known_fields_and_mixed_operations_are_rejected() {
    let host = append_field(Vec::new(), 4, &[8, 0, 8, 1]);
    let frame = append_field(Vec::new(), 2, &append_field(vec![8, 7], 26, &host));
    assert!(decode_client(Bytes::from(frame), false).is_err());
    let mixed = append_field(append_field(Vec::new(), 1, &[]), 6, &[]);
    let frame = append_field(Vec::new(), 2, &append_field(vec![8, 7], 26, &mixed));
    assert!(decode_client(Bytes::from(frame), false).is_err());
}
#[test]
fn unknown_bounded_extensions_do_not_replace_the_operation() {
    let bytes = append_field(proto_body(&activate()).to_vec(), 300, b"extension");
    assert!(decode_client(Bytes::from(bytes), false).is_ok());
    let frame = append_field(Vec::new(), 2, &append_field(vec![8, 7], 30, &[]));
    assert!(decode_client(Bytes::from(frame), false).is_err());
}
#[test]
fn zero_and_duplicate_request_identifiers_are_rejected() {
    let host = append_field(Vec::new(), 1, &[]);
    for id in [vec![8, 0], vec![8, 7, 8, 7]] {
        let frame = append_field(Vec::new(), 2, &append_field(id, 26, &host));
        assert!(decode_client(Bytes::from(frame), false).is_err());
    }
}
#[test]
fn request_ids_do_not_wrap() {
    let mut ids = IdSequence::after(u64::MAX - 1);
    assert_eq!(ids.allocate().unwrap().get(), u64::MAX);
    assert!(ids.allocate().is_err());
    assert!(ids.allocate().is_err());
}
#[test]
fn response_operation_and_closure_are_independent_checks() {
    let ClientMessage::Request(request) = activate() else {
        panic!()
    };
    let wrong = Response {
        id: request.id,
        outcome: Outcome::Completed(Box::new(Reply::Host(Box::new(h::Reply::CheckForUpdates(
            Box::new(Present {}),
        ))))),
    };
    assert!(wrong.validate_for(&request).is_err());
    let mut exchange = Exchange::default();
    exchange
        .client(&ClientMessage::Hello(Box::new(Hello {
            contract_id: Some(newport_protocol::desktop::CONTRACT_ID.into()),
            version: VERSION,
        })))
        .unwrap();
    exchange
        .server(&ServerMessage::Ready(Box::new(Ready {
            contract_id: Some(newport_protocol::desktop::CONTRACT_ID.into()),
            version: VERSION,
            backend_instance: uuid::Uuid::nil(),
        })))
        .unwrap();
    exchange.client(&ClientMessage::Request(request)).unwrap();
    let response = Response {
        id: RequestId::new(7).unwrap(),
        outcome: Outcome::Completed(Box::new(Reply::Host(Box::new(h::Reply::Activate(
            Box::new(Present {}),
        ))))),
    };
    exchange
        .server(&ServerMessage::Response(Box::new(response)))
        .unwrap();
    assert_eq!(exchange.phase(), Phase::Closing);
    exchange.eof().unwrap();
    assert_eq!(exchange.phase(), Phase::Ended);
    assert!(
        exchange
            .server(&ServerMessage::Refused(Box::default()))
            .is_err()
    );
}
#[test]
fn native_paths_preserve_non_unicode_names() {
    for path in [
        f::NativePath::Unix(Box::new(Bytes::from_static(b"/tmp/\xff"))),
        f::NativePath::Windows(Box::new(vec![67, 58, 92, 0xd800])),
    ] {
        let encoded = path.to_proto().unwrap();
        let decoded = f::NativePath::from_proto(encoded).unwrap();
        match (path, decoded) {
            (f::NativePath::Unix(a), f::NativePath::Unix(b)) => assert_eq!(a, b),
            (f::NativePath::Windows(a), f::NativePath::Windows(b)) => assert_eq!(a, b),
            _ => panic!(),
        }
    }
    assert!(utf16(Bytes::from_static(&[1])).is_err());
    assert!(
        f::NativePath::Unix(Box::new(Bytes::from_static(b"/x\0")))
            .validate()
            .is_err()
    );
}
#[test]
fn secrets_are_redacted_through_reply_debug() {
    let value = a::RemoteLicense {
        token: crate::common::Secret {
            value: Bytes::from_static(b"token-never-log"),
        },
        machine: "machine".into(),
        trusted_time: 1,
        expires_at: 2,
    };
    assert!(!format!("{value:?}").contains("token-never-log"));
}
#[test]
fn terminal_failure_preserves_code_and_retry() {
    let failure = Failure {
        error: crate::common::Error {
            code: "holder_outcome_unknown".into(),
            message: "Completion was not observed".into(),
            retry: Retry::Unknown,
        },
        detail: None,
    };
    let message = s::TerminalServer::Event(Box::new(s::TerminalEvent::Error(Box::new(failure))));
    let decoded = decode_terminal_server(body(encode_terminal_server(&message).unwrap())).unwrap();
    let s::TerminalServer::Event(event) = decoded else {
        panic!()
    };
    let s::TerminalEvent::Error(failure) = *event else {
        panic!()
    };
    assert_eq!(failure.error.code, "holder_outcome_unknown");
    assert_eq!(failure.error.retry, Retry::Unknown);
}
#[test]
fn opaque_completion_requires_matching_kind_and_transport_eof() {
    let mut flow = OpaqueFlow::new(OpaqueKind::Command);
    let done =
        s::OpaqueServer::Completion(Box::new(s::Completion::Lsp(Box::new(s::LspCompletion {
            error: None,
        }))));
    assert!(flow.server(&done).is_err());
    assert!(flow.eof().is_err());
    let mut flow = OpaqueFlow::new(OpaqueKind::Lsp);
    flow.server(&done).unwrap();
    flow.eof().unwrap();
    assert!(flow.server(&done).is_err());
}
#[test]
fn terminal_flow_rejects_excess_credit_and_future_epochs() {
    let mut flow = TerminalFlow::default();
    assert!(flow.acknowledge(1).is_err());
    let mut flow = TerminalFlow::default();
    assert!(flow.input(1, 1).is_err());
    let mut flow = TerminalFlow::default();
    flow.depart().unwrap();
    assert!(!flow.input(0, 1).unwrap());
    flow.taken(1).unwrap();
    for _ in 0..4 {
        flow.input(1, MAX_DATA).unwrap();
    }
    assert_eq!(flow.input_available(), 0);
    assert!(flow.input(1, 1).is_err());
}
struct PartialWrite {
    count: usize,
}
impl Write for PartialWrite {
    fn write(&mut self, value: &[u8]) -> io::Result<usize> {
        if self.count != 0 {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed"));
        }
        self.count += 1;
        Ok(value.len().min(1))
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
#[test]
fn a_partial_write_permanently_retires_the_writer() {
    let mut writer = Writer::new(PartialWrite { count: 0 });
    assert!(writer.client(&activate()).is_err());
    assert!(matches!(writer.client(&activate()), Err(Error::Retired)));
}

#[test]
fn document_bytes_are_not_charged_as_metadata_and_must_be_utf8() {
    let document = f::Document {
        path: "/tmp/document".into(),
        content: "x".repeat(MAX_DOCUMENT),
        revision: "revision".into(),
        page: None,
        cached: false,
    };
    let wire = document.to_proto().unwrap();
    let mut encoded = Vec::new();
    protobuf::write_message(
        &mut encoded,
        &wire,
        &Limits {
            message: MAX_SAVE_REQUEST,
            metadata: 4096,
        },
    )
    .unwrap();
    let mut bad = wire;
    bad.content = Some(Bytes::from_static(&[0xff]));
    assert!(matches!(f::Document::from_proto(bad), Err(Error::Utf8(_))));
}
#[test]
fn admission_sizes_choose_large_permits_before_allocating() {
    assert_eq!(
        request_allocation(MAX_REQUEST as u64).unwrap(),
        Allocation::Ordinary
    );
    assert_eq!(
        request_allocation(MAX_REQUEST as u64 + 1).unwrap(),
        Allocation::Large
    );
    assert!(request_allocation(MAX_SAVE_REQUEST as u64 + 1).is_err());
}
#[test]
fn fragmented_and_coalesced_frames_preserve_reader_tail() {
    let first = encode_client(&activate()).unwrap();
    let mut joined = first.clone();
    joined.extend_from_slice(&first);
    let mut decoder = protobuf::FrameDecoder::new(MAX_REQUEST);
    let mut prefix = &joined[..1];
    assert!(decoder.consume(&mut prefix).unwrap().is_none());
    let mut tail = &joined[1..];
    let frame = decoder.consume(&mut tail).unwrap().unwrap();
    assert_eq!(frame.into_bytes().as_ref(), first.as_slice());
    let frame = decoder.consume(&mut tail).unwrap().unwrap();
    assert!(decode_client(frame.body(), false).is_ok());
    assert!(tail.is_empty());
    decoder.finish().unwrap();
}
#[test]
fn metadata_and_invalid_bool_integers_are_rejected() {
    let host = append_field(Vec::new(), 4, &[8, 2]);
    let frame = append_field(Vec::new(), 2, &append_field(vec![8, 7], 26, &host));
    assert!(decode_client(Bytes::from(frame), false).is_err());
    let value = h::InstallUpdateArgs {
        confirmed_version: "x".repeat(MAX_TEXT + 1),
    };
    assert!(value.validate().is_err());
}

#[test]
fn ssh_resolve_round_trips_and_rejects_port_zero() {
    use crate::desktop::connection as c;
    let request = crate::desktop::Request {
        id: RequestId::new(1).unwrap(),
        operation: Operation::Connection(Box::new(c::Operation::SshResolve(Box::new(
            c::SshResolveArgs {
                host: "app.box.me.gw".into(),
            },
        )))),
    };
    let message = ClientMessage::Request(Box::new(request.clone()));
    decode_client(body(encode_client(&message).unwrap()), false).unwrap();
    let resolved = c::SshResolved {
        user: Some("ops".into()),
        port: Some(2222),
        proxy_command: Some("gateway ssh --stdio --suffix gw %h".into()),
        ..c::SshResolved::default()
    };
    let reply = |value: c::SshResolved| {
        ServerMessage::Response(Box::new(Response {
            id: request.id,
            outcome: Outcome::Completed(Box::new(Reply::Connection(Box::new(
                c::Reply::SshResolve(Box::new(value)),
            )))),
        }))
    };
    let decoded = decode_server_for(
        body(encode_server_for(&reply(resolved.clone()), &request).unwrap()),
        &request,
    )
    .unwrap();
    let ServerMessage::Response(response) = decoded else {
        panic!("response");
    };
    let Outcome::Completed(value) = response.outcome else {
        panic!("completed");
    };
    let Reply::Connection(value) = *value else {
        panic!("connection");
    };
    let c::Reply::SshResolve(value) = *value else {
        panic!("ssh resolve");
    };
    assert_eq!(*value, resolved);
    assert!(
        encode_server_for(
            &reply(c::SshResolved {
                port: Some(0),
                ..resolved
            }),
            &request
        )
        .is_err()
    );
}
