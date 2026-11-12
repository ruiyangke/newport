use super::super::{
    compose as co, containers as ct, file_watch as fw, metrics as mt, ports::discovery as pt,
    services as sv, stable_display as sd,
};
use super::*;

#[test]
fn reserved_request_and_response_fields_are_not_operations() {
    assert!(decode_request(Bytes::from_static(&[10, 4, 8, 1, 58, 0])).is_err());
    let request = Request::Ports(Default::default());
    assert!(decode_reply(Bytes::from_static(&[10, 4, 8, 7, 58, 0]), &request).is_err());
}
use prost::Message;

#[test]
fn active_discovery_reply_bounds_details_and_holder_references_across_records() {
    use crate::encoding::protobuf::generated::newport::discovery as pb;
    let details = pb::Snapshot {
        processes: vec![
            pb::Process {
                details: Some(pb::Details::default()),
                ..Default::default()
            };
            pt::MAX_DETAILS + 1
        ],
        ..Default::default()
    };
    let holders = pb::Snapshot {
        sockets: vec![
            pb::Socket {
                holders: vec![1; pt::MAX_HOLDERS],
                ..Default::default()
            };
            pt::MAX_HOLDER_REFERENCES / pt::MAX_HOLDERS + 1
        ],
        ..Default::default()
    };
    for snapshot in [details, holders] {
        let frame = service(9, &snapshot.encode_to_vec());
        assert!(matches!(
            decode_reply(frame.into(), &Request::Ports(Default::default())),
            Err(Error::Invalid(Invalid(
                "encoded discovery allocation count"
            )))
        ));
    }
}

