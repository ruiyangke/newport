use super::*;
use crate::docker::testing::Fixture;
use newport_protocol::monitor::ports::discovery::{Snapshot, wire};
use serde_json::json;

fn row(id: char, ports: Value) -> Value {
    json!({
        "Id": id.to_string().repeat(64), "Names": ["/web"], "Image": "example:latest",
        "State": "running", "Ports": ports,
        "Labels": {"com.docker.compose.project": "demo", "com.docker.compose.service": "web", "com.docker.compose.project.working_dir": "/srv/demo"}
    })
}

fn binding(address: &str, host: u16, target: u16) -> Value {
    json!({"Type": "tcp", "IP": address, "PublicPort": host, "PrivatePort": target})
}

fn parse(rows: Vec<Value>) -> Publications {
    convert(
        Some("engine-a"),
        rows,
        &Query::new(
            Instant::now() + Duration::from_secs(2),
            RESPONSE_BYTES,
            &|| false,
        ),
    )
}

fn engine(answer: impl Fn(&str) -> (u16, String) + Send + 'static) -> (Fixture, Containers) {
    let fixture = Fixture::new(move |path| match path {
        "/version" => (200, json!({"ApiVersion": "1.47"}).to_string()),
        _ => answer(path),
    });
    let socket = fixture.socket.clone();
    let containers = Containers::new(Box::new(move || vec![socket.clone()]));
    (fixture, containers)
}

#[test]
fn nat_publications_roundtrip_without_fabricating_a_host_listener() {
    let result = parse(vec![row(
        'a',
        json!([binding("0.0.0.0", 8080, 80), binding("::", 8080, 80)]),
    )]);
    assert_eq!(result.report.outcome, Outcome::Complete);
    assert_eq!(result.values.len(), 2);
    for publication in &result.values {
        assert_eq!(publication.engine.as_deref(), Some("engine-a"));
        assert_eq!(publication.target_port, 80);
        assert_eq!(publication.target_address, None);
        assert_eq!(
            publication.compose.as_ref().unwrap().project.as_deref(),
            Some("demo")
        );
    }
    let mut snapshot = Snapshot {
        generation: 1,
        publications: result.values,
        container_engine: result.engine,
        ..Snapshot::default()
    };
    snapshot.coverage.containers = result.report;
    assert!(snapshot.sockets.is_empty());
    let bytes = wire::encode(&snapshot).unwrap();
    assert_eq!(wire::read(&mut &bytes[..]).unwrap(), snapshot);
}

#[test]
fn podman_publications_remain_visible_without_claiming_verified_engine_absence() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    let present = std::sync::Arc::new(AtomicBool::new(true));
    let running = present.clone();
    let ids = AtomicUsize::new(0);
    let fixture = Fixture::new(move |path| {
        let reply = match path {
            "/version" => json!({"ApiVersion": "1.47", "Components": [{"Name": "Podman Engine"}]}),
            "/v1.47/info" => {
                json!({"ID": format!("response-{}", ids.fetch_add(1, Ordering::SeqCst))})
            }
            "/v1.47/containers/json" if running.load(Ordering::SeqCst) => {
                json!([row('a', json!([binding("127.0.0.1", 8080, 80)]))])
            }
            "/v1.47/containers/json" => json!([]),
            _ => return (404, String::new()),
        };
        (200, reply.to_string())
    });
    let socket = fixture.socket.clone();
    let containers = Containers::new(Box::new(move || vec![socket.clone()]));
    for count in [1, 1, 0] {
        present.store(count != 0, Ordering::SeqCst);
        let value = containers.publication_inventory(
            Instant::now() + Duration::from_secs(2),
            &|| false,
            true,
        );
        assert_eq!(value.report.outcome, Outcome::Partial);
        assert_eq!(value.values.len(), count);
        assert!(value.engine.is_none());
        assert!(
            value
                .values
                .iter()
                .all(|publication| publication.engine.is_none())
        );
        assert!(
            value
                .report
                .issues
                .iter()
                .any(|issue| issue.kind == IssueKind::Unsupported)
        );
        let mut snapshot = Snapshot {
            generation: 1,
            publications: value.values,
            container_engine: value.engine,
            ..Snapshot::default()
        };
        snapshot.coverage.containers = value.report;
        assert!(snapshot.sockets.is_empty());
        let bytes = wire::encode(&snapshot).unwrap();
        assert_eq!(wire::read(&mut &bytes[..]).unwrap(), snapshot);
    }
}

