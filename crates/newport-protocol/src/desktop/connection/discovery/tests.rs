use super::*;
use discovery::{ContainerState, Coverage, Endpoint, Namespace, Publication, Scope};

fn value() -> Discovery {
    let scope = Scope {
        network: Some(Namespace {
            device: 1,
            inode: 2,
        }),
        processes: Some(Namespace {
            device: 1,
            inode: 3,
        }),
        boot_id: Some("18f68fcb-71e9-47fe-a226-e9da27d82953".into()),
    };
    Discovery {
        current: Snapshot {
            generation: 2,
            scope: scope.clone(),
            ..Snapshot::default()
        },
        retained: vec![Snapshot {
            generation: 1,
            scope,
            container_engine: Some("engine".into()),
            coverage: Coverage {
                containers: Report::complete(),
                ..Coverage::default()
            },
            publications: vec![Publication {
                engine: Some("engine".into()),
                container: "container".into(),
                name: "web".into(),
                image: None,
                state: ContainerState::Running,
                host: Endpoint {
                    address: std::net::Ipv4Addr::UNSPECIFIED.into(),
                    port: 8080,
                    interface: None,
                },
                target_port: 80,
                target_address: None,
                compose: None,
            }],
            ..Snapshot::default()
        }],
        retention: Report::complete(),
    }
}

#[test]
fn stale_publications_do_not_make_failed_current_inventory_useful() {
    let mut value = value();
    value.validate().unwrap();
    assert!(!value.has_current_inventory());
    value.current.coverage.ipv6 = Report::complete();
    assert!(value.has_current_inventory());
}

#[test]
fn retained_slices_require_known_matching_scope_and_older_unique_generation() {
    for kind in 0..6 {
        let mut value = value();
        match kind {
            0 => value.retained[0].scope.network = None,
            1 => value.retained[0].scope.processes.as_mut().unwrap().inode += 1,
            2 => value.retained[0].generation = 2,
            3 => value.retained[0].generation = 3,
            4 => value.retained.push(value.retained[0].clone()),
            _ => value.retained[0].publications.clear(),
        }
        assert!(value.validate().is_err());
    }
}

#[test]
fn retention_diagnostics_are_explicit_and_bounded() {
    let mut value = value();
    value.retention.outcome = Outcome::Partial;
    assert!(value.validate().is_err());
    value.retention.issues.push(discovery::Issue {
        kind: discovery::IssueKind::BudgetExceeded,
        count: 1,
        message: "History limit reached.".into(),
    });
    value.validate().unwrap();
    value.retention.issues[0].count = 0;
    assert!(value.validate().is_err());
    value.retention.issues[0].count = 1;
    value.retention.issues[0].message = "x".repeat(discovery::MAX_DIAGNOSTIC + 1);
    assert!(value.validate().is_err());
}