fn body(frame: Vec<u8>, limits: &common::Limits) -> Bytes {
    protobuf::read_frame(&mut frame.as_slice(), limits.message)
        .unwrap()
        .unwrap()
}
fn call(request: &Request) -> Call {
    Call {
        id: 7,
        request: request.clone(),
    }
}
fn encode_request(request: &Request) -> Result<Vec<u8>, Error> {
    super::encode_request(&call(request))
}
fn decode_request(body: Bytes) -> Result<Request, Error> {
    super::decode_request(body).map(|v| v.request)
}
fn encode_reply(reply: &Reply, request: &Request) -> Result<Vec<u8>, Error> {
    super::encode_reply(
        &Response {
            id: 7,
            reply: reply.clone(),
        },
        &call(request),
    )
}
fn decode_reply(body: Bytes, request: &Request) -> Result<Reply, Error> {
    super::decode_reply(body, &call(request)).map(|v| v.reply)
}
fn service(tag: u32, payload: &[u8]) -> Vec<u8> {
    field(1, &[vec![0x08, 7], field(tag + 1, payload)].concat())
}
fn roundtrip(request: Request, reply: Reply) {
    let decoded = decode_request(body(encode_request(&request).unwrap(), &CLIENT_LIMITS)).unwrap();
    assert_eq!(decoded, request);
    let decoded = decode_reply(
        body(encode_reply(&reply, &request).unwrap(), &SERVER_LIMITS),
        &request,
    )
    .unwrap();
    assert_eq!(decoded, reply);
}
#[test]
fn operations_preserve_their_arguments_and_results() {
    roundtrip(
        Request::Metrics,
        Reply::Metrics(mt::Snapshot {
            cores: 2,
            load: [1.0, 2.0, 3.0],
            cpus: vec![mt::Core {
                id: 4,
                usage: 25.0,
                history: vec![0.0, 50.0],
            }],
            disks: vec![mt::Disk {
                available: -7,
                ..Default::default()
            }],
            processes: vec![mt::Process {
                pid: 1,
                cpu: 200.0,
                elapsed: Some(0),
                command: Some("cmd".into()),
                ..Default::default()
            }],
            ..Default::default()
        }),
    );
    roundtrip(
        Request::StableDisplay,
        Reply::StableDisplay(sd::State {
            running: true,
            viewer: Some(sd::Viewer {
                session: "session".into(),
                computer: Some("Windows".into()),
            }),
        }),
    );
    for refresh in [false, true] {
        roundtrip(
            Request::Ports(pt::Request { refresh }),
            Reply::Ports(Box::new(pt::Snapshot {
                generation: 7,
                sockets: vec![pt::Socket {
                    id: 1,
                    endpoint: pt::Endpoint {
                        address: "::1".parse().unwrap(),
                        port: 65535,
                        interface: Some(3),
                    },
                    source: pt::SocketSource::Diagnostic,
                    inode: 5,
                    cookie: Some(0),
                    uid: 0,
                    account: Some("root".into()),
                    ipv6_only: Some(true),
                    cgroup: None,
                    holders: vec![1],
                    holder_state: pt::HolderState::Observed,
                    holders_truncated: false,
                    services: Vec::new(),
                }],
                processes: vec![pt::Process {
                    id: 1,
                    identity: pt::ProcessIdentity {
                        pid: 7,
                        start_ticks: 8,
                    },
                    name: Some("app".into()),
                    executable_name: None,
                    uid: Some(0),
                    account: None,
                    details: Some(pt::Details {
                        arguments: vec!["app".into(), "literal arg".into()],
                        ..Default::default()
                    }),
                    metadata: pt::Report::complete(),
                }],
                coverage: pt::Coverage {
                    ipv6: pt::Report::complete(),
                    processes: pt::Report::complete(),
                    ..Default::default()
                },
                ..Default::default()
            })),
        );
    }
    roundtrip(
        Request::FileWatch(fw::Request {
            watch: "11111111-1111-4111-8111-111111111111".into(),
            root: "/repo".into(),
            directories: vec!["/repo".into()],
            cursor: None,
        }),
        Reply::FileWatch(fw::Reply {
            cursor: Some("cursor".into()),
            paths: vec!["/repo/a".into()],
            reset: true,
        }),
    );
    roundtrip(
        Request::Services(sv::Request::Logs(sv::LogsRequest {
            scope: sv::Scope::User,
            unit: Some("app.service".into()),
            priority: Some(sv::Priority::Warning),
            cursor: Some("cursor".into()),
            lines: Some(10),
        })),
        Reply::Services(sv::Reply::Logs(sv::Logs {
            cursor: Some("next".into()),
            entries: vec![sv::Entry {
                at: Some(-1),
                priority: Some(3),
                pid: Some(7),
                identifier: Some("app".into()),
                message: "failure".into(),
            }],
        })),
    );
    roundtrip(
        Request::Services(sv::Request::Action {
            scope: sv::Scope::System,
            unit: "app.service".into(),
            verb: sv::Verb::Restart,
        }),
        Reply::Services(sv::Reply::Action(sv::ActionReply {
            ok: false,
            needs_password: Some(true),
            command: Some("sudo systemctl restart app.service".into()),
        })),
    );
    roundtrip(
        Request::Services(sv::Request::Unit {
            scope: sv::Scope::User,
            unit: "app.service".into(),
        }),
        Reply::Services(sv::Reply::Unit(sv::UnitFile {
            text: "[Service]\n".into(),
        })),
    );
    roundtrip(
        Request::Services(sv::Request::Snapshot),
        Reply::Services(sv::Reply::Snapshot(sv::Snapshot {
            services: vec![],
            timers: vec![],
            user: sv::UserManager {
                available: false,
                reason: Some("absent".into()),
            },
            privilege: sv::Privilege::None,
        })),
    );
    let inspect = ct::ContainerInspect {
        id: "a".repeat(64),
        name: "web".into(),
        entrypoint: Some(vec![]),
        cmd: None,
        labels: [("a".into(), "b".into())].into(),
        ..Default::default()
    };
    roundtrip(
        Request::Containers(ct::Request::Inspect {
            container: "web".into(),
        }),
        Reply::Containers(ct::Reply::Inspect(Box::new(inspect))),
    );
    roundtrip(
        Request::Containers(ct::Request::Logs(ct::LogsRequest {
            target: ct::LogTarget::Project("app".into()),
            cursor: Some("cursor".into()),
            lines: Some(10),
        })),
        Reply::Containers(ct::Reply::Logs(ct::ContainerLogPage {
            entries: vec![ct::ContainerLogEntry {
                at: Some(0),
                stream: ct::LogStream::Stderr,
                message: "line\0text".into(),
                container: Some("web".into()),
            }],
            cursor: Some("next".into()),
        })),
    );
    roundtrip(
        Request::Containers(ct::Request::Snapshot),
        Reply::Containers(ct::Reply::Snapshot(ct::ContainersSnapshot {
            engine: ct::EngineInfo::default(),
            containers: vec![ct::Container {
                id: "a".repeat(64),
                name: "web".into(),
                state: ct::ContainerState::Running,
                health: Some(ct::ContainerHealth::Healthy),
                cpu: Some(300.0),
                ports: vec![ct::ContainerPort {
                    container_port: 80,
                    protocol: ct::PortProtocol::Tcp,
                    ..Default::default()
                }],
                ..Default::default()
            }],
            truncated: true,
        })),
    );
    for request in [
        ct::Request::Action {
            container: "web".into(),
            verb: ct::ContainerVerb::Restart,
        },
        ct::Request::ProjectAction {
            project: "app".into(),
            verb: ct::ProjectVerb::Start,
            expected_ids: vec!["a".repeat(64)],
        },
        ct::Request::Remove {
            containers: vec!["web".into()],
        },
        ct::Request::Prune {
            what: ct::PruneTarget::BuildCache,
        },
        ct::Request::Disk {
            part: Some(ct::DiskPart::Volumes),
        },
        ct::Request::Disk { part: None },
    ] {
        let reply = match request {
            ct::Request::Action { .. } => ct::Reply::Action(ct::Done { ok: true }),
            ct::Request::ProjectAction { .. } => ct::Reply::ProjectAction(ct::Done { ok: true }),
            ct::Request::Remove { .. } => ct::Reply::Removed(ct::Removed { removed: 1 }),
            ct::Request::Prune { .. } => ct::Reply::Pruned(ct::Pruned {
                removed: 3,
                reclaimed: u64::MAX,
            }),
            _ => ct::Reply::Disk(ct::DockerDisk {
                images: vec![],
                volumes: vec![],
                totals: ct::DockerTotals::default(),
                volumes_measured: false,
            }),
        };
        roundtrip(Request::Containers(request), Reply::Containers(reply));
    }
    for action in [
        co::Action::Up {
            services: vec!["web".into()],
            build: true,
            pull: true,
            force_recreate: true,
        },
        co::Action::Down {
            volumes: true,
            remove_orphans: true,
        },
        co::Action::Pull { services: vec![] },
        co::Action::Build {
            services: vec!["web".into()],
            no_cache: true,
        },
        co::Action::Scale {
            service: "web".into(),
            replicas: 0,
        },
    ] {
        roundtrip(
            Request::Compose(co::Request::Run {
                project: "app".into(),
                action,
            }),
            Reply::Compose(co::Reply::Started(co::Started { job: "abc".into() })),
        );
    }
    for services in [
        None,
        Some(vec![]),
        Some(vec![co::Service {
            name: "web".into(),
            image: Some("nginx".into()),
            build: true,
            profiles: vec!["debug".into()],
        }]),
    ] {
        roundtrip(
            Request::Compose(co::Request::Project {
                project: "app".into(),
            }),
            Reply::Compose(co::Reply::Project(co::Project {
                cli: Some(co::Cli {
                    kind: co::CliKind::Plugin,
                    version: "2.29.1".into(),
                }),
                cli_error: None,
                working_dir: Some("/srv/app".into()),
                config_files: vec!["/srv/app/compose.yaml".into()],
                files: vec![co::File {
                    path: "/srv/app/compose.yaml".into(),
                    text: Some("services: {}".into()),
                    error: None,
                }],
                services,
                config_error: None,
            })),
        );
    }
    roundtrip(
        Request::Compose(co::Request::Job {
            job: "abc".into(),
            offset: 7,
        }),
        Reply::Compose(co::Reply::Job(co::Job {
            job: "abc".into(),
            project: "app".into(),
            command: "docker compose up".into(),
            state: co::State::Failed,
            exit_code: Some(-1),
            started_at: 1,
            finished_at: Some(2),
            output: "line\n".into(),
            offset: 12,
        })),
    );
    roundtrip(
        Request::Compose(co::Request::Jobs),
        Reply::Compose(co::Reply::Jobs(co::Jobs { jobs: vec![] })),
    );
    roundtrip(
        Request::Compose(co::Request::Cancel { job: "abc".into() }),
        Reply::Compose(co::Reply::Cancelled(co::Done { ok: true })),
    );
    roundtrip(
        Request::Metrics,
        Reply::Failed(common::Error {
            code: "not_ready".into(),
            message: "Try again".into(),
            retry: common::Retry::Yes,
        }),
    );
}

