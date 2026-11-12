use super::*;

fn holder(pid: u32) -> Holder {
    Holder {
        identity: Process { pid, start: 100 },
        name: Some("worker".into()),
        executable: Some("node".into()),
        label: Some("node".into()),
        account: None,
        uid: Some(1000),
        project: None,
        kinds: vec!["Node"],
        details: vec![],
    }
}

fn member(id: u32, address: &str, holders: Vec<Holder>) -> Member {
    Member {
        observation: Observation::default(),
        reference: Reference {
            generation: 1,
            record: Record::Socket(id),
        },
        scope: Some(Arc::new(Scope {
            network: (1, 2),
            processes: (1, 3),
            boot: "boot".into(),
        })),
        endpoint: Endpoint {
            address: address.parse().unwrap(),
            port: 3000,
            interface: None,
        },
        stale: false,
        kind: Kind::Socket(Socket {
            inode: u64::from(id),
            cookie: Some(u64::from(id)),
            uid: 1000,
            account: None,
            ipv6_only: None,
            holders: holders.into_iter().map(Arc::new).collect(),
            holder_coverage: HolderCoverage::Complete,
            units: vec![],
        }),
    }
}

fn socket(value: &mut Member) -> &mut Socket {
    let Kind::Socket(socket) = &mut value.kind else {
        panic!("fixture socket");
    };
    socket
}

fn project(root: &str) -> Project {
    Project {
        root: root.into(),
        marker: ".git".into(),
        checkout: None,
    }
}

fn rows(members: Vec<Member>) -> Vec<Row> {
    group(members).into_iter().flat_map(|g| g.rows).collect()
}

#[test]
fn dual_stack_loopback_defaults_keep_explicit_overrides() {
    for (v4, v6) in [("0.0.0.0", "::"), ("127.0.0.1", "::1")] {
        let a = member(1, v4, vec![holder(2)]);
        let b = member(2, v6, vec![holder(2)]);
        for members in [vec![a.clone(), b.clone()], vec![b.clone(), a.clone()]] {
            let row = Row { members };
            assert_eq!(row.destination(None).unwrap().host, "localhost");
            assert_eq!(row.destination(Some(b.reference)).unwrap().host, "::1");
            assert_eq!(row.members.len(), 2);
        }
        // Alone, `::` accepts both families; `::1` is IPv6 only.
        assert_eq!(
            Row { members: vec![b] }.destination(None).unwrap().host,
            if v6 == "::" { "localhost" } else { "::1" }
        );
    }
    let mut a = member(1, "0.0.0.0", vec![]);
    let mut b = member(2, "::", vec![]);
    for member in [&mut a, &mut b] {
        socket(member).holder_coverage = HolderCoverage::Partial;
        socket(member).units.push(Unit {
            manager_uid: None,
            name: "ssh.socket".into(),
            socket_cgroup: true,
        });
    }
    assert_eq!(
        Row {
            members: vec![a, b]
        }
        .destination(None)
        .unwrap()
        .host,
        "localhost"
    );
}

#[test]
fn automatic_loopback_choice_rejects_ambiguous_or_incomplete_evidence() {
    let a = member(1, "0.0.0.0", vec![holder(2)]);
    let b = member(2, "::", vec![holder(2)]);
    for case in 0..12 {
        let mut other = b.clone();
        match case {
            0 => socket(&mut other).holders.clear(),
            1 => socket(&mut other).holders = vec![Arc::new(holder(3))],
            2 => socket(&mut other).holder_coverage = HolderCoverage::Truncated,
            3 => other.scope = None,
            4 => other.reference.generation += 1,
            5 => other.endpoint.address = "fd00::4".parse().unwrap(),
            6 => other.endpoint.interface = Some(2),
            7 => other.endpoint.port += 1,
            8 => Arc::make_mut(other.scope.as_mut().unwrap()).boot = "reboot".into(),
            9 => {
                Arc::make_mut(&mut socket(&mut other).holders[0])
                    .identity
                    .start += 1
            }
            10 => socket(&mut other).holder_coverage = HolderCoverage::Partial,
            _ => {
                Arc::make_mut(&mut socket(&mut other).holders[0]).project =
                    Some(project("/different"))
            }
        }
        assert_eq!(
            Row {
                members: vec![a.clone(), other]
            }
            .destination(None),
            Err(DestinationError::ChooseMember),
            "case {case}"
        );
    }
    let mut stale = b;
    stale.stale = true;
    assert_eq!(
        Row {
            members: vec![a, stale]
        }
        .destination(None),
        Err(DestinationError::Stale)
    );
}

