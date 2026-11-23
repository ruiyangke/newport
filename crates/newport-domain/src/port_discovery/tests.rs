use super::*;

fn holder(pid: u32) -> Holder {
    Holder {
        identity: Process { pid, start: 100 },
        name: Some("worker".into()),
        executable: Some("node".into()),
        path: None,
        label: Some("node".into()),
        account: None,
        uid: Some(1000),
        project: None,
        kinds: vec!["Node"],
        details: vec![],
    }
}

/// A holder of another program, run by the same account as `holder`.
fn program(pid: u32, name: &str) -> Holder {
    Holder {
        name: Some(name.into()),
        executable: Some(name.into()),
        label: Some(name.into()),
        ..holder(pid)
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
            1 => socket(&mut other).holders = vec![Arc::new(program(3, "other"))],
            2 => socket(&mut other).holder_coverage = HolderCoverage::Truncated,
            3 => other.scope = None,
            4 => other.reference.generation += 1,
            5 => other.endpoint.address = "fd00::4".parse().unwrap(),
            6 => other.endpoint.interface = Some(2),
            7 => other.endpoint.port += 1,
            8 => Arc::make_mut(other.scope.as_mut().unwrap()).boot = "reboot".into(),
            9 => Arc::make_mut(&mut socket(&mut other).holders[0]).uid = Some(1001),
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
    socket(&mut b).holders[1] = Arc::new(program(2, "other"));
    assert_eq!(rows(vec![a, b]).len(), 2);
}

#[test]
fn unknown_incomplete_and_different_program_owners_do_not_merge() {
    let a = member(1, "0.0.0.0", vec![]);
    let b = member(2, "::", vec![]);
    assert_eq!(rows(vec![a, b]).len(), 2);
    let a = member(1, "0.0.0.0", vec![holder(7)]);
    for case in 0..6 {
        let mut b = member(2, "::", vec![holder(7)]);
        match case {
            0 => socket(&mut b).holder_coverage = HolderCoverage::Partial,
            1 => socket(&mut b).holders = vec![Arc::new(program(7, "other"))],
            2 => b.scope = None,
            3 => Arc::make_mut(b.scope.as_mut().unwrap()).boot = "reboot".into(),
            4 => b.stale = true,
            _ => Arc::make_mut(&mut socket(&mut b).holders[0]).uid = Some(1001),
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
            _ => socket(&mut other).holders = vec![Arc::new(program(7, "other"))],
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

fn docker(pid: u32) -> Holder {
    Holder {
        account: Some("root".into()),
        uid: Some(0),
        kinds: vec!["Docker"],
        ..program(pid, "docker-proxy")
    }
}

/// A docker-proxy socket of a published port, in the system's docker.service.
fn proxy(id: u32, address: &str, port: u16, holders: Vec<Holder>) -> Member {
    let mut value = member(id, address, holders);
    value.endpoint.port = port;
    let socket = socket(&mut value);
    socket.uid = 0;
    socket.account = Some("root".into());
    socket.holder_coverage = HolderCoverage::Partial;
    socket.units = vec![Unit {
        manager_uid: None,
        name: "docker.service".into(),
        socket_cgroup: true,
    }];
    value
}

fn published(id: usize, address: &str, port: u16, container: &str) -> Member {
    let mut value = member(0, address, vec![]);
    value.reference.record = Record::Publication(id);
    value.endpoint.port = port;
    value.kind = Kind::Publication(Publication {
        engine: Some("engine".into()),
        engine_kind: EngineKind::Docker,
        container: format!("id-{container}"),
        name: container.into(),
        image: Some("debian:13".into()),
        service: None,
        compose: None,
        directory: None,
        project: None,
        target_port: 22,
        target_address: None,
        running: Some(true),
    });
    value
}

#[test]
fn sibling_processes_of_one_program_share_a_row_with_every_address() {
    let ipv4 = proxy(1, "0.0.0.0", 2201, vec![docker(10)]);
    let ipv6 = proxy(2, "::", 2201, vec![docker(11)]);
    let groups = group(vec![ipv6.clone(), ipv4.clone()]);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].kind, GroupKind::System);
    assert_eq!(groups[0].rows.len(), 1);
    let row = &groups[0].rows[0];
    assert_eq!(row.members, [ipv4, ipv6]);
    assert_eq!(row.label(), "docker.service");
    assert_eq!(row.destination(None).unwrap().host, "localhost");
}

#[test]
fn holders_without_a_program_name_are_told_apart_by_process() {
    let unnamed = |pid| Holder {
        name: None,
        executable: None,
        label: None,
        ..holder(pid)
    };
    let a = member(1, "0.0.0.0", vec![unnamed(7)]);
    let b = member(2, "::", vec![unnamed(8)]);
    assert_eq!(rows(vec![a.clone(), b]).len(), 2);
    let b = member(2, "::", vec![unnamed(7)]);
    assert_eq!(rows(vec![a, b]).len(), 1);
}

#[test]
fn a_published_port_and_its_proxy_sockets_are_one_container_row() {
    for unattributed in [false, true] {
        let holders = |pid| {
            if unattributed {
                vec![]
            } else {
                vec![docker(pid)]
            }
        };
        let members = vec![
            proxy(1, "0.0.0.0", 2201, holders(10)),
            proxy(2, "::", 2201, holders(11)),
            published(0, "0.0.0.0", 2201, "fx-minimal"),
            published(1, "::", 2201, "fx-minimal"),
        ];
        let groups = group(members.clone());
        assert_eq!(groups.len(), 1, "unattributed {unattributed}");
        assert_eq!(groups[0].kind, GroupKind::Containers);
        assert_eq!(groups[0].rows.len(), 1);
        let row = &groups[0].rows[0];
        assert_eq!(row.members.len(), members.len());
        assert_eq!(row.label(), "fx-minimal");
        assert_eq!(row.identifying_members().count(), 2);
        assert_eq!(row.destination(None).unwrap().host, "localhost");
    }
}

#[test]
fn a_published_port_in_a_compose_project_keeps_its_project_group() {
    let mut publication = published(0, "0.0.0.0", 5432, "stack-db-1");
    let Kind::Publication(value) = &mut publication.kind else {
        unreachable!()
    };
    value.compose = Some("stack".into());
    let groups = group(vec![
        publication,
        proxy(1, "0.0.0.0", 5432, vec![docker(10)]),
    ]);
    assert_eq!(groups.len(), 1);
    assert_eq!(
        groups[0].kind,
        GroupKind::Compose {
            name: "stack".into(),
            directory: None
        }
    );
    assert_eq!(groups[0].rows[0].members.len(), 2);
}

#[test]
fn another_program_or_address_on_a_published_port_keeps_its_own_row() {
    let publication = published(0, "127.0.0.1", 2201, "fx-minimal");
    let mut sshd = member(1, "127.0.0.1", vec![program(5, "sshd")]);
    sshd.endpoint.port = 2201;
    let mut elsewhere = member(2, "192.0.2.2", vec![]);
    elsewhere.endpoint.port = 2201;
    let other_port = proxy(3, "127.0.0.1", 2202, vec![docker(10)]);
    let mut stale = proxy(4, "127.0.0.1", 2201, vec![docker(11)]);
    stale.stale = true;
    let grouped = rows(vec![
        publication.clone(),
        sshd,
        elsewhere,
        other_port,
        stale,
    ]);
    assert_eq!(grouped.len(), 5);
    assert!(
        grouped
            .iter()
            .any(|row| row.members == [publication.clone()])
    );
}

#[test]
fn each_proxy_joins_the_container_published_on_its_address() {
    let first = published(0, "127.0.0.1", 2201, "first");
    let second = published(1, "10.0.0.4", 2201, "second");
    let grouped = rows(vec![
        first,
        second,
        proxy(1, "127.0.0.1", 2201, vec![docker(10)]),
        proxy(2, "10.0.0.4", 2201, vec![docker(11)]),
    ]);
    assert_eq!(grouped.len(), 2);
    for row in grouped {
        let label = row.label();
        assert_eq!(row.members.len(), 2, "{label}");
        let addresses: BTreeSet<_> = row.members.iter().map(|m| m.endpoint.address).collect();
        assert_eq!(addresses.len(), 1, "{label}");
    }
}

fn running(member: &mut Member, value: Option<bool>) {
    let Kind::Publication(publication) = &mut member.kind else {
        panic!("publication fixture");
    };
    publication.running = value;
}

#[test]
fn a_stopped_container_never_takes_a_listener_on_its_port() {
    let hidden = |id| {
        let mut value = member(id, "0.0.0.0", vec![]);
        value.endpoint.port = 2201;
        socket(&mut value).holder_coverage = HolderCoverage::Partial;
        value
    };
    for (state, joins) in [(Some(true), true), (None, true), (Some(false), false)] {
        let mut publication = published(0, "0.0.0.0", 2201, "fx-minimal");
        running(&mut publication, state);
        let grouped = rows(vec![
            publication,
            hidden(1),
            proxy(2, "::", 2201, vec![docker(10)]),
        ]);
        assert_eq!(grouped.len(), if joins { 1 } else { 3 }, "{state:?}");
    }
}

#[test]
fn a_socket_a_complete_scan_found_no_holder_for_is_not_a_proxy() {
    let mut empty = member(1, "0.0.0.0", vec![]);
    empty.endpoint.port = 2201;
    let publication = published(0, "0.0.0.0", 2201, "fx-minimal");
    assert_eq!(rows(vec![publication.clone(), empty.clone()]).len(), 2);
    socket(&mut empty).holder_coverage = HolderCoverage::Partial;
    assert_eq!(rows(vec![publication, empty]).len(), 1);
}

#[test]
fn only_a_known_proxy_joins_a_publication_in_the_other_family() {
    let mut hidden = member(1, "::", vec![]);
    hidden.endpoint.port = 80;
    socket(&mut hidden).ipv6_only = Some(true);
    socket(&mut hidden).holder_coverage = HolderCoverage::Partial;
    let publication = published(0, "0.0.0.0", 80, "web");
    assert_eq!(rows(vec![publication.clone(), hidden]).len(), 2);
    let named = proxy(2, "::", 80, vec![docker(10)]);
    assert_eq!(rows(vec![publication.clone(), named.clone()]).len(), 1);
    // Two wildcard publications reach the proxy equally: it keeps its own row.
    let mut other = published(1, "0.0.0.0", 80, "other");
    let Kind::Publication(value) = &mut other.kind else {
        unreachable!()
    };
    value.engine = Some("another-engine".into());
    let grouped = rows(vec![publication, other, named.clone()]);
    assert_eq!(grouped.len(), 3);
    assert!(grouped.iter().any(|row| row.members == [named.clone()]));
}

#[test]
fn every_engine_proxy_joins_its_container() {
    for name in [
        "docker-proxy",
        "rootlesskit",
        "rootlessport",
        "pasta",
        "pasta.avx2",
        "slirp4netns",
    ] {
        let grouped = rows(vec![
            published(0, "0.0.0.0", 8080, "web"),
            proxy(1, "::", 8080, vec![program(10, name)]),
        ]);
        assert_eq!(grouped.len(), 1, "{name}");
    }
    let grouped = rows(vec![
        published(0, "0.0.0.0", 8080, "web"),
        proxy(1, "::", 8080, vec![program(10, "nginx")]),
    ]);
    assert_eq!(grouped.len(), 2);
}

#[test]
fn one_program_name_with_another_executable_or_account_is_another_program() {
    let with = |pid, path: Option<&str>, uid: Option<u32>| Holder {
        path: path.map(str::to_owned),
        uid,
        ..program(pid, "python")
    };
    let pair = |a: Holder, b: Holder| {
        rows(vec![
            member(1, "0.0.0.0", vec![a]),
            member(2, "::", vec![b]),
        ])
        .len()
    };
    let system = Some("/usr/bin/python3");
    assert_eq!(
        pair(with(7, system, Some(1000)), with(8, system, Some(1000))),
        1
    );
    assert_eq!(
        pair(
            with(7, system, Some(1000)),
            with(8, Some("/opt/venv/bin/python"), Some(1000))
        ),
        2
    );
    assert_eq!(
        pair(with(7, system, Some(1000)), with(8, system, Some(1001))),
        2
    );
    // Unknown accounts are not the same account.
    assert_eq!(pair(with(7, system, None), with(8, system, None)), 2);
    assert_eq!(pair(with(7, system, None), with(7, system, None)), 1);
}