fn field(tag: u32, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    prost::encoding::encode_varint((u64::from(tag) << 3) | 2, &mut out);
    prost::encoding::encode_varint(body.len() as u64, &mut out);
    out.extend_from_slice(body);
    out
}
#[test]
fn malformed_shapes_fail_before_record_allocation() {
    // Many empty children must not become a large Vec of full records.
    let rows = [0x12, 0].repeat(ct::MAX_CONTAINERS + 1);
    let body = service(4, &field(1, &rows));
    assert!(matches!(
        decode_reply(body.into(), &Request::Containers(ct::Request::Snapshot)),
        Err(Error::Invalid(Invalid("encoded collection count")))
    ));
    let targets = [field(1, b"web"), field(2, b"app")].concat();
    assert!(decode_request(service(4, &field(3, &targets)).into()).is_err());
    let request = body_from_request(&Request::Metrics);
    assert!(decode_request([request.as_ref(), request.as_ref()].concat().into()).is_err());
    let duplicate = [field(1, b"app"), field(1, b"other")].concat();
    assert!(decode_request(service(5, &field(1, &duplicate)).into()).is_err());
    let zero_enum = pb::ServicesRequest {
        operation: Some(pb::services_request::Operation::Unit(pb::ServicesUnit {
            scope: 0,
            unit: "app.service".into(),
        })),
    };
    assert!(decode_request(service(3, &zero_enum.encode_to_vec()).into()).is_err());
}
fn body_from_request(request: &Request) -> Bytes {
    body(encode_request(request).unwrap(), &CLIENT_LIMITS)
}
#[test]
fn controls_wrong_results_and_oversized_frames_are_rejected() {
    use crate::worker::{ClientControl, Exit, ServerControl};
    for control in [
        crate::worker::wire::encode_client(ClientControl::InputComplete).unwrap(),
        crate::worker::wire::encode_server(&ServerControl::Exit(Exit::Status(0))).unwrap(),
    ] {
        let data = body(control, &SERVER_LIMITS);
        assert!(decode_request(data.clone()).is_err());
        assert!(decode_reply(data.clone(), &Request::Metrics).is_err());
        let mixed = [data.as_ref(), body_from_request(&Request::Metrics).as_ref()].concat();
        assert!(decode_request(mixed.into()).is_err());
    }
    let reply = Reply::Compose(co::Reply::Cancelled(co::Done { ok: true }));
    let request = Request::Compose(co::Request::Cancel { job: "abc".into() });
    let data = body(encode_reply(&reply, &request).unwrap(), &SERVER_LIMITS);
    assert!(decode_reply(data, &Request::Compose(co::Request::Jobs)).is_err());
    let mut input = Vec::new();
    prost::encoding::encode_varint((CLIENT_LIMITS.message + 1) as u64, &mut input);
    assert!(matches!(
        read_request(&mut input.as_slice()),
        Err(Error::Frame(FrameError::TooLarge { .. }))
    ));
    assert!(read_request(&mut [0x82].as_slice()).is_err());
    assert!(read_request(&mut [].as_slice()).unwrap().is_none());
}
#[test]
fn packed_and_unpacked_history_share_one_count_budget() {
    let mut core = Vec::new();
    for _ in 0..mt::MAX_HISTORY {
        core.push(0x19);
        core.extend_from_slice(&0f64.to_le_bytes());
    }
    core.extend(field(3, &0f64.to_le_bytes()));
    let snapshot = field(8, &core);
    assert!(matches!(
        decode_reply(service(1, &snapshot).into(), &Request::Metrics),
        Err(Error::Invalid(Invalid("encoded collection count")))
    ));
}