/// A server may reach only one loopback family: with IPv6 off, or an
/// IPv4-only SSH server, `::1` fails. A socket that accepts both families
/// defaults to `localhost`, whose forward tries `127.0.0.1` and then `::1`.
#[test]
fn a_lone_dual_stack_socket_defaults_to_both_loopbacks() {
    for (ipv6_only, expected) in [
        (Some(false), "localhost"),
        (None, "localhost"),
        (Some(true), "::1"),
    ] {
        let mut lone = member(1, "::", vec![holder(2)]);
        socket(&mut lone).ipv6_only = ipv6_only;
        let row = Row {
            members: vec![lone.clone()],
        };
        let destination = row.destination(None).unwrap();
        assert_eq!(destination.host, expected, "{ipv6_only:?}");
        assert_eq!(destination.port, lone.endpoint.port);
        assert_eq!(
            row.destination(Some(lone.reference)).unwrap().host,
            "::1",
            "an explicit choice keeps its family"
        );
    }
}

#[test]
fn compatible_dual_stack_preserves_members_and_defaults_to_both_loopbacks() {
    let a = member(1, "0.0.0.0", vec![holder(2)]);
    let b = member(2, "::", vec![holder(2)]);
    let row = rows(vec![b.clone(), a.clone()]).remove(0);
    assert_eq!(row.members, [a.clone(), b.clone()]);
    assert_eq!(row.destination(None).unwrap().host, "localhost");
    assert_eq!(
        row.destination(Some(a.reference)).unwrap().host,
        "127.0.0.1"
    );
    assert_eq!(row.destination(Some(b.reference)).unwrap().host, "::1");
    assert_eq!(rows(vec![a, b]), [row]);
}

#[test]
fn reuseport_sockets_with_the_same_holders_keep_both_identities() {
    let a = member(1, "10.0.0.4", vec![holder(2), holder(3)]);
    let mut b = member(2, "10.0.0.4", vec![holder(3), holder(2)]);
    let result = rows(vec![a.clone(), b.clone()]);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].members.len(), 2);
    assert_eq!(result[0].label(), "Shared listener");
    assert_eq!(result[0].destination(None).unwrap().host, "10.0.0.4");
    socket(&mut b).holders.pop();
    assert_eq!(rows(vec![a, b]).len(), 2);
}

#[test]
fn unknown_incomplete_and_reused_process_owners_do_not_merge() {
    let a = member(1, "0.0.0.0", vec![]);
    let b = member(2, "::", vec![]);
    assert_eq!(rows(vec![a, b]).len(), 2);
    let a = member(1, "0.0.0.0", vec![holder(7)]);
    for case in 0..5 {
        let mut b = member(2, "::", vec![holder(7)]);
        match case {
            0 => socket(&mut b).holder_coverage = HolderCoverage::Partial,
            1 => Arc::make_mut(&mut socket(&mut b).holders[0]).identity.start += 1,
            2 => b.scope = None,
            3 => Arc::make_mut(b.scope.as_mut().unwrap()).boot = "reboot".into(),
            _ => b.stale = true,
        }
        assert_eq!(rows(vec![a.clone(), b]).len(), 2, "case {case}");
    }
}

