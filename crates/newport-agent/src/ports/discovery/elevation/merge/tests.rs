use super::*;
use newport_protocol::monitor::ports::discovery::{
    Namespace, ProcessIdentity, Report, SocketSource,
};

fn process(id: u32, pid: u32) -> Process {
    Process {
        id,
        identity: ProcessIdentity {
            pid,
            start_ticks: 1,
        },
        name: Some(format!("process-{pid}")),
        executable_name: None,
        uid: Some(0),
        account: None,
        details: None,
        metadata: Report::complete(),
    }
}
fn snapshot() -> Snapshot {
    Snapshot {
        generation: 1,
        scope: Scope {
            network: Some(Namespace {
                device: 1,
                inode: 2,
            }),
            processes: Some(Namespace {
                device: 1,
                inode: 3,
            }),
            boot_id: Some("11111111-1111-4111-8111-111111111111".into()),
        },
        sockets: vec![Socket {
            id: 1,
            endpoint: Endpoint {
                address: "127.0.0.1".parse().unwrap(),
                port: 22,
                interface: None,
            },
            source: SocketSource::Diagnostic,
            inode: 100,
            cookie: Some(0),
            uid: 0,
            account: None,
            ipv6_only: None,
            cgroup: None,
            holders: vec![1],
            holder_state: HolderState::Observed,
            holders_truncated: false,
            services: Vec::new(),
        }],
        processes: vec![process(1, 1)],
        coverage: newport_protocol::monitor::ports::discovery::Coverage {
            ipv4: Report::complete(),
            ipv6: Report::complete(),
            processes: Report::complete(),
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn union_preserves_the_base_inventory_existing_metadata_and_shared_holders() {
    let mut base = snapshot();
    base.processes[0].account = Some("root".into());
    let mut extra = snapshot();
    extra.processes.push(process(2, 200));
    extra.sockets[0].holders.push(2);
    let mut later = extra.sockets[0].clone();
    later.id = 2;
    later.endpoint.port = 80;
    later.inode = 101;
    later.cookie = Some(1);
    extra.sockets.push(later);
    apply(&mut base, &extra, None).unwrap();
    assert_eq!(base.sockets.len(), 1);
    assert_eq!(base.sockets[0].holders, [1, 2]);
    assert_eq!(base.processes.len(), 2);
    assert_eq!(base.processes[0].account.as_deref(), Some("root"));
    assert_eq!(base.processes[1].identity.pid, 200);
    assert_eq!(base.coverage.elevation.outcome, Outcome::Complete);
    apply(&mut base, &extra, None).unwrap();
    assert_eq!(base.sockets[0].holders, [1, 2]);
}

#[test]
fn changed_cookie_endpoint_uid_namespace_or_generation_cannot_add_holders() {
    let base = snapshot();
    let mut template = snapshot();
    template.processes = vec![process(1, 200)];
    for change in 0..7 {
        let mut extra = template.clone();
        match change {
            0 => extra.sockets[0].cookie = Some(4),
            1 => extra.sockets[0].endpoint.port += 1,
            2 => extra.sockets[0].uid = 1000,
            3 => extra.scope.network.as_mut().unwrap().inode += 1,
            4 => extra.scope.processes = None,
            5 => extra.scope.boot_id = None,
            _ => extra.generation += 1,
        }
        let mut result = base.clone();
        let _ = apply(&mut result, &extra, None);
        assert_eq!(result.sockets, base.sockets);
        assert_eq!(result.processes, base.processes);
    }
}

#[test]
fn weak_identity_requires_a_complete_current_unambiguous_revalidation() {
    let mut base = snapshot();
    base.sockets[0].cookie = None;
    let mut extra = base.clone();
    extra.processes = vec![process(1, 200)];
    assert!(needs_revalidation(&base, &extra));
    let mut without = base.clone();
    apply(&mut without, &extra, None).unwrap();
    assert_eq!(without.processes, base.processes);
    assert_eq!(without.coverage.elevation.outcome, Outcome::Partial);
    let mut good = base.clone();
    apply(&mut good, &extra, Some(&base)).unwrap();
    assert_eq!(good.processes.len(), 2);
    for change in 0..5 {
        let mut current = base.clone();
        match change {
            0 => record(&mut current.coverage.ipv4, &Failure::Interrupted),
            1 => current.sockets[0].inode += 1,
            2 => current.scope.network.as_mut().unwrap().inode += 1,
            3 => {
                let mut duplicate = current.sockets[0].clone();
                duplicate.id = 2;
                current.sockets.push(duplicate);
            }
            _ => current.generation += 1,
        }
        let mut result = base.clone();
        apply(&mut result, &extra, Some(&current)).unwrap();
        assert_eq!(result.processes, base.processes);
    }
}

#[test]
fn revalidation_must_preserve_any_known_cookie_and_reject_inode_zero() {
    let mut base = snapshot();
    base.sockets[0].cookie = None;
    let mut extra = snapshot();
    extra.processes = vec![process(1, 200)];
    for cookie in [None, Some(1)] {
        let mut current = base.clone();
        current.sockets[0].cookie = cookie;
        let mut result = base.clone();
        apply(&mut result, &extra, Some(&current)).unwrap();
        assert_eq!(result.processes.len(), 1);
    }
    base.sockets[0].inode = 0;
    extra.sockets[0].inode = 0;
    extra.sockets[0].cookie = None;
    let mut result = base.clone();
    apply(&mut result, &extra, Some(&base)).unwrap();
    assert_eq!(result.processes.len(), 1);
}

#[test]
fn holder_limits_are_partial_and_only_referenced_processes_are_added() {
    let mut base = snapshot();
    base.processes = (1..=MAX_HOLDERS as u32).map(|id| process(id, id)).collect();
    base.sockets[0].holders = base.processes.iter().map(|p| p.id).collect();
    let mut extra = snapshot();
    extra.processes = vec![process(1, 200), process(2, 201)];
    apply(&mut base, &extra, None).unwrap();
    assert_eq!(base.processes.len(), MAX_HOLDERS);
    assert_eq!(base.sockets[0].holders.len(), MAX_HOLDERS);
    assert!(base.sockets[0].holders_truncated);
    assert_eq!(base.coverage.elevation.outcome, Outcome::Partial);
}

#[test]
fn a_reused_pid_is_not_the_same_process_identity() {
    let mut base = snapshot();
    let mut extra = snapshot();
    extra.processes[0].identity.start_ticks += 1;
    apply(&mut base, &extra, None).unwrap();
    assert_eq!(base.processes.len(), 2);
    assert_ne!(base.processes[0].identity, base.processes[1].identity);
}

#[test]
fn invalid_enrichment_does_not_partially_modify_the_base() {
    let mut base = snapshot();
    let original = base.clone();
    let mut extra = snapshot();
    extra.sockets[0].holders.push(99);
    assert!(apply(&mut base, &extra, None).is_err());
    assert_eq!(base, original);
}

#[test]
fn missing_names_can_be_enriched_without_overwriting_detailed_or_conflicting_observations() {
    let mut base = snapshot();
    base.processes[0].name = None;
    let extra = snapshot();
    apply(&mut base, &extra, None).unwrap();
    assert_eq!(base.processes[0].name, extra.processes[0].name);
    base.processes[0].name = None;
    base.processes[0].details = Some(Default::default());
    apply(&mut base, &extra, None).unwrap();
    assert!(base.processes[0].name.is_none());
    base.processes[0].details = None;
    base.processes[0].uid = Some(1000);
    apply(&mut base, &extra, None).unwrap();
    assert!(base.processes[0].name.is_none());
    assert_eq!(base.processes[0].uid, Some(1000));
}
