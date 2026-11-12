use super::*;
use discovery::*;
use std::net::{Ipv4Addr, Ipv6Addr};

fn scope() -> Scope {
    Scope {
        network: Some(Namespace {
            device: 4,
            inode: 1,
        }),
        processes: Some(Namespace {
            device: 4,
            inode: 2,
        }),
        boot_id: Some("18f68fcb-71e9-47fe-a226-e9da27d82953".into()),
    }
}

fn incomplete(outcome: Outcome) -> Report {
    Report {
        outcome,
        issues: vec![Issue {
            kind: IssueKind::TimedOut,
            count: 1,
            message: "Scan deadline expired.".into(),
        }],
    }
}

fn socket(id: u32, address: std::net::IpAddr) -> Socket {
    Socket {
        id,
        endpoint: Endpoint {
            address,
            port: 8080,
            interface: None,
        },
        source: SocketSource::Diagnostic,
        inode: u64::from(id),
        cookie: Some(u64::from(id)),
        uid: 1000,
        account: None,
        ipv6_only: address.is_ipv6().then_some(true),
        cgroup: None,
        holders: Vec::new(),
        holder_state: HolderState::Unresolved,
        holders_truncated: false,
        services: Vec::new(),
    }
}

fn scan(generation: u64) -> Snapshot {
    Snapshot {
        generation,
        scope: scope(),
        sockets: vec![
            socket(1, Ipv4Addr::UNSPECIFIED.into()),
            socket(2, Ipv6Addr::UNSPECIFIED.into()),
        ],
        coverage: Coverage {
            ipv4: Report::complete(),
            ipv6: Report::complete(),
            ..Coverage::default()
        },
        ..Snapshot::default()
    }
}

fn failed(generation: u64) -> Snapshot {
    let mut snapshot = scan(generation);
    snapshot.sockets.clear();
    snapshot.coverage.ipv4 = incomplete(Outcome::Unavailable);
    snapshot.coverage.ipv6 = incomplete(Outcome::Unavailable);
    snapshot
}

fn publication(engine: &str, container: &str) -> Publication {
    Publication {
        engine: Some(engine.into()),
        container: container.into(),
        name: container.into(),
        image: None,
        state: ContainerState::Running,
        host: Endpoint {
            address: Ipv4Addr::UNSPECIFIED.into(),
            port: 8080,
            interface: None,
        },
        target_port: 80,
        target_address: None,
        compose: None,
    }
}

fn engine(snapshot: &mut Snapshot, id: &str) {
    snapshot.container_engine = Some(id.into());
    snapshot.coverage.containers = Report::complete();
}

#[test]
fn first_failed_inventory_is_an_error_but_complete_empty_is_useful() {
    let mut cache = Cache::default();
    assert!(
        matches!(cache.apply(failed(1)), Err(Error::NoInventory(snapshot)) if snapshot.coverage.ipv4.outcome == Outcome::Unavailable)
    );
    assert!(cache.snapshot().is_none());
    let mut empty = scan(2);
    empty.sockets.clear();
    assert!(cache.apply(empty).unwrap().has_current_inventory());
    let result = cache.apply(failed(3)).unwrap();
    assert!(!result.has_current_inventory());
    assert!(result.current.sockets.is_empty());
    assert_eq!(result.current.coverage.ipv4.outcome, Outcome::Unavailable);
}

#[test]
fn failed_refresh_preserves_all_old_evidence_as_stale() {
    let original = scan(1);
    let mut cache = Cache::default();
    cache.apply(original.clone()).unwrap();
    let result = cache.apply(failed(2)).unwrap();
    assert!(!result.has_current_inventory());
    assert_eq!(result.retained, [original]);
    assert_eq!(result.retention, Report::complete());
    result.validate_encoding().unwrap();
}

