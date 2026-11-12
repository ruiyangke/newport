use super::*;
use crate::discovery::Reading;

#[test]
fn limited_process_inspection_preserves_known_projects_without_merging_sockets() {
    let mut current = snapshot(1);
    current.publications.clear();
    current.sockets[0].services.clear();
    current.coverage.processes = p::Report {
        outcome: p::Outcome::Partial,
        issues: vec![p::Issue {
            kind: p::IssueKind::PermissionDenied,
            count: 1,
            message: "Another account cannot be inspected".into(),
        }],
    };
    for process in &mut current.processes {
        process.details.as_mut().unwrap().project = Some(Project {
            root: "/repo".into(),
            marker: ".git".into(),
            worktree: Some("/repo/feature".into()),
            branch: Some("feature".into()),
            main: Some(false),
        });
    }
    let mut sibling = current.sockets[0].clone();
    sibling.id += 1;
    sibling.inode += 1;
    sibling.cookie = Some(78);
    sibling.endpoint.address = "127.0.0.1".parse().unwrap();
    sibling.endpoint.interface = None;
    sibling.ipv6_only = None;
    current.sockets.push(sibling);
    let view = View::new(observation(current.clone()).data.unwrap(), false).unwrap();
    let groups = view.groups();
    assert_eq!(groups.len(), 1);
    assert!(matches!(&groups[0].kind, d::GroupKind::Project(project)
        if project.root == "/repo"
        && project.checkout.as_ref().unwrap().branch.as_deref() == Some("feature")));
    assert_eq!(groups[0].rows.len(), 2);
    assert!(view.members().iter().all(|member| matches!(&member.kind,
        d::Kind::Socket(socket) if socket.holder_coverage == d::HolderCoverage::Partial)));

    current.sockets[0].holders_truncated = true;
    let view = View::new(observation(current).data.unwrap(), false).unwrap();
    assert!(view.members()[0].project().is_none());
    assert!(view.members()[1].project().is_some());
}

#[test]
fn inventory_notice_distinguishes_missing_ports_from_limited_process_information() {
    let mut snapshot = snapshot(1);
    let partial = p::Report {
        outcome: p::Outcome::Partial,
        issues: vec![p::Issue {
            kind: p::IssueKind::PermissionDenied,
            count: 1,
            message: "Access denied".into(),
        }],
    };
    snapshot.coverage.processes = partial.clone();
    let view = |current, stale| {
        View::new(
            Discovery {
                current,
                retained: vec![],
                retention: p::Report::complete(),
            },
            stale,
        )
        .unwrap()
    };
    assert!(!view(snapshot.clone(), false).inventory_incomplete());
    assert!(view(snapshot.clone(), true).inventory_incomplete());
    snapshot.coverage.ipv6 = partial.clone();
    assert!(view(snapshot.clone(), false).inventory_incomplete());
    snapshot.coverage.ipv6 = p::Report::complete();
    snapshot.coverage.containers = partial;
    assert!(view(snapshot, false).inventory_incomplete());
}

fn observation(current: p::Snapshot) -> newport_protocol::desktop::connection::PortsObservation {
    newport_protocol::desktop::connection::PortsObservation {
        data: Some(Discovery {
            current,
            retained: vec![],
            retention: p::Report::complete(),
        }),
        error: None,
        fetching: false,
        stale: false,
        revision: 1,
        updated_at: Some(1000),
    }
}

#[test]
fn cached_readings_reuse_views_but_repeated_revision_numbers_do_not_hide_changes() {
    let original = observation(snapshot(1));
    let first = Reading::prepare(Ok(original.clone()), None);
    let mut cached = original.clone();
    cached.fetching = true;
    let cached = Reading::prepare(Ok(cached), Some(&first));
    assert!(!cached.data_changed(Some(&first)));
    assert!(cached.status().fetching);
    let mut changed = original;
    changed.data.as_mut().unwrap().current.sockets[0]
        .endpoint
        .port += 1;
    let changed = Reading::prepare(Ok(changed), Some(&cached));
    assert!(changed.data_changed(Some(&cached)));
    assert_eq!(changed.view().unwrap().members()[0].endpoint.port, 8001);
}

