use super::*;
use newport_client::discovery::Reading;
use newport_domain::port_discovery::Kind;
use newport_protocol::{desktop, monitor};

fn publication(engine: &str) -> Publication {
    Publication {
        engine: Some(engine.into()),
        container: "container".into(),
        name: "web".into(),
        image: None,
        state: ContainerState::Running,
        host: Endpoint {
            address: "127.0.0.1".parse().unwrap(),
            port: 8080,
            interface: None,
        },
        target_port: 80,
        target_address: None,
        compose: None,
    }
}

fn scan(generation: u64, engine: &str, present: bool) -> Snapshot {
    let mut snapshot = snapshot(generation);
    snapshot.sockets.clear();
    snapshot.container_engine = Some(engine.into());
    snapshot.coverage.ipv6 = Report::complete();
    snapshot.coverage.containers = Report::complete();
    if present {
        snapshot.publications.push(publication(engine));
    }
    snapshot
}

fn monitor_roundtrip(snapshot: Snapshot) -> Snapshot {
    let call = monitor::Call {
        id: snapshot.generation,
        request: monitor::Request::Ports(Request { refresh: true }),
    };
    let response = monitor::Response {
        id: call.id,
        reply: monitor::Reply::Ports(Box::new(snapshot)),
    };
    let bytes = monitor::wire::encode_reply(&response, &call).unwrap();
    let response = monitor::wire::read_reply(&mut bytes.as_slice(), &call).unwrap();
    let monitor::Reply::Ports(snapshot) = response.reply else {
        panic!("unexpected monitor reply");
    };
    *snapshot
}

#[tokio::test]
async fn unknown_engine_publications_remain_visible_and_go_stale_when_missing() {
    let step = Arc::new(std::sync::atomic::AtomicU64::new(1));
    let server = uuid::Uuid::new_v4();
    let query = Query::new(policy(), server, move |_| {
        let generation = step.fetch_add(1, Ordering::AcqRel);
        async move {
            let mut value = scan(generation, "verified", matches!(generation, 1 | 3));
            if generation < 5 {
                value.container_engine = None;
                for publication in &mut value.publications {
                    publication.engine = None;
                }
                value.coverage.containers = Report {
                    outcome: Outcome::Partial,
                    issues: vec![Issue {
                        kind: IssueKind::Unsupported,
                        count: 1,
                        message: "Stable engine identity unavailable".into(),
                    }],
                };
            }
            Ok(monitor_roundtrip(value))
        }
    });
    let mut reading = None;
    for generation in 1..=5 {
        query.refresh().await.unwrap();
        let next = Reading::prepare(
            Ok(desktop_roundtrip(query.snapshot(), server)),
            reading.as_ref(),
        );
        let view = next.view().unwrap();
        assert!(view.inventory().fresh);
        assert_eq!(view.members().len(), 1);
        let member = &view.members()[0];
        assert_eq!(member.stale, !matches!(generation, 1 | 3));
        let Kind::Publication(publication) = &member.kind else {
            panic!("publication fixture")
        };
        assert!(publication.engine.is_none());
        reading = Some(next);
    }
    query.close();
}

pub(crate) fn desktop_roundtrip(
    observation: PortsObservation,
    server: uuid::Uuid,
) -> PortsObservation {
    use desktop::connection;
    let request = desktop::Request {
        id: desktop::RequestId::new(1).unwrap(),
        operation: desktop::Operation::Connection(Box::new(
            connection::Operation::ConnectionsQuery(Box::new(connection::ConnectionsQueryArgs {
                server,
                refresh: true,
            })),
        )),
    };
    let response = desktop::ServerMessage::Response(Box::new(desktop::Response {
        id: request.id,
        outcome: desktop::Outcome::Completed(Box::new(desktop::Reply::Connection(Box::new(
            connection::Reply::ConnectionsQuery(Box::new(observation)),
        )))),
    }));
    let bytes = desktop::wire::encode_server_for(&response, &request).unwrap();
    let response = desktop::wire::read_server(&mut bytes.as_slice()).unwrap();
    desktop::wire::validate_server_for(&response, &request).unwrap();
    let desktop::ServerMessage::Response(response) = response else {
        panic!("unexpected desktop envelope");
    };
    let desktop::Outcome::Completed(reply) = response.outcome else {
        panic!("unexpected desktop outcome");
    };
    let desktop::Reply::Connection(reply) = *reply else {
        panic!("unexpected desktop reply family");
    };
    let connection::Reply::ConnectionsQuery(observation) = *reply else {
        panic!("unexpected desktop reply operation");
    };
    *observation
}