#[test]
fn aggregate_fields_and_duplicate_labels_are_bounded() {
    let empty_names = field(17, b"").repeat(sv::MAX_LIST);
    let services = field(1, &empty_names).repeat(256);
    assert!(matches!(
        decode_reply(
            service(3, &field(1, &services)).into(),
            &Request::Services(sv::Request::Snapshot)
        ),
        Err(Error::FieldBudget)
    ));
    let request = Request::Containers(ct::Request::Inspect {
        container: "web".into(),
    });
    let label = pb::Label {
        key: "a".into(),
        value: "b".into(),
    };
    let inspect = pb::ContainersContainerInspect {
        id: "a".repeat(64),
        name: "web".into(),
        labels: vec![label.clone(), label],
        ..Default::default()
    };
    let data = service(4, &field(2, &inspect.encode_to_vec()));
    assert!(matches!(
        decode_reply(data.into(), &request),
        Err(Error::Invalid(Invalid("duplicate container label")))
    ));
}
#[test]
fn enum_integers_and_booleans_do_not_wrap_into_valid_values() {
    let mut scope = vec![0x08];
    prost::encoding::encode_varint((1u64 << 32) + 1, &mut scope);
    scope.extend(field(2, b"app.service"));
    assert!(decode_request(service(3, &field(3, &scope)).into()).is_err());
    assert!(decode_reply(service(2, &[0x08, 0x02]).into(), &Request::StableDisplay).is_err());
}