#[test]
fn families_remain_independent_and_process_visibility_does_not_block_absence() {
    let mut cache = Cache::default();
    cache.apply(scan(1)).unwrap();
    let mut next = failed(2);
    next.coverage.ipv4 = Report::complete();
    next.coverage.processes = incomplete(Outcome::Unavailable);
    let result = cache.apply(next).unwrap();
    assert_eq!(result.retained[0].sockets.len(), 1);
    assert!(result.retained[0].sockets[0].endpoint.address.is_ipv6());
    let mut next = failed(3);
    next.coverage.ipv6 = Report::complete();
    assert!(cache.apply(next).unwrap().retained.is_empty());
}

#[test]
fn verified_observation_replaces_old_identity_even_in_partial_scan() {
    let mut cache = Cache::default();
    cache.apply(scan(1)).unwrap();
    let mut next = scan(2);
    next.sockets.truncate(1);
    next.sockets[0].id = 99;
    next.coverage.ipv4 = incomplete(Outcome::Partial);
    next.coverage.ipv6 = incomplete(Outcome::Unavailable);
    let result = cache.apply(next).unwrap();
    assert_eq!(result.current.sockets[0].id, 99);
    assert_eq!(result.retained[0].sockets.len(), 1);
    assert!(result.retained[0].sockets[0].endpoint.address.is_ipv6());
}

#[test]
fn endpoint_and_inode_reuse_do_not_replace_stale_socket() {
    for cookie in [None, Some(100)] {
        let mut cache = Cache::default();
        cache.apply(scan(1)).unwrap();
        let mut next = scan(2);
        next.sockets[0].cookie = cookie;
        next.coverage.ipv4 = incomplete(Outcome::Partial);
        let result = cache.apply(next).unwrap();
        assert_eq!(result.retained[0].sockets.len(), 1);
        assert_eq!(result.retained[0].sockets[0].cookie, Some(1));
    }
}

#[test]
fn unknown_scope_changed_scope_and_nonmonotonic_generation_invalidate_identities() {
    for kind in 0..6 {
        let mut cache = Cache::default();
        cache.apply(scan(2)).unwrap();
        let mut next = scan(3);
        match kind {
            0 => next.scope.network = None,
            1 => next.scope.network.as_mut().unwrap().inode += 1,
            2 => next.scope.processes.as_mut().unwrap().inode += 1,
            3 => next.scope.boot_id = Some("28f68fcb-71e9-47fe-a226-e9da27d82953".into()),
            4 => next.generation = 2,
            _ => next.generation = 1,
        }
        next.coverage.ipv4 = incomplete(Outcome::Partial);
        next.coverage.ipv6 = incomplete(Outcome::Partial);
        assert!(cache.apply(next.clone()).unwrap().retained.is_empty());
        let mut failure = failed(next.generation + 1);
        failure.scope.network = None;
        assert!(matches!(cache.apply(failure), Err(Error::NoInventory(_))));
        assert!(cache.snapshot().is_none());
    }
}

#[test]
fn invalid_response_is_atomic_and_preserves_last_snapshot() {
    let mut cache = Cache::default();
    let original = cache.apply(scan(1)).unwrap().clone();
    let mut invalid = scan(2);
    invalid.sockets[0].holders.push(999);
    assert!(matches!(cache.apply(invalid), Err(Error::Invalid(_))));
    assert_eq!(cache.snapshot(), Some(&original));
}

#[test]
fn container_inventory_is_independent_and_engine_identity_prevents_false_absence() {
    let mut cache = Cache::default();
    let mut old = scan(1);
    engine(&mut old, "a");
    old.publications.push(publication("a", "web"));
    cache.apply(old).unwrap();
    let mut next = failed(2);
    engine(&mut next, "b");
    let result = cache.apply(next).unwrap();
    assert!(result.has_current_inventory());
    assert_eq!(
        result.retained[0].publications[0].engine.as_deref(),
        Some("a")
    );
    assert_eq!(result.retained[0].sockets.len(), 2);
    let mut next = failed(3);
    engine(&mut next, "a");
    let result = cache.apply(next).unwrap();
    assert!(result.retained[0].publications.is_empty());
    assert_eq!(result.retained[0].sockets.len(), 2);
}