#[test]
fn withdrawn_data_is_not_resurrected_while_a_query_refetches() {
    let first = Reading::prepare(Ok(observation(snapshot(1))), None);
    let mut next = observation(snapshot(1));
    next.data = None;
    next.fetching = true;
    let next = Reading::prepare(Ok(next), Some(&first));
    assert!(next.view().is_none());
    assert!(next.status().updated_at.is_none());
    assert!(next.data_changed(Some(&first)));
}

#[test]
fn transport_failure_keeps_only_stale_evidence_and_preserves_the_error_code() {
    let mut original = snapshot(1);
    original.sockets[0].cookie = None;
    let original = observation(original);
    let first = Reading::prepare(Ok(original.clone()), None);
    let selected = d::Selection::capture(first.view().unwrap().members());
    let error = crate::request_error::RequestError {
        message: "offline".into(),
        code: Some("transport_closed".into()),
    };
    let failed = Reading::prepare(Err(error.clone()), Some(&first));
    assert_eq!(failed.status().error.as_ref(), Some(&error));
    assert_eq!(failed.status().updated_at, first.status().updated_at);
    assert!(!failed.view().unwrap().inventory().fresh);
    assert!(
        failed
            .view()
            .unwrap()
            .members()
            .iter()
            .all(|member| member.stale)
    );
    assert_eq!(
        failed.view().unwrap().evidence(),
        first.view().unwrap().evidence()
    );
    let repeated = Reading::prepare(Err(error), Some(&failed));
    assert!(!repeated.data_changed(Some(&failed)));
    let recovered = Reading::prepare(Ok(original), Some(&repeated));
    assert!(recovered.view().unwrap().inventory().fresh);
    assert!(!selected.contains(&recovered.view().unwrap().members()[0]));
}

#[test]
fn invalid_reading_does_not_replace_valid_evidence_or_leave_it_fresh() {
    let first = Reading::prepare(Ok(observation(snapshot(1))), None);
    let mut invalid = observation(snapshot(2));
    invalid.data.as_mut().unwrap().current.sockets[0]
        .holders
        .push(999);
    let failed = Reading::prepare(Ok(invalid), Some(&first));
    assert!(failed.status().error.is_some());
    assert!(failed.status().stale);
    assert_eq!(
        failed.view().unwrap().evidence(),
        first.view().unwrap().evidence()
    );
    assert!(!failed.view().unwrap().inventory().fresh);
}

#[test]
fn weak_selection_belongs_to_one_view_even_when_remote_ids_repeat() {
    let mut current = snapshot(1);
    current.sockets[0].cookie = None;
    let evidence = Discovery {
        current,
        retained: vec![],
        retention: p::Report::complete(),
    };
    let view = View::new(evidence.clone(), false).unwrap();
    let selection = d::Selection::capture(view.members());
    assert!(selection.contains(&view.clone().members()[0]));
    let refreshed = View::new(evidence, false).unwrap();
    assert!(!selection.contains(&refreshed.members()[0]));
}