#[test]
fn verified_socket_unit_has_precedence_without_manufacturing_a_holder() {
    let mut a = member(1, "0.0.0.0", vec![]);
    socket(&mut a).units = vec![Unit {
        manager_uid: None,
        name: "ssh.socket".into(),
        socket_cgroup: true,
    }];
    let groups = group(vec![a.clone()]);
    assert_eq!(groups[0].kind, GroupKind::System);
    assert_eq!(groups[0].rows[0].label(), "ssh.socket");
    assert!(socket(&mut a).holders.is_empty());
    let mut b = a.clone();
    b.reference.record = Record::Socket(2);
    b.endpoint.address = "::".parse().unwrap();
    assert_eq!(rows(vec![a.clone(), b]).len(), 1);
    socket(&mut a).units[0].name = "user@1000.service".into();
    assert_eq!(a.label(), "TCP listener");
    assert_eq!(group(vec![a.clone()])[0].kind, GroupKind::Other);
    socket(&mut a).units[0].name = "session-3.scope".into();
    assert_eq!(a.label(), "TCP listener");
    socket(&mut a).uid = 0;
    assert_eq!(group(vec![a])[0].kind, GroupKind::Other);
}

#[test]
fn conflicting_units_do_not_invent_one_service_or_system_group() {
    let mut a = member(1, "::", vec![holder(2)]);
    socket(&mut a).units = vec![
        Unit {
            manager_uid: None,
            name: "alpha.service".into(),
            socket_cgroup: true,
        },
        Unit {
            manager_uid: None,
            name: "beta.service".into(),
            socket_cgroup: true,
        },
    ];
    assert_eq!(a.unit(), None);
    assert_eq!(group(vec![a])[0].kind, GroupKind::Other);
}

#[test]
fn a_verified_service_with_matching_partial_holders_keeps_one_row() {
    let mut a = member(1, "0.0.0.0", vec![holder(7)]);
    let mut b = member(2, "::", vec![holder(7)]);
    for value in [&mut a, &mut b] {
        socket(value).holder_coverage = HolderCoverage::Partial;
        socket(value).units = vec![Unit {
            manager_uid: None,
            name: "ssh.socket".into(),
            socket_cgroup: true,
        }];
    }
    let grouped = rows(vec![a.clone(), b.clone()]);
    assert_eq!(grouped.len(), 1);
    assert_eq!(grouped[0].label(), "ssh.socket");
    assert_eq!(grouped[0].members, [a.clone(), b.clone()]);
    assert_eq!(grouped[0].destination(None).unwrap().host, "localhost");
    for case in 0..4 {
        let mut other = b.clone();
        match case {
            0 => socket(&mut other).holder_coverage = HolderCoverage::Truncated,
            1 => socket(&mut other).units.clear(),
            2 => socket(&mut other).units[0].name = "other.socket".into(),
            _ => {
                Arc::make_mut(&mut socket(&mut other).holders[0])
                    .identity
                    .pid += 1
            }
        }
        assert_eq!(rows(vec![a.clone(), other]).len(), 2, "case {case}");
    }
}

#[test]
fn project_and_checkout_require_agreement_without_omitted_holders() {
    let mut a = member(1, "0.0.0.0", vec![holder(2), holder(3)]);
    Arc::make_mut(&mut socket(&mut a).holders[0]).project = Some(project("/alpha"));
    assert_eq!(a.project(), None);
    Arc::make_mut(&mut socket(&mut a).holders[1]).project = Some(project("/alpha"));
    assert_eq!(a.project(), Some(&project("/alpha")));
    Arc::make_mut(&mut socket(&mut a).holders[1])
        .project
        .as_mut()
        .unwrap()
        .checkout = Some(Checkout {
        path: "/feature".into(),
        branch: Some("feature".into()),
        main: Some(false),
    });
    assert_eq!(a.project(), None);
    Arc::make_mut(&mut socket(&mut a).holders[1]).project = Some(project("/beta"));
    assert_eq!(a.project(), None);
    socket(&mut a).holders.pop();
    socket(&mut a).holder_coverage = HolderCoverage::Partial;
    assert_eq!(a.project(), Some(&project("/alpha")));
    socket(&mut a).holder_coverage = HolderCoverage::Truncated;
    assert_eq!(a.project(), None);
}