#[test]
fn partial_publication_scan_replaces_only_same_publication() {
    let mut cache = Cache::default();
    let mut old = scan(1);
    engine(&mut old, "a");
    old.publications = vec![publication("a", "web"), publication("a", "worker")];
    cache.apply(old).unwrap();
    let mut next = scan(2);
    engine(&mut next, "a");
    next.coverage.containers = incomplete(Outcome::Partial);
    next.publications.push(publication("a", "web"));
    next.publications[0].state = ContainerState::Stopped;
    let result = cache.apply(next).unwrap();
    assert_eq!(
        result.current.publications[0].state,
        ContainerState::Stopped
    );
    assert_eq!(result.retained[0].publications.len(), 1);
    assert_eq!(result.retained[0].publications[0].container, "worker");
    assert!(result.retained[0].sockets.is_empty());
}

fn attribute(snapshot: &mut Snapshot) {
    snapshot.coverage.processes = Report::complete();
    snapshot.coverage.services = Report::complete();
    for id in 1..=3 {
        snapshot.processes.push(Process {
            id,
            identity: ProcessIdentity {
                pid: id * 100,
                start_ticks: 1,
            },
            name: Some(format!("holder-{id}")),
            executable_name: None,
            uid: None,
            account: None,
            details: None,
            metadata: Report::complete(),
        });
        snapshot.services.push(Service {
            id,
            manager: Manager::System,
            unit: format!("test-{id}.service"),
            description: None,
            control_group: format!("/system.slice/test-{id}.service"),
            main_pid: Some(9999),
            triggers: Vec::new(),
        });
    }
    snapshot.sockets[0].holders = vec![1, 2];
    snapshot.sockets[0].holder_state = HolderState::Observed;
    snapshot.sockets[0].services = vec![Association {
        service: 1,
        relation: ServiceRelation::Holder { process: 1 },
    }];
    snapshot.sockets[1].holders = vec![2, 3];
    snapshot.sockets[1].holder_state = HolderState::Observed;
    snapshot.sockets[1].services = vec![Association {
        service: 2,
        relation: ServiceRelation::Holder { process: 2 },
    }];
}

#[test]
fn retained_slices_keep_all_holder_references_and_only_reachable_metadata() {
    let mut cache = Cache::default();
    let mut old = scan(1);
    attribute(&mut old);
    cache.apply(old).unwrap();
    let mut next = failed(2);
    next.coverage.ipv4 = Report::complete();
    let result = cache.apply(next).unwrap();
    let retained = &result.retained[0];
    assert_eq!(retained.sockets[0].holders, [2, 3]);
    assert_eq!(
        retained.processes.iter().map(|p| p.id).collect::<Vec<_>>(),
        [2, 3]
    );
    assert_eq!(
        retained.services.iter().map(|s| s.id).collect::<Vec<_>>(),
        [2]
    );
    result.validate_encoding().unwrap();
}

#[test]
fn fresh_process_names_never_overwrite_stale_metadata_or_create_holders() {
    let mut cache = Cache::default();
    let mut old = scan(1);
    attribute(&mut old);
    cache.apply(old).unwrap();
    let mut next = scan(2);
    attribute(&mut next);
    next.sockets.truncate(1);
    next.sockets[0].cookie = Some(999);
    next.coverage.ipv4 = incomplete(Outcome::Partial);
    next.coverage.ipv6 = incomplete(Outcome::Unavailable);
    next.processes[1].name = Some("new-name".into());
    next.processes[1].identity.start_ticks = 2;
    let result = cache.apply(next).unwrap();
    assert_eq!(
        result.retained[0].processes[1].name.as_deref(),
        Some("holder-2")
    );
    assert_eq!(
        result.current.processes[1].name.as_deref(),
        Some("new-name")
    );
    assert!(
        !result
            .current
            .processes
            .iter()
            .any(|p| p.identity.pid == 9999)
    );
}

