use super::*;

fn sample() -> Snapshot {
    let mut value = crate::monitor::ports::discovery::tests::snapshot();
    value.scope = Scope {
        network: Some(Namespace {
            device: 4,
            inode: 9,
        }),
        processes: Some(Namespace {
            device: 4,
            inode: 10,
        }),
        boot_id: Some("1d8731a3-373f-44dc-b00c-e68f7c454df4".into()),
    };
    value.processes[1].name = Some("sshd".into());
    value.processes[1].executable_name = Some("sshd".into());
    value.processes[1].details = Some(Details {
        executable: Some(ObservedPath {
            value: "/usr/sbin/sshd".into(),
            deleted: true,
            lossy: false,
        }),
        directory: Some(ObservedPath {
            value: "/srv/project".into(),
            deleted: false,
            lossy: true,
        }),
        arguments: vec!["sshd".into(), "-D".into()],
        project: Some(Project {
            root: "/srv/project".into(),
            marker: ".git".into(),
            worktree: Some("/srv/project".into()),
            branch: Some("main".into()),
            main: Some(true),
        }),
    });
    value.processes[1].metadata = Report::complete();
    value.services[0].manager = Manager::User { uid: 0 };
    value.services[0].main_pid = Some(400);
    value.sockets[0].services.push(Association {
        service: 1,
        relation: ServiceRelation::Holder { process: 2 },
    });
    value.publications.push(Publication {
        engine: Some("local".into()),
        container: "container-id".into(),
        name: "web".into(),
        image: Some("test/web:latest".into()),
        state: ContainerState::Running,
        host: Endpoint {
            address: "fe80::1".parse().unwrap(),
            port: 8080,
            interface: Some(3),
        },
        target_port: 80,
        target_address: Some("172.16.0.2".parse().unwrap()),
        compose: Some(Compose {
            project: Some("test".into()),
            service: Some("web".into()),
            directory: Some("/srv/project".into()),
            directory_project: None,
        }),
    });
    value.coverage.containers = Report::complete();
    value.container_engine = Some("local".into());
    value.coverage.elevation = Report {
        outcome: Outcome::Unavailable,
        issues: vec![Issue {
            kind: IssueKind::PermissionDenied,
            count: 1,
            message: "Descriptors restricted".into(),
        }],
    };
    value
}

#[test]
fn shared_holders_service_relations_and_publications_round_trip() {
    let expected = sample();
    let frame = encode(&expected).unwrap();
    assert_eq!(read(&mut frame.as_slice()).unwrap(), expected);
    let mut output = Vec::new();
    write(&mut output, &expected).unwrap();
    assert_eq!(output, frame);
}

#[test]
fn unknown_engine_identity_roundtrips_only_as_incomplete_publication_evidence() {
    let mut value = sample();
    value.container_engine = None;
    value.publications[0].engine = None;
    value.coverage.containers = Report {
        outcome: Outcome::Partial,
        issues: vec![Issue {
            kind: IssueKind::Unsupported,
            count: 1,
            message: "Stable engine identity unavailable".into(),
        }],
    };
    assert_eq!(
        read(&mut encode(&value).unwrap().as_slice()).unwrap(),
        value
    );
    assert!(!value.publications_complete_for("local"));
    for engine in [
        Some("local".into()),
        Some(String::new()),
        Some("bad\nidentity".into()),
    ] {
        let mut invalid = value.clone();
        invalid.publications[0].engine = engine.clone();
        assert!(decode(invalid.to_wire().encode_to_vec().into()).is_err());
        invalid.container_engine = engine;
        if invalid.container_engine.as_deref() != Some("local") {
            assert!(decode(invalid.to_wire().encode_to_vec().into()).is_err());
        }
    }
    let mut mismatch = value.clone();
    mismatch.container_engine = Some("local".into());
    assert!(decode(mismatch.to_wire().encode_to_vec().into()).is_err());
    value.coverage.containers = Report::complete();
    assert!(decode(value.to_wire().encode_to_vec().into()).is_err());
    value.publications.clear();
    assert!(decode(value.to_wire().encode_to_vec().into()).is_err());
}

#[test]
fn empty_inventory_sources_round_trip() {
    let mut value = sample();
    value.sockets.clear();
    value.diagnostics = Diagnostics {
        ipv4_source: Some(SocketSource::Proc),
        ipv6_source: Some(SocketSource::Diagnostic),
    };
    assert_eq!(
        read(&mut encode(&value).unwrap().as_slice()).unwrap(),
        value
    );
    let mut missing = value.to_wire();
    missing.diagnostics = None;
    assert!(decode(missing.encode_to_vec().into()).is_err());
    let mut unknown = value.to_wire();
    unknown.diagnostics.as_mut().unwrap().ipv4_source = Some(100);
    assert!(decode(unknown.encode_to_vec().into()).is_err());
}

