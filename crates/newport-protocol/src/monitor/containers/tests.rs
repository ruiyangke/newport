use super::*;

#[test]
fn mutations_validate_targets_and_whole_request_budgets() {
    let request = Request::Action {
        container: "../web".into(),
        verb: ContainerVerb::Stop,
    };
    assert!(request.validate().is_err());
    assert!(
        Request::ProjectAction {
            project: "app".into(),
            verb: ProjectVerb::Stop,
            expected_ids: vec![]
        }
        .validate()
        .is_err()
    );
    assert!(
        Request::Remove {
            containers: vec!["a".repeat(128); MAX_IDS]
        }
        .validate()
        .is_err()
    );
    Request::ProjectAction {
        project: "app".into(),
        verb: ProjectVerb::Restart,
        expected_ids: vec!["a".repeat(64); MAX_IDS],
    }
    .validate()
    .unwrap();
}
#[test]
fn logs_select_one_target_and_bound_the_page() {
    assert!(
        serde_json::from_value::<Request>(serde_json::json!({
            "op": "container_logs", "target": {"container": "web", "project": "app"}
        }))
        .is_err()
    );
    let request = Request::Logs(LogsRequest {
        target: LogTarget::Project("app".into()),
        cursor: Some("p1.aaaaaaaaaaaa-5-1".into()),
        lines: Some(2000),
    });
    request.validate().unwrap();
    let value = serde_json::to_value(&request).unwrap();
    assert_eq!(serde_json::from_value::<Request>(value).unwrap(), request);
    let mut page = ContainerLogPage::default();
    page.validate().unwrap();
    page.entries = vec![ContainerLogEntry::default(); MAX_LOG_ENTRIES + 1];
    assert!(page.validate().is_err());
    page.entries = vec![
        ContainerLogEntry {
            message: "x".repeat(MAX_TEXT),
            ..Default::default()
        };
        MAX_REPLY_TEXT / MAX_TEXT + 1
    ];
    assert!(page.validate().is_err());
}
#[test]
fn replies_match_operations_and_reject_impossible_outcomes() {
    let request = Request::Remove {
        containers: vec!["web".into()],
    };
    assert!(
        Reply::Removed(Removed { removed: 2 })
            .validate_for(&request)
            .is_err()
    );
    assert!(
        Reply::Action(Done { ok: true })
            .validate_for(&request)
            .is_err()
    );
    let request = Request::Action {
        container: "web".into(),
        verb: ContainerVerb::Start,
    };
    assert!(
        Reply::Action(Done { ok: false })
            .validate_for(&request)
            .is_err()
    );
}
#[test]
fn observations_keep_multicore_cpu_and_bound_nested_metadata() {
    let mut snapshot = ContainersSnapshot {
        containers: vec![Container {
            id: "a".repeat(64),
            name: "web".into(),
            cpu: Some(250.0),
            ..Default::default()
        }],
        ..Default::default()
    };
    snapshot.validate().unwrap();
    snapshot.containers[0].cpu = Some(f64::NAN);
    assert!(snapshot.validate().is_err());
    let mut inspect = ContainerInspect {
        id: "a".repeat(64),
        name: "web".into(),
        ..Default::default()
    };
    inspect.validate().unwrap();
    inspect.networks.push(Network {
        aliases: vec![String::new(); MAX_ITEMS],
        ..Default::default()
    });
    assert!(inspect.validate().is_err());
}
#[test]
fn disk_totals_add_every_category_without_wrapping() {
    let category = |size, reclaimable| DiskTotals {
        count: 1,
        size,
        reclaimable,
    };
    let mut totals = DockerTotals {
        images: category(100, 80),
        containers: category(10, 1),
        volumes: category(50, 20),
        build_cache: category(30, 30),
    };
    assert_eq!(totals.size(), 190);
    assert_eq!(totals.reclaimable(), 131);
    totals.images = category(u64::MAX, u64::MAX);
    assert_eq!(totals.size(), u64::MAX);
    assert_eq!(totals.reclaimable(), u64::MAX);
}
