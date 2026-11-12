use super::*;

fn process(id: u32, pid: u32) -> Process {
    Process {
        id,
        identity: ProcessIdentity {
            pid,
            start_ticks: 1,
        },
        name: None,
        executable_name: None,
        uid: Some(0),
        account: Some("root".into()),
        details: None,
        metadata: Report::default(),
    }
}

pub(super) fn snapshot() -> Snapshot {
    Snapshot {
        generation: 1,
        sockets: vec![Socket {
            id: 1,
            endpoint: Endpoint {
                address: "0.0.0.0".parse().unwrap(),
                port: 22,
                interface: None,
            },
            source: SocketSource::Diagnostic,
            inode: 7,
            cookie: Some(0),
            uid: 0,
            account: Some("root".into()),
            ipv6_only: None,
            cgroup: Some(Cgroup {
                id: 2,
                path: Some("/system.slice/ssh.socket".into()),
            }),
            holders: vec![1, 2],
            holder_state: HolderState::Observed,
            holders_truncated: false,
            services: vec![Association {
                service: 1,
                relation: ServiceRelation::SocketCgroup,
            }],
        }],
        processes: vec![process(1, 1), process(2, 100)],
        services: vec![Service {
            id: 1,
            manager: Manager::System,
            unit: "ssh.socket".into(),
            description: None,
            control_group: "/system.slice/ssh.socket".into(),
            main_pid: None,
            triggers: vec!["ssh.service".into()],
        }],
        coverage: Coverage {
            ipv4: Report::complete(),
            processes: Report::complete(),
            services: Report::complete(),
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn shared_socket_keeps_both_holders_and_zero_uid_and_cookie() {
    snapshot().validate().unwrap();
}

#[test]
fn service_attribution_does_not_require_or_invent_process_holders() {
    let mut value = snapshot();
    value.sockets[0].holders.clear();
    value.sockets[0].holder_state = HolderState::Unresolved;
    value.processes.clear();
    value.services[0].main_pid = Some(200);
    value.validate().unwrap();
    assert!(value.sockets[0].holders.is_empty());
}

#[test]
fn references_cannot_point_to_missing_or_unrelated_processes() {
    let mut value = snapshot();
    value.processes.pop();
    assert!(value.validate().is_err());
    value = snapshot();
    value.processes.push(process(3, 200));
    value.sockets[0].services[0].relation = ServiceRelation::Holder { process: 3 };
    assert!(value.validate().is_err());
    value.sockets[0].services[0].relation = ServiceRelation::Holder { process: 2 };
    value.validate().unwrap();
}

#[test]
fn duplicate_descriptors_do_not_become_duplicate_holders() {
    let mut value = snapshot();
    value.sockets[0].holders.push(1);
    assert!(value.validate().is_err());
}

#[test]
fn pid_reuse_is_a_different_identity_but_duplicate_identity_is_rejected() {
    let mut value = snapshot();
    let mut reused = value.processes[1].clone();
    reused.id = 3;
    value.processes.push(reused);
    assert!(value.validate().is_err());
    value.processes[2].identity.start_ticks += 1;
    value.validate().unwrap();
}

#[test]
fn incomplete_inventory_cannot_prove_absence_but_denied_enrichment_does_not_hide_ports() {
    let mut value = snapshot();
    let address = value.sockets[0].endpoint.address;
    let restricted = Report {
        outcome: Outcome::Partial,
        issues: vec![Issue {
            kind: IssueKind::PermissionDenied,
            count: 2,
            message: "Process descriptors are restricted".into(),
        }],
    };
    value.coverage.processes = restricted.clone();
    value.validate().unwrap();
    assert!(value.coverage.socket_inventory_complete(address));
    assert!(
        !value
            .coverage
            .socket_inventory_complete("::".parse().unwrap())
    );
    value.coverage.ipv4 = restricted;
    assert!(!value.coverage.socket_inventory_complete(address));
    value.coverage.ipv4 = Report::default();
    assert!(!value.coverage.socket_inventory_complete(address));
}

#[test]
fn nat_publication_does_not_require_a_host_socket() {
    let mut value = Snapshot {
        generation: 1,
        ..Default::default()
    };
    value.publications.push(Publication {
        engine: Some("local".into()),
        container: "container-id".into(),
        name: "web".into(),
        image: None,
        state: ContainerState::Running,
        host: Endpoint {
            address: "0.0.0.0".parse().unwrap(),
            port: 8080,
            interface: None,
        },
        target_port: 80,
        target_address: None,
        compose: None,
    });
    assert!(value.validate().is_err());
    assert!(value.sockets.is_empty());
    assert!(!value.publications_complete_for("local"));
    value.coverage.containers = Report::complete();
    value.container_engine = Some("local".into());
    value.validate().unwrap();
    assert!(value.publications_complete_for("local"));
    value.publications[0].engine = Some("another".into());
    assert!(value.validate().is_err());
    value.publications[0].engine = Some("local".into());
    value.coverage.containers = Report::not_applicable();
    assert!(value.validate().is_err());
}

#[test]
fn an_empty_engine_inventory_only_establishes_absence_for_that_engine() {
    let mut value = Snapshot {
        generation: 1,
        container_engine: Some("replacement".into()),
        ..Snapshot::default()
    };
    value.coverage.containers = Report::complete();
    value.validate().unwrap();
    assert!(value.publications_complete_for("replacement"));
    assert!(!value.publications_complete_for("previous"));
    value.container_engine = None;
    assert!(value.validate().is_err());
    value.coverage.containers = Report::not_applicable();
    value.validate().unwrap();
    assert!(!value.publications_complete_for("previous"));
}

#[test]
fn corrupt_identity_and_scope_are_rejected() {
    let mut value = snapshot();
    value.sockets[0].cookie = Some(u64::MAX);
    assert!(value.validate().is_err());
    value = snapshot();
    value.sockets[0].ipv6_only = Some(true);
    assert!(value.validate().is_err());
    value = snapshot();
    value.scope.boot_id = Some("not-a-boot-id".into());
    assert!(value.validate().is_err());
    value = snapshot();
    value.services[0].control_group = "/system.slice/../ssh.socket".into();
    assert!(value.validate().is_err());
}

#[test]
fn reports_require_a_reason_for_failed_or_partial_observations() {
    let mut value = snapshot();
    value.coverage.ipv4.outcome = Outcome::Partial;
    assert!(value.validate().is_err());
    value.coverage.ipv4.issues.push(Issue {
        kind: IssueKind::Interrupted,
        count: 1,
        message: "Dump interrupted".into(),
    });
    value.validate().unwrap();
}

#[test]
fn record_and_text_limits_apply_before_an_observation_can_be_sent() {
    let mut value = snapshot();
    value.sockets[0].account = Some("x".repeat(MAX_FIELD + 1));
    assert!(value.validate().is_err());
    value = snapshot();
    value.sockets[0].holders = (1..=u32::try_from(MAX_HOLDERS + 1).unwrap()).collect();
    assert!(value.validate().is_err());
}