#[test]
fn unavailable_podman_publications_do_not_become_partial_successes() {
    for (status, body, expected) in [
        (403, "{}", IssueKind::PermissionDenied),
        (200, "{}", IssueKind::InvalidResponse),
    ] {
        let fixture = Fixture::new(move |path| match path {
            "/version" => (
                200,
                json!({"ApiVersion": "1.47", "Platform": {"Name": "Podman Engine"}}).to_string(),
            ),
            "/v1.47/containers/json" => (status, body.into()),
            _ => (404, String::new()),
        });
        let socket = fixture.socket.clone();
        let containers = Containers::new(Box::new(move || vec![socket.clone()]));
        let value = containers.publication_inventory(
            Instant::now() + Duration::from_secs(2),
            &|| false,
            true,
        );
        assert!(value.values.is_empty());
        assert!(value.engine.is_none());
        assert_eq!(value.report.outcome, Outcome::Unavailable);
        assert_eq!(value.report.issues[0].kind, expected);
    }
}

#[test]
fn binding_identity_keeps_distinct_containers_families_and_target_ports() {
    let ports = json!([
        binding("127.0.0.1", 8080, 80),
        binding("127.0.0.1", 8080, 80),
        binding("127.0.0.1", 8080, 81),
        binding("::1", 8080, 80)
    ]);
    let result = parse(vec![row('a', ports.clone()), row('b', ports)]);
    assert_eq!(result.values.len(), 6);
    assert_eq!(result.report.outcome, Outcome::Complete);
}

#[test]
fn exposed_and_udp_ports_are_not_tcp_host_publications() {
    let result = parse(vec![row(
        'a',
        json!([
            {"Type": "tcp", "PrivatePort": 80},
            // As Docker lists a port the container exposes but doesn't publish.
            {"Type": "tcp", "PrivatePort": 6222, "IP": ""},
            {"Type": "udp", "PrivatePort": 53, "PublicPort": 53, "IP": "0.0.0.0"}
        ]),
    )]);
    assert!(result.values.is_empty());
    assert_eq!(result.report.outcome, Outcome::Complete);
}

#[test]
fn invalid_bindings_do_not_erase_valid_publications_or_invent_addresses() {
    let result = parse(vec![row(
        'a',
        json!([
            binding("127.0.0.1", 8080, 80), binding("0.0.0.0", 0, 80),
            {"Type": "tcp", "PublicPort": 8081, "PrivatePort": 81},
            {"Type": "tcp", "PublicPort": 65536, "PrivatePort": 82, "IP": "0.0.0.0"},
            {"Type": "tcp", "PublicPort": 8083, "PrivatePort": 83, "IP": "not an address"}
        ]),
    )]);
    assert_eq!(result.values.len(), 1);
    assert_eq!(result.report.outcome, Outcome::Partial);
    assert_eq!(result.report.issues[0].count, 4);
}

#[test]
fn a_container_without_ports_encoded_as_null_is_a_complete_empty_row() {
    let result = parse(vec![
        row('b', Value::Null),
        row('a', json!([binding("127.0.0.1", 8080, 80)])),
    ]);
    assert_eq!(result.values.len(), 1);
    assert_eq!(result.report.outcome, Outcome::Complete);
}

#[test]
fn malformed_container_and_overlong_metadata_leave_other_rows_usable() {
    let mut overlong = row('b', json!([binding("127.0.0.1", 80, 80)]));
    overlong["Image"] = Value::String("x".repeat(MAX_FIELD + 1));
    let result = parse(vec![
        json!({"Id": "wrong"}),
        overlong,
        row('a', json!([binding("127.0.0.1", 8080, 80)])),
    ]);
    assert_eq!(result.values.len(), 1);
    assert_eq!(result.report.outcome, Outcome::Partial);
    assert_eq!(result.report.issues[0].count, 2);
}

#[test]
fn stopped_and_unrecognized_states_do_not_claim_running() {
    let mut stopped = row('a', json!([binding("127.0.0.1", 80, 80)]));
    stopped["State"] = "exited".into();
    let mut unknown = row('b', json!([binding("127.0.0.1", 81, 80)]));
    unknown["State"] = "paused".into();
    let result = parse(vec![stopped, unknown]);
    assert_eq!(result.values[0].state, ContainerState::Stopped);
    assert_eq!(result.values[1].state, ContainerState::Unknown);
}

#[test]
fn unavailable_previously_seen_engine_is_not_an_empty_success() {
    let containers = Containers::new(Box::new(Vec::new));
    let deadline = Instant::now() + Duration::from_secs(1);
    let initial = containers.publication_inventory(deadline, &|| false, false);
    assert_eq!(initial.report.outcome, Outcome::NotApplicable);
    let later = containers.publication_inventory(deadline, &|| false, true);
    assert_eq!(later.report.outcome, Outcome::Unavailable);
    assert!(later.observed_engine);
    assert_eq!(later.report.issues[0].kind, IssueKind::Unavailable);
}