#[test]
fn numbered_calls_reject_missing_stale_and_duplicate_identifiers() {
    let call = Call {
        id: u64::MAX,
        request: Request::StableDisplay,
    };
    let encoded = super::encode_request(&call).unwrap();
    assert_eq!(
        encoded,
        [
            vec![0x0f, 0x0a, 0x0d, 0x08],
            vec![0xff; 9],
            vec![0x01, 0x1a, 0x00]
        ]
        .concat()
    );
    assert_eq!(
        super::decode_request(body(encoded, &CLIENT_LIMITS)).unwrap(),
        call
    );
    let response = Response {
        id: call.id,
        reply: Reply::StableDisplay(sd::State::default()),
    };
    let encoded = super::encode_reply(&response, &call).unwrap();
    let stale = Call {
        id: 1,
        request: call.request.clone(),
    };
    assert!(super::decode_reply(body(encoded, &SERVER_LIMITS), &stale).is_err());
    assert!(
        super::encode_request(&Call {
            id: 0,
            request: Request::Metrics
        })
        .is_err()
    );
    assert!(super::decode_request(field(1, &field(2, &[])).into()).is_err());
    let duplicate = [vec![0x08, 1, 0x08, 2], field(2, &[])].concat();
    assert!(super::decode_request(field(1, &duplicate).into()).is_err());
}
#[test]
fn future_metadata_is_skipped_within_the_same_frame_budget() {
    let payload = [vec![0x08, 7], field(2, &[]), field(90, b"future")].concat();
    assert_eq!(
        decode_request(field(1, &payload).into()).unwrap(),
        Request::Metrics
    );
    let state = field(90, b"future");
    assert_eq!(
        decode_reply(service(2, &state).into(), &Request::StableDisplay).unwrap(),
        Reply::StableDisplay(sd::State::default())
    );
    let unknown = [vec![0x08, 7], field(90, b"unsupported")].concat();
    assert!(decode_request(field(1, &unknown).into()).is_err());
}