#[tokio::test]
async fn failed_scans_keep_diagnostics_and_mark_the_reading_stale_across_both_protocols() {
    let step = Arc::new(std::sync::atomic::AtomicU64::new(1));
    let server = uuid::Uuid::new_v4();
    let query = Query::new(policy(), server, move |_| {
        let generation = step.fetch_add(1, Ordering::AcqRel);
        async move {
            let mut value = snapshot(generation);
            if generation >= 3 {
                value.scope.network.as_mut().unwrap().inode += 1;
            }
            if matches!(generation, 1 | 3) {
                value.sockets.clear();
                for (report, kind) in [
                    (&mut value.coverage.ipv4, IssueKind::TimedOut),
                    (&mut value.coverage.ipv6, IssueKind::PermissionDenied),
                    (&mut value.coverage.containers, IssueKind::Unavailable),
                ] {
                    *report = Report {
                        outcome: Outcome::Unavailable,
                        issues: vec![Issue {
                            kind,
                            count: 1,
                            message: "Unavailable source".into(),
                        }],
                    };
                }
            } else if generation == 5 {
                value.sockets.clear();
                value.coverage.ipv6 = Report::complete();
                value.container_engine = Some("engine".into());
                value.coverage.containers = Report::complete();
            }
            Ok(monitor_roundtrip(value))
        }
    });
    let mut reading = None;
    let mut copied_failure = None;
    for generation in 1..=5 {
        let failed = matches!(generation, 1 | 3);
        assert_eq!(query.refresh().await.is_err(), failed);
        if failed {
            copied_failure = Some(query.inner.snapshot());
        } else if let Some(old) = copied_failure.take() {
            assert!(
                query.project(old).data.is_none(),
                "recovery fences failed scans"
            );
        }
        let observation = desktop_roundtrip(query.snapshot(), server);
        assert_eq!(observation.error.is_some(), failed);
        assert_eq!(observation.stale, failed);
        if failed {
            assert!(observation.updated_at.is_none());
        }
        let next = Reading::prepare(Ok(observation), reading.as_ref());
        let view = next
            .view()
            .expect("failed scans still carry typed diagnostics");
        assert_eq!(view.evidence().current.generation, generation);
        assert!(view.evidence().retained.is_empty());
        assert_eq!(view.inventory().fresh, !failed);
        if failed {
            assert!(view.members().is_empty());
            assert!(!view.evidence().has_current_inventory());
            assert_eq!(
                view.evidence().current.coverage.ipv4.issues[0].kind,
                IssueKind::TimedOut
            );
            assert_eq!(
                view.evidence().current.coverage.ipv6.issues[0].kind,
                IssueKind::PermissionDenied
            );
            assert_eq!(
                view.evidence().current.coverage.containers.issues[0].kind,
                IssueKind::Unavailable
            );
        }
        reading = Some(next);
    }
    query.close();
    assert!(query.snapshot().data.is_none());
}

#[tokio::test]
async fn engine_replacement_and_retention_survive_both_protocol_boundaries() {
    let step = Arc::new(std::sync::atomic::AtomicU64::new(1));
    let query = Query::new(policy(), uuid::Uuid::nil(), move |refresh| {
        assert!(refresh);
        let step = step.fetch_add(1, Ordering::AcqRel);
        async move {
            let mut value = match step {
                1 => scan(step, "a", true),
                2 => {
                    let mut value = scan(step, "a", false);
                    value.coverage.containers = Report {
                        outcome: Outcome::Unavailable,
                        issues: vec![Issue {
                            kind: IssueKind::PermissionDenied,
                            count: 1,
                            message: "Engine access denied".into(),
                        }],
                    };
                    value
                }
                3 | 5 => scan(step, "b", false),
                4 => scan(step, "b", true),
                6 | 8 => scan(step, "a", false),
                7 => scan(step, "a", true),
                _ => return Err(crate::monitor::Error::Closed.into()),
            };
            if step >= 7 {
                value.scope.network.as_mut().unwrap().inode += 1;
            }
            Ok(monitor_roundtrip(value))
        }
    });
    let server = uuid::Uuid::new_v4();
    let mut reading = None;
    for (step, expected_members, expected_stale) in [
        (1, 1, 0),
        (2, 1, 1),
        (3, 1, 1),
        (4, 2, 1),
        (5, 1, 1),
        (6, 0, 0),
        (7, 1, 0),
        (8, 0, 0),
    ] {
        query.refresh().await.unwrap();
        let observation = desktop_roundtrip(query.snapshot(), server);
        let next = Reading::prepare(Ok(observation), reading.as_ref());
        let view = next.view().unwrap();
        assert!(view.inventory().fresh, "step {step}");
        assert_eq!(view.members().len(), expected_members, "step {step}");
        assert_eq!(
            view.members().iter().filter(|member| member.stale).count(),
            expected_stale,
            "step {step}"
        );
        assert!(
            view.members()
                .iter()
                .all(|member| matches!(member.kind, Kind::Publication(_)))
        );
        reading = Some(next);
    }
    assert!(query.refresh().await.is_err());
    let failed = Reading::prepare(
        Ok(desktop_roundtrip(query.snapshot(), server)),
        reading.as_ref(),
    );
    let view = failed.view().unwrap();
    assert!(!view.inventory().fresh);
    query.close();
}
