use super::*;

#[tokio::test]
#[ignore = "Requires cargo xtask test remote and an isolated rootful Podman engine"]
async fn socketless_publication_survives_roundtrips_and_stays_unverified() {
    let server = crate::remote_tests::installed_server(
        "NEWPORT_TEST_PODMAN_PORT",
        "Disposable Podman",
        "fixture",
    )
    .await;
    let scratch = Scratch::new(&server).await;
    let work = Scratch {
        shell: scratch.shell.clone(),
        tag: scratch.tag.clone(),
    };
    let owner = crate::remote_tests::owner(&server);
    let query = crate::ports::query(&owner);
    let task_owner = owner.clone();
    let result = tokio::spawn(async move {
        let scratch = work;
        let query = crate::ports::query(&task_owner);
        let marker = format!("socketless-{}", scratch.tag);
        let container = scratch
            .run(
                "web",
                "-p 127.0.0.1::8080",
                &format!("mkdir -p /www; echo {marker} > /www/index.html; exec httpd -f -p 8080 -h /www"),
            )
            .await;
        let port = reachable_without_listener(&scratch, &container, &marker).await;
        let mut reading = None;
        let first = refresh(query, server.id, &mut reading).await;
        let current = &first.evidence().current;
        assert_unverified(&first);
        assert_eq!(current.coverage.ipv4.outcome, Outcome::Complete);
        assert_eq!(current.coverage.ipv6.outcome, Outcome::Complete);
        let scope = current.scope.clone();
        let member = publication(&first, &container);
        assert!(!member.stale);
        assert_eq!(member.endpoint.address.to_string(), "127.0.0.1");
        assert_eq!(member.endpoint.port, port);
        assert!(first.members().iter().all(|value| {
            value.endpoint.port != port || matches!(value.kind, Kind::Publication(_))
        }), "NAT publication must not acquire a fabricated socket");

        scratch.sh(&format!("docker stop -t 1 {container} >/dev/null")).await;
        let stopped = refresh(query, server.id, &mut reading).await;
        let current = &stopped.evidence().current;
        assert_eq!(current.scope, scope);
        assert_unverified(&stopped);
        assert!(current.publications.iter().all(|value| value.container != container));
        assert!(publication(&stopped, &container).stale);

        scratch.sh(&format!("docker start {container} >/dev/null")).await;
        let restarted_port = reachable_without_listener(&scratch, &container, &marker).await;
        let restarted = refresh(query, server.id, &mut reading).await;
        assert_unverified(&restarted);
        assert_eq!(publication(&restarted, &container).endpoint.port, restarted_port);
        assert!(!publication(&restarted, &container).stale);

        scratch.sh(&format!("docker rm -f {container} >/dev/null")).await;
        let removed = refresh(query, server.id, &mut reading).await;
        assert_unverified(&removed);
        assert!(removed.evidence().current.publications.iter().all(|value| value.container != container));
        assert!(publication(&removed, &container).stale);
        eprintln!("Socketless NAT: reachable publication, empty ss inventory, protobuf/desktop projection, stop/restart/removal and conservative saved-forward retention passed");
    }).await;
    query.close();
    scratch.cleanup().await;
    result.expect("socketless NAT lifecycle assertions");
}

fn assert_unverified(view: &View) {
    use newport_protocol::monitor::ports::discovery::IssueKind;
    let current = &view.evidence().current;
    assert!(current.container_engine.is_none());
    assert_eq!(
        current.coverage.containers.outcome,
        Outcome::Partial,
        "{:?}",
        current.coverage.containers
    );
    assert!(
        current
            .coverage
            .containers
            .issues
            .iter()
            .any(|issue| issue.kind == IssueKind::Unsupported)
    );
    assert!(
        current
            .publications
            .iter()
            .all(|value| value.engine.is_none())
    );
}

async fn reachable_without_listener(scratch: &Scratch, container: &str, marker: &str) -> u16 {
    let port = scratch
        .sh(&format!("docker port {container} 8080/tcp"))
        .await
        .strip_prefix("127.0.0.1:")
        .unwrap()
        .parse::<u16>()
        .unwrap();
    assert_eq!(
        scratch.sh(&format!("for i in $(seq 50); do curl --fail --silent --max-time 1 http://127.0.0.1:{port}/ && exit 0; sleep 0.1; done; exit 1")).await,
        marker,
    );
    let sockets = scratch.sh(&format!("ss -H -ltn 'sport = :{port}'")).await;
    assert!(
        sockets.is_empty(),
        "NAT fixture must have no host listener: {sockets}"
    );
    eprintln!("Socketless NAT fixture: HTTP reachable on {port}; ss confirms no host listener");
    port
}