fn snapshot(generation: u64) -> p::Snapshot {
    p::Snapshot {
        generation,
        scope: p::Scope {
            network: Some(p::Namespace {
                device: 1,
                inode: 2,
            }),
            processes: Some(p::Namespace {
                device: 1,
                inode: 3,
            }),
            boot_id: Some("18f68fcb-71e9-47fe-a226-e9da27d82953".into()),
        },
        coverage: p::Coverage {
            ipv4: p::Report::complete(),
            ipv6: p::Report::complete(),
            processes: p::Report::complete(),
            services: p::Report::complete(),
            containers: p::Report::complete(),
            ..p::Coverage::default()
        },
        container_engine: Some("engine".into()),
        processes: [7, 8]
            .into_iter()
            .map(|id| p::Process {
                id,
                identity: p::ProcessIdentity {
                    pid: id * 10,
                    start_ticks: u64::from(id) * 100,
                },
                name: Some("MainThread".into()),
                executable_name: Some("python3".into()),
                uid: Some(1000),
                account: None,
                details: Some(p::Details {
                    arguments: vec![
                        "python3".into(),
                        "-m".into(),
                        "http.server".into(),
                        "--password=DO_NOT_SEARCH".into(),
                    ],
                    ..p::Details::default()
                }),
                metadata: p::Report::complete(),
            })
            .collect(),
        sockets: vec![p::Socket {
            id: 3,
            endpoint: p::Endpoint {
                address: "::".parse().unwrap(),
                port: 8000,
                interface: Some(7),
            },
            source: p::SocketSource::Diagnostic,
            inode: 101,
            cookie: Some(77),
            uid: 0,
            account: Some("root".into()),
            ipv6_only: Some(true),
            cgroup: None,
            holders: vec![8, 7],
            holder_state: p::HolderState::Observed,
            holders_truncated: false,
            services: vec![p::Association {
                service: 1,
                relation: p::ServiceRelation::Holder { process: 7 },
            }],
        }],
        services: vec![p::Service {
            id: 1,
            manager: p::Manager::System,
            unit: "web.service".into(),
            description: Some("Web".into()),
            control_group: "/system.slice/web.service".into(),
            main_pid: Some(99),
            triggers: vec![],
        }],
        publications: vec![p::Publication {
            engine: Some("engine".into()),
            container: "container".into(),
            name: "web".into(),
            image: Some("team/web".into()),
            state: p::ContainerState::Stopped,
            host: p::Endpoint {
                address: "0.0.0.0".parse().unwrap(),
                port: 8080,
                interface: None,
            },
            target_port: 80,
            target_address: None,
            compose: None,
        }],
        ..p::Snapshot::default()
    }
}

fn evidence() -> Discovery {
    Discovery {
        current: snapshot(2),
        retained: vec![snapshot(1)],
        retention: p::Report::complete(),
    }
}

#[test]
fn view_preserves_all_evidence_and_resolves_generation_local_ids() {
    let evidence = evidence();
    let view = View::new(evidence.clone(), false).unwrap();
    assert_eq!(view.evidence(), &evidence);
    assert_eq!(view.members().len(), 4);
    for member in view.members() {
        let snapshot = view.snapshot(member.reference).unwrap();
        assert_eq!(snapshot.generation, member.reference.generation);
        assert_eq!(member.stale, snapshot.generation == 1);
        match &member.kind {
            d::Kind::Socket(socket) => {
                assert_eq!(
                    socket
                        .holders
                        .iter()
                        .map(|p| p.identity.pid)
                        .collect::<Vec<_>>(),
                    [70, 80]
                );
                assert!(
                    socket
                        .holders
                        .iter()
                        .all(|holder| holder.identity.pid != 99)
                );
                assert_eq!(member.endpoint.interface, Some(7));
                assert_eq!(socket.uid, 0);
                assert_eq!(socket.holders[0].uid, Some(1000));
                assert_eq!(view.socket(member.reference), snapshot.sockets.first());
                assert_eq!(view.socket(member.reference).unwrap().ipv6_only, Some(true));
            }
            d::Kind::Publication(publication) => {
                assert_eq!(publication.running, Some(false));
                assert_eq!(
                    view.publication(member.reference),
                    snapshot.publications.first()
                );
            }
        }
    }
}

#[test]
fn search_uses_curated_arguments_and_never_service_main_pid_as_a_holder() {
    let view = View::new(evidence(), false).unwrap();
    let search = view.members()[0].search_text();
    assert!(search.contains("web.service"));
    assert!(search.contains("python3"));
    assert!(search.contains("http.server"));
    assert!(!search.contains("do_not_search"));
    assert!(!search.contains("password"));
    assert!(!search.split_whitespace().any(|v| v == "99"));
}

#[test]
fn incomplete_inspection_and_stale_replies_do_not_authorize_absence() {
    let mut evidence = evidence();
    evidence.current.sockets[0].holders_truncated = true;
    let view = View::new(evidence, true).unwrap();
    assert!(view.members().iter().all(|member| member.stale));
    assert!(!view.inventory().fresh);
    let d::Kind::Socket(socket) = &view.members()[0].kind else {
        panic!("fixture socket");
    };
    assert_eq!(socket.holder_coverage, d::HolderCoverage::Truncated);
}

#[test]
fn invalid_references_are_rejected_before_a_partial_ui_model_is_created() {
    let mut evidence = evidence();
    evidence.current.sockets[0].holders.push(999);
    assert!(View::new(evidence, false).is_err());
}
