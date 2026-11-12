use super::*;

#[tokio::test]
#[ignore = "Requires cargo xtask test remote and two isolated Docker engines"]
async fn a_different_empty_engine_keeps_the_original_publication_last_observed() {
    let server = docker("fixture").await;
    let scratch = Scratch::new(&server).await;
    let owner = crate::remote_tests::owner(&server);
    let directory = format!("/home/fixture/{}", scratch.project());
    scratch.sh(&format!("mkdir -p {directory}")).await;
    let work = Scratch {
        shell: scratch.shell.clone(),
        tag: scratch.tag.clone(),
    };
    let task_owner = owner.clone();
    let task_directory = directory.clone();
    let result = tokio::spawn(async move {
        replacement(
            &work,
            &task_directory,
            server.id,
            crate::ports::query(&task_owner),
        )
        .await;
    })
    .await;
    scratch.sh(&format!("touch {directory}/release; for i in $(seq 100); do test ! -e /var/run/docker-original.sock && exit 0; sleep 0.1; done; exit 1")).await;
    crate::ports::query(&owner).close();
    scratch.cleanup().await;
    result.expect("engine replacement assertions");
}

async fn replacement(scratch: &Scratch, directory: &str, server: uuid::Uuid, query: &Query) {
    let other_engine = scratch
        .sh("docker --host unix:///var/run/docker-replacement.sock info --format '{{.ID}}'")
        .await;
    let container = scratch.run("web", "-p 127.0.0.1::8080", "sleep 600").await;
    let mut reading = None;
    let first = refresh(query, server, &mut reading).await;
    let member = publication(&first, &container);
    let port = member.endpoint.port;
    let engine = first.evidence().current.container_engine.clone().unwrap();
    assert_ne!(engine, other_engine);
    let scope = first.evidence().current.scope.clone();
    let generation = first.evidence().current.generation;

    scratch
        .sh(&format!("docker stop -t 1 {container} >/dev/null"))
        .await;
    scratch.run("switch", &format!("-v /var/run:/engine-run -v {directory}:/control -v /usr/local/bin/switch-engine:/switch-engine:ro"), "sh /switch-engine").await;
    scratch
        .sh(&format!(
            "for i in $(seq 50); do test -f {directory}/switched && exit 0; sleep 0.1; done; exit 1"
        ))
        .await;
    assert_eq!(
        scratch.sh("docker info --format '{{.ID}}'").await,
        other_engine
    );
    let replaced = refresh(query, server, &mut reading).await;
    let current = &replaced.evidence().current;
    assert_eq!(current.scope, scope);
    assert!(current.generation > generation);
    assert_eq!(current.container_engine.as_ref(), Some(&other_engine));
    assert_eq!(current.coverage.containers.outcome, Outcome::Complete);
    assert_eq!(current.coverage.ipv4.outcome, Outcome::Complete);
    assert!(current.publications.is_empty());
    assert!(
        current
            .sockets
            .iter()
            .all(|socket| socket.endpoint.port != port)
    );
    let stale = publication(&replaced, &container);
    assert!(stale.stale);
    assert_eq!(stale.reference, member.reference);
    assert!(
        replaced
            .members()
            .iter()
            .filter(|value| value.endpoint.port == port)
            .all(|value| matches!(value.kind, Kind::Publication(_)))
    );

    scratch.sh(&format!("touch {directory}/release; for i in $(seq 100); do test -f {directory}/restored && exit 0; sleep 0.1; done; exit 1")).await;
    assert_eq!(scratch.sh("docker info --format '{{.ID}}'").await, engine);
    let restored = refresh(query, server, &mut reading).await;
    assert_eq!(restored.evidence().current.scope, scope);
    assert_eq!(
        restored.evidence().current.container_engine.as_ref(),
        Some(&engine)
    );
    assert_eq!(
        restored.evidence().current.coverage.containers.outcome,
        Outcome::Complete
    );
    assert!(
        restored
            .members()
            .iter()
            .all(|value| value.endpoint.port != port)
    );
}
