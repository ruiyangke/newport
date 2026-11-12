use super::*;
use newport_protocol::monitor::ports::{MAX_REPLY_TEXT, discovery::*};
use std::net::{Ipv4Addr, Ipv6Addr};

fn snapshot(count: u32) -> Snapshot {
    Snapshot {
        generation: 1,
        sockets: (1..=count)
            .map(|id| Socket {
                id,
                endpoint: Endpoint {
                    address: Ipv4Addr::LOCALHOST.into(),
                    port: 8080,
                    interface: None,
                },
                source: SocketSource::Diagnostic,
                inode: u64::from(id),
                cookie: Some(u64::from(id)),
                uid: 1000,
                account: None,
                ipv6_only: None,
                cgroup: None,
                holders: vec![id, id % count + 1],
                holder_state: HolderState::Observed,
                holders_truncated: false,
                services: Vec::new(),
            })
            .collect(),
        processes: (1..=count)
            .map(|id| Process {
                id,
                identity: ProcessIdentity {
                    pid: id,
                    start_ticks: 1,
                },
                name: Some("listener".into()),
                executable_name: Some("server".into()),
                uid: Some(1000),
                account: None,
                details: None,
                metadata: Report::complete(),
            })
            .collect(),
        coverage: Coverage {
            ipv4: Report::complete(),
            ipv6: Report::complete(),
            processes: Report::complete(),
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn fitting_small_response_preserves_every_observation() {
    let mut value = snapshot(3);
    let original = value.clone();
    fit(&mut value).unwrap();
    assert_eq!(value, original);
}

#[test]
fn dropping_large_details_preserves_inventory_and_every_holder() {
    let mut value = snapshot(256);
    for process in &mut value.processes {
        process.details = Some(Details {
            arguments: vec!["a".repeat(4000); 4],
            ..Default::default()
        });
    }
    value.coverage.details = Report::complete();
    assert!(value.validate_records().unwrap() > MAX_REPLY_TEXT);
    assert!(value.validate().is_err());
    let sockets = value.sockets.clone();
    fit(&mut value).unwrap();
    assert_eq!(value.sockets, sockets);
    assert_eq!(value.processes.len(), 256);
    assert!(
        value
            .processes
            .iter()
            .all(|p| p.details.is_none() && p.metadata.outcome == Outcome::Partial)
    );
    assert_eq!(value.coverage.details.outcome, Outcome::Partial);
    assert_eq!(value.coverage.ipv4.outcome, Outcome::Complete);
    assert_eq!(value.coverage.processes.outcome, Outcome::Complete);
    assert!(wire::fits_response(&value).unwrap());
}

#[test]
fn field_fitting_keeps_complete_holder_sets_and_marks_only_omitted_family() {
    let mut value = snapshot(4096);
    value.sockets[0].endpoint.address = Ipv6Addr::LOCALHOST.into();
    value.sockets[0].ipv6_only = Some(true);
    value.validate().unwrap();
    assert!(!wire::fits_response(&value).unwrap());
    let sockets = value.sockets.clone();
    fit(&mut value).unwrap();
    assert!(!value.sockets.is_empty());
    assert!(value.sockets.len() < sockets.len());
    assert_eq!(value.sockets, sockets[..value.sockets.len()]);
    assert_eq!(value.coverage.ipv4.outcome, Outcome::Partial);
    assert_eq!(value.coverage.ipv6.outcome, Outcome::Complete);
    assert!(
        value
            .sockets
            .iter()
            .all(|s| s.holders.len() == 2 && !s.holders_truncated)
    );
    let request = helper::Request {
        projects: None,
        contract_id: Some(newport_protocol::monitor::CONTRACT_ID.into()),
        generation: 1,
    };
    let reply = helper::Reply {
        contract_id: request.contract_id.clone(),
        generation: 1,
        result: Ok(Box::new(value.clone())),
    };
    let bytes = helper::wire::encode_reply(&reply, &request).unwrap();
    let decoded = helper::wire::read_reply(&mut bytes.as_slice(), &request).unwrap();
    assert_eq!(*decoded.result.unwrap(), value);
    let call = newport_protocol::monitor::Call {
        id: u64::MAX,
        request: newport_protocol::monitor::Request::Ports(Request { refresh: true }),
    };
    let response = newport_protocol::monitor::Response {
        id: call.id,
        reply: newport_protocol::monitor::Reply::Ports(Box::new(value)),
    };
    let frame = newport_protocol::monitor::wire::encode_reply(&response, &call).unwrap();
    assert!(frame.len() < newport_protocol::monitor::SERVER_LIMITS.message);
}

#[test]
fn publications_fit_without_inventing_socket_failure_or_losing_engine_scope() {
    let mut value = snapshot(2);
    value.container_engine = Some("engine".into());
    value.coverage.containers = Report::complete();
    value.publications = (0..1000)
        .map(|id| Publication {
            engine: Some("engine".into()),
            container: format!("container-{id}"),
            name: "n".repeat(4000),
            image: Some("i".repeat(4000)),
            state: ContainerState::Running,
            host: Endpoint {
                address: Ipv4Addr::UNSPECIFIED.into(),
                port: 8080,
                interface: None,
            },
            target_port: 80,
            target_address: None,
            compose: None,
        })
        .collect();
    fit(&mut value).unwrap();
    assert!(!value.publications.is_empty());
    assert!(value.publications.len() < 1000);
    assert_eq!(value.container_engine.as_deref(), Some("engine"));
    assert_eq!(value.coverage.containers.outcome, Outcome::Partial);
    assert_eq!(value.sockets.len(), 2);
    assert_eq!(value.coverage.ipv4.outcome, Outcome::Complete);
    value.validate().unwrap();
    assert!(wire::fits_response(&value).unwrap());
}

#[test]
fn invalid_records_are_not_hidden_by_pruning_or_budget_eviction() {
    let mut value = snapshot(2);
    value.processes[0].name = Some("invalid\0name".into());
    let original = value.clone();
    assert!(fit(&mut value).is_err());
    assert_eq!(value, original);
}

#[test]
fn omission_diagnostic_remains_present_when_report_is_full() {
    let mut report = Report {
        outcome: Outcome::Partial,
        issues: vec![
            Issue {
                kind: IssueKind::Io,
                count: 1,
                message: "Unavailable metadata.".into()
            };
            MAX_ISSUES
        ],
    };
    omitted(&mut report, 10);
    omitted(&mut report, 20);
    assert_eq!(report.issues.len(), MAX_ISSUES);
    assert_eq!(
        report.issues.last().unwrap().kind,
        IssueKind::BudgetExceeded
    );
    assert_eq!(report.issues.last().unwrap().count, 30);
}