#[test]
fn omitted_holders_do_not_become_ownerless_service_rows() {
    let mut a = member(1, "0.0.0.0", vec![]);
    let mut b = member(2, "::", vec![]);
    for value in [&mut a, &mut b] {
        socket(value).holder_coverage = HolderCoverage::Partial;
        socket(value).units = vec![Unit {
            manager_uid: None,
            name: "ssh.socket".into(),
            socket_cgroup: true,
        }];
    }
    assert_eq!(rows(vec![a.clone(), b.clone()]).len(), 1);
    socket(&mut b).holder_coverage = HolderCoverage::Truncated;
    assert_eq!(rows(vec![a.clone(), b.clone()]).len(), 2);
    socket(&mut a).holder_coverage = HolderCoverage::Truncated;
    assert_eq!(rows(vec![a, b]).len(), 2);
}

#[test]
fn selection_tracks_cookie_endpoint_and_scope_not_row_position() {
    let a = member(1, "127.0.0.1", vec![holder(7)]);
    let selection = Selection::capture([&a]);
    let mut next = a.clone();
    next.reference.generation += 1;
    next.reference.record = Record::Socket(35);
    Arc::make_mut(&mut socket(&mut next).holders[0]).name = Some("renamed".into());
    assert!(selection.contains(&next));
    for case in 0..6 {
        let mut next = next.clone();
        match case {
            0 => socket(&mut next).cookie = Some(999),
            1 => socket(&mut next).cookie = None,
            2 => next.endpoint.address = "::1".parse().unwrap(),
            3 => next.stale = true,
            4 => next.scope = None,
            _ => Arc::make_mut(next.scope.as_mut().unwrap()).network.1 += 1,
        }
        assert!(!selection.contains(&next), "case {case}");
    }
}

#[test]
fn weak_choices_work_in_the_original_observation_and_expire_on_refresh() {
    for unknown_scope in [false, true] {
        let mut original = member(1, "127.0.0.1", vec![]);
        socket(&mut original).cookie = None;
        if unknown_scope {
            original.scope = None;
        }
        let selection = Selection::capture([&original]);
        assert!(selection.contains(&original.clone()));
        for case in 0..4 {
            let mut changed = original.clone();
            match case {
                0 => changed.observation = Observation::default(),
                1 => changed.reference.generation += 1,
                2 => changed.endpoint.address = "::1".parse().unwrap(),
                _ => changed.stale = true,
            }
            assert!(!selection.contains(&changed), "case {case}");
        }
        let row = Row {
            members: vec![original],
        };
        assert_eq!(row.destination(None).unwrap().host, "127.0.0.1");
    }
}

#[test]
fn destinations_preserve_specific_binds_and_reject_remote_interface_loss() {
    for (address, expected) in [
        ("0.0.0.0", "127.0.0.1"),
        ("::", "::1"),
        ("127.0.0.2", "127.0.0.2"),
        ("10.0.0.4", "10.0.0.4"),
        ("fd00::4", "fd00::4"),
    ] {
        let mut a = member(1, address, vec![]);
        assert_eq!(a.endpoint.destination().unwrap().host, expected);
        a.endpoint.interface = Some(2);
        assert_eq!(
            a.endpoint.destination(),
            Err(DestinationError::RemoteInterface)
        );
    }
    let a = member(1, "fe80::4", vec![]);
    assert_eq!(
        a.endpoint.destination(),
        Err(DestinationError::RemoteInterface)
    );
}

#[test]
fn labels_escape_controls_and_search_finds_all_holder_context() {
    let mut a = member(1, "127.0.0.1", vec![holder(2)]);
    Arc::make_mut(&mut socket(&mut a).holders[0]).label = Some("name\n\u{202e}spoof".into());
    assert_eq!(a.label(), "name\\u{a}\\u{202e}spoof");
    socket(&mut a).holders.push(Arc::new(holder(3)));
    Arc::make_mut(&mut socket(&mut a).holders[1]).project =
        Some(project("/only-the-second-process"));
    assert!(a.search_text().contains("only-the-second-process"));
    assert_eq!(a.label(), "Shared listener");
}