#[test]
fn absence_and_zero_values_remain_distinct() {
    let value = sample();
    let mut wire = value.to_wire();
    assert_eq!(wire.sockets[0].uid, Some(0));
    assert_eq!(wire.sockets[0].cookie, Some(0));
    assert!(wire.sockets[0].ipv6_only.is_none());
    wire.sockets[0].uid = None;
    assert!(decode(wire.encode_to_vec().into()).is_err());
    wire = value.to_wire();
    wire.coverage = None;
    assert!(decode(wire.encode_to_vec().into()).is_err());
    wire = value.to_wire();
    wire.processes[0].identity.as_mut().unwrap().start_ticks = None;
    assert!(decode(wire.encode_to_vec().into()).is_err());
}

#[test]
fn successful_empty_container_inventory_retains_its_engine_over_protobuf() {
    let mut value = sample();
    value.publications.clear();
    let frame = encode(&value).unwrap();
    let decoded = read(&mut frame.as_slice()).unwrap();
    assert!(decoded.publications_complete_for("local"));
    assert!(!decoded.publications_complete_for("other"));
    let mut missing = value.to_wire();
    missing.container_engine = None;
    assert!(decode(missing.encode_to_vec().into()).is_err());
}

#[test]
fn unknown_enums_and_narrowing_ports_fail_checked_conversion() {
    let mut value = sample().to_wire();
    value.sockets[0].source = 100;
    assert!(decode(value.encode_to_vec().into()).is_err());
    value = sample().to_wire();
    value.sockets[0].endpoint.as_mut().unwrap().port = 65536;
    assert!(decode(value.encode_to_vec().into()).is_err());
    value = sample().to_wire();
    value.sockets[0].endpoint.as_mut().unwrap().address = Bytes::from_static(&[1, 2, 3]);
    assert!(decode(value.encode_to_vec().into()).is_err());
}

#[test]
fn duplicate_fields_are_rejected_before_last_one_wins_decoding() {
    let mut body = sample().to_wire().encode_to_vec();
    body.extend_from_slice(&[8, 1]);
    assert!(matches!(
        decode(body.into()),
        Err(Error::Invalid(Invalid("encoded collection count")))
    ));
}

#[test]
fn diagnostic_fields_reject_duplicates_and_wide_enum_aliases_before_decoding() {
    let mut wire = sample().to_wire();
    wire.diagnostics = None;
    let mut duplicate = wire.encode_to_vec();
    duplicate.extend(message(11, &[24, 1, 24, 2]));
    assert!(decode(duplicate.into()).is_err());
    let mut diagnostics = vec![24];
    prost::encoding::encode_varint((1u64 << 32) | 1, &mut diagnostics);
    let mut wide = wire.encode_to_vec();
    wide.extend(message(11, &diagnostics));
    assert!(decode(wide.into()).is_err());
}

fn message(tag: u32, body: &[u8]) -> Vec<u8> {
    let mut output = Vec::new();
    prost::encoding::encode_varint(u64::from(tag) * 8 + 2, &mut output);
    prost::encoding::encode_varint(u64::try_from(body.len()).unwrap(), &mut output);
    output.extend_from_slice(body);
    output
}

#[test]
fn ambiguous_manager_and_service_relation_fail_before_decoding() {
    // Service.manager contains both system and user-UID alternatives.
    let manager = [10, 0, 16, 0];
    let body = message(7, &message(2, &manager));
    assert!(matches!(
        decode(body.into()),
        Err(Error::Invalid(Invalid("duplicate operation")))
    ));
    // Socket.services contains both socket-cgroup and holder alternatives.
    let relation = [18, 0, 24, 1];
    let body = message(5, &message(13, &relation));
    assert!(matches!(
        decode(body.into()),
        Err(Error::Invalid(Invalid("duplicate operation")))
    ));
}

#[test]
fn excessive_records_and_address_lengths_fail_before_generated_allocation() {
    let mut value = sample().to_wire();
    // Empty records isolate the collection limit from the aggregate field limit.
    value.sockets = vec![Default::default(); MAX_LISTENERS + 1];
    assert!(matches!(
        decode(value.encode_to_vec().into()),
        Err(Error::Invalid(Invalid("encoded collection count")))
    ));
    let body = message(5, &message(2, &message(1, &[0; 17])));
    assert!(matches!(
        decode(body.into()),
        Err(Error::Invalid(Invalid("encoded text size")))
    ));
}

#[test]
fn oversized_frame_is_rejected_without_reading_its_payload() {
    let mut frame = Vec::new();
    prost::encoding::encode_varint(
        u64::try_from(SERVER_LIMITS.message + 1).unwrap(),
        &mut frame,
    );
    assert!(matches!(
        read(&mut frame.as_slice()),
        Err(Error::Frame(protobuf::FrameError::TooLarge { .. }))
    ));
}

#[test]
fn bounded_unknown_fields_do_not_change_known_observations() {
    let value = sample();
    let mut body = value.to_wire().encode_to_vec();
    body.extend(message(100, b"future"));
    assert_eq!(decode(body.into()).unwrap(), value);
}