#[test]
fn repeated_partial_scans_bound_history_and_report_loss_without_claiming_absence() {
    let mut cache = Cache::default();
    for generation in 1..=70 {
        let mut next = scan(generation);
        next.sockets.truncate(1);
        next.sockets[0].cookie = Some(generation);
        next.coverage.ipv4 = incomplete(Outcome::Partial);
        cache.apply(next).unwrap();
    }
    let result = cache.snapshot().unwrap();
    assert_eq!(result.retained.len(), desktop::MAX_RETAINED);
    assert_eq!(result.retained.first().unwrap().generation, 69);
    assert_eq!(result.retained.last().unwrap().generation, 6);
    assert_eq!(result.retention.outcome, Outcome::Partial);
    assert_eq!(result.retention.issues[0].count, 5);
    assert_eq!(result.current.coverage.ipv4.outcome, Outcome::Partial);
    result.validate_encoding().unwrap();
    let result = cache.apply(failed(71)).unwrap();
    assert_eq!(result.retention.issues[0].count, 6);
    assert_eq!(result.current.coverage.ipv4.outcome, Outcome::Unavailable);
}

#[test]
fn total_row_budget_preserves_newest_slices() {
    let mut cache = Cache::default();
    for generation in 1..=3 {
        let mut next = scan(generation);
        next.sockets = (1..=4000)
            .map(|id| {
                let mut socket = socket(id, Ipv4Addr::LOCALHOST.into());
                socket.cookie = Some(generation * 10000 + u64::from(id));
                socket
            })
            .collect();
        next.coverage.ipv4 = incomplete(Outcome::Partial);
        cache.apply(next).unwrap();
    }
    let result = cache.snapshot().unwrap();
    assert_eq!(result.retained.len(), 1);
    assert_eq!(result.retained[0].generation, 2);
    assert_eq!(result.retention.issues[0].kind, IssueKind::BudgetExceeded);
    result.validate_encoding().unwrap();
}

#[test]
fn combined_text_is_bounded_below_desktop_frame_limit() {
    let mut cache = Cache::default();
    for generation in 1..=10 {
        let mut next = scan(generation);
        next.sockets.clear();
        engine(&mut next, "engine");
        next.coverage.containers = incomplete(Outcome::Partial);
        next.publications = (0..200)
            .map(|id| {
                let mut publication = publication("engine", &format!("{generation}-{id}"));
                publication.name = "n".repeat(4000);
                publication.image = Some("i".repeat(4000));
                publication
            })
            .collect();
        cache.apply(next).unwrap();
    }
    let result = cache.snapshot().unwrap();
    assert_eq!(result.retention.outcome, Outcome::Partial);
    assert!(result.retained.len() < 9);
    assert_eq!(result.retained[0].generation, 9);
    result.validate_encoding().unwrap();
}

#[test]
fn repeated_associations_cannot_exhaust_desktop_field_budget() {
    let mut cache = Cache::default();
    for generation in 1..=24 {
        let mut next = scan(generation);
        next.coverage.ipv4 = incomplete(Outcome::Partial);
        next.coverage.services = Report::complete();
        next.services = (1..=64)
            .map(|id| Service {
                id,
                manager: Manager::System,
                unit: format!("{id}.socket"),
                description: None,
                control_group: "/system.slice".into(),
                main_pid: None,
                triggers: Vec::new(),
            })
            .collect();
        next.sockets = (1..=50)
            .map(|id| {
                let mut socket = socket(id, Ipv4Addr::LOCALHOST.into());
                socket.cookie = Some(generation * 10000 + u64::from(id));
                socket.cgroup = Some(Cgroup { id: 10, path: None });
                socket.services = (1..=64)
                    .map(|service| Association {
                        service,
                        relation: ServiceRelation::SocketCgroup,
                    })
                    .collect();
                socket
            })
            .collect();
        cache.apply(next).unwrap();
    }
    let result = cache.snapshot().unwrap();
    assert_eq!(result.retention.outcome, Outcome::Partial);
    assert!(result.retained.len() < 23);
    result.validate_encoding().unwrap();
}