#[test]
fn denied_engine_metadata_and_invalid_list_are_explicit_failures() {
    for (status, body, expected) in [
        (403, "{}", IssueKind::PermissionDenied),
        (200, "{}", IssueKind::InvalidResponse),
    ] {
        let (_fixture, containers) = engine(move |path| {
            if path.ends_with("/info") {
                if status == 403 {
                    return (403, "{}".into());
                }
                return (200, json!({"ID": "engine-a"}).to_string());
            }
            (status, body.into())
        });
        let result = containers.publication_inventory(
            Instant::now() + Duration::from_secs(2),
            &|| false,
            false,
        );
        assert!(result.values.is_empty());
        assert!(result.observed_engine);
        assert_eq!(result.report.outcome, Outcome::Unavailable);
        assert_eq!(result.report.issues[0].kind, expected);
    }
}

#[test]
fn complete_engine_query_uses_its_real_identity_and_existing_api() {
    let (fixture, containers) = engine(|path| match path {
        "/v1.47/info" => (200, json!({"ID": "real-engine"}).to_string()),
        "/v1.47/containers/json" => (
            200,
            json!([row('a', json!([binding("127.0.0.1", 8080, 80)]))]).to_string(),
        ),
        _ => (404, "{}".into()),
    });
    let result =
        containers.publication_inventory(Instant::now() + Duration::from_secs(2), &|| false, false);
    assert_eq!(result.report.outcome, Outcome::Complete);
    assert_eq!(result.values[0].engine.as_deref(), Some("real-engine"));
    assert_eq!(
        *fixture.requests.lock().unwrap(),
        [
            "/version",
            "/v1.47/info",
            "/v1.47/containers/json",
            "/v1.47/info"
        ]
    );
}

#[test]
fn empty_successful_inventory_differs_from_cancellation() {
    let (_fixture, containers) = engine(|path| {
        (
            200,
            if path.ends_with("/info") {
                json!({"ID": "engine-a"}).to_string()
            } else {
                "[]".into()
            },
        )
    });
    let deadline = Instant::now() + Duration::from_secs(2);
    let empty = containers.publication_inventory(deadline, &|| false, false);
    assert!(empty.values.is_empty());
    assert_eq!(empty.engine.as_deref(), Some("engine-a"));
    assert_eq!(empty.report.outcome, Outcome::Complete);
    let cancelled = containers.publication_inventory(deadline, &|| true, true);
    assert_eq!(cancelled.report.outcome, Outcome::Unavailable);
    assert_eq!(cancelled.report.issues[0].kind, IssueKind::Interrupted);
}

#[test]
fn changed_or_unverifiable_engine_cannot_publish_rows_or_claim_empty_success() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    for rows in [
        json!([]),
        json!([row('a', json!([binding("127.0.0.1", 8080, 80)]))]),
    ] {
        for (status, body, expected) in [
            (200, json!({"ID": "engine-b"}), IssueKind::InvalidResponse),
            (200, json!({"ID": ""}), IssueKind::InvalidResponse),
            (403, json!({}), IssueKind::PermissionDenied),
        ] {
            let calls = AtomicUsize::new(0);
            let rows = rows.clone();
            let (_fixture, containers) = engine(move |path| {
                if path.ends_with("/info") {
                    if calls.fetch_add(1, Ordering::AcqRel) == 0 {
                        return (200, json!({"ID": "engine-a"}).to_string());
                    }
                    return (status, body.to_string());
                }
                (200, rows.to_string())
            });
            let result = containers.publication_inventory(
                Instant::now() + Duration::from_secs(2),
                &|| false,
                true,
            );
            assert_eq!(result.report.outcome, Outcome::Unavailable);
            assert!(result.values.is_empty());
            assert!(result.observed_engine);
            assert!(result.engine.is_none());
            assert_eq!(result.report.issues[0].kind, expected);
        }
    }
}

#[test]
fn large_container_and_publication_lists_report_truncation() {
    let result = parse(vec![row('a', json!([])); MAX_CONTAINERS + 1]);
    assert_eq!(result.report.outcome, Outcome::Partial);
    assert_eq!(result.report.issues[0].kind, IssueKind::BudgetExceeded);
    let ports: Vec<_> = (1..=MAX_LISTENERS + 1)
        .map(|port| binding("127.0.0.1", u16::try_from(port).unwrap(), 80))
        .collect();
    let result = parse(vec![row('a', json!(ports))]);
    assert!(result.values.len() <= MAX_LISTENERS);
    assert_eq!(result.report.outcome, Outcome::Partial);
    assert_eq!(result.report.issues[0].kind, IssueKind::BudgetExceeded);
}
