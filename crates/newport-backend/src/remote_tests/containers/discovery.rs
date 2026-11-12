use super::{Scratch, docker};
use crate::ports::{Query, tests::pipeline::desktop_roundtrip};
use newport_client::discovery::{Reading, View};
use newport_domain::port_discovery::Kind;
use newport_protocol::monitor::ports::discovery::{ContainerState, Outcome};
use std::sync::Arc;

mod nat;
mod replacement;

async fn refresh(query: &Query, server: uuid::Uuid, previous: &mut Option<Reading>) -> Arc<View> {
    query.refresh().await.unwrap();
    let observation = desktop_roundtrip(query.snapshot(), server);
    let reading = Reading::prepare(Ok(observation), previous.as_ref());
    let view = reading.view().unwrap().clone();
    assert!(view.inventory().fresh);
    *previous = Some(reading);
    view
}

fn publication<'a>(view: &'a View, id: &str) -> &'a newport_domain::port_discovery::Member {
    view.members()
        .iter()
        .find(|member| matches!(&member.kind, Kind::Publication(value) if value.container == id))
        .unwrap_or_else(|| panic!("publication {id} missing: {view:?}"))
}

#[tokio::test]
#[ignore = "Requires cargo xtask test remote and an isolated Docker engine"]
async fn publication_permission_loss_and_removal_keep_the_last_observation() {
    let server = docker("fixture").await;
    let scratch = Scratch::new(&server).await;
    let owner = crate::remote_tests::owner(&server);
    let query = crate::ports::query(&owner);
    let directory = format!("/home/fixture/{}", scratch.project());
    scratch.sh(&format!("mkdir -p {directory}")).await;
    let work = Scratch {
        shell: scratch.shell.clone(),
        tag: scratch.tag.clone(),
    };
    let task_owner = owner.clone();
    let task_directory = directory.clone();
    let result = tokio::spawn(async move {
        let scratch = work;
        let directory = task_directory;
        let query = crate::ports::query(&task_owner);
        let container = scratch.run("web", "-p 127.0.0.1::8080", "sleep 600").await;
        let port = scratch
            .sh(&format!("docker port {container} 8080/tcp"))
            .await
            .strip_prefix("127.0.0.1:")
            .unwrap()
            .parse::<u16>()
            .unwrap();
        let mut reading = None;
        let first = refresh(query, server.id, &mut reading).await;
        let current = &first.evidence().current;
        assert_eq!(current.coverage.containers.outcome, Outcome::Complete);
        assert_eq!(current.coverage.ipv4.outcome, Outcome::Complete);
        let engine = current.container_engine.clone().unwrap();
        let scope = current.scope.clone();
        let generation = current.generation;
        let member = publication(&first, &container);
        assert!(!member.stale);
        assert_eq!(member.endpoint.address.to_string(), "127.0.0.1");
        assert_eq!(member.endpoint.port, port);
        assert!(current.publications.iter().any(|value| {
            value.container == container && value.target_port == 8080
                && value.state == ContainerState::Running
        }));
        // Docker reserves a socket even when its userland proxy is disabled.
        let reservation = first.members().iter().find(|value| {
            value.endpoint == member.endpoint && matches!(value.kind, Kind::Socket(_))
        }).expect("Docker's reservation socket must stay distinct from its publication");
        assert_ne!(reservation.reference, member.reference);

        let mode = scratch.sh("stat -c '%a' /var/run/docker.sock").await;
        assert!(matches!(mode.as_str(), "660" | "666"));
        scratch.run("access", &format!("-v /var/run/docker.sock:/engine.sock -v {directory}:/control"),
            &format!("trap \"chmod {mode} /engine.sock\" EXIT; chmod 600 /engine.sock; touch /control/denied; i=0; while [ ! -f /control/release ] && [ $i -lt 300 ]; do sleep 0.1; i=$((i+1)); done")
        ).await;
        scratch.sh(&format!("for i in $(seq 50); do test -f {directory}/denied && exit 0; sleep 0.1; done; exit 1")).await;
        let denied = refresh(query, server.id, &mut reading).await;
        let current = &denied.evidence().current;
        assert_eq!(current.scope, scope);
        assert!(current.generation > generation);
        assert_eq!(current.coverage.containers.outcome, Outcome::Unavailable);
        assert!(current.publications.is_empty());
        assert!(publication(&denied, &container).stale);

        restore(&scratch, &directory).await;
        let restored = refresh(query, server.id, &mut reading).await;
        assert_eq!(restored.evidence().current.container_engine.as_ref(), Some(&engine));
        assert_eq!(restored.evidence().current.scope, scope);
        assert!(!publication(&restored, &container).stale);

        scratch.sh(&format!("docker stop -t 1 {container} >/dev/null")).await;
        let stopped = refresh(query, server.id, &mut reading).await;
        assert_eq!(stopped.evidence().current.coverage.containers.outcome, Outcome::Complete);
        assert_eq!(stopped.evidence().current.container_engine.as_ref(), Some(&engine));
        assert!(stopped.members().iter().all(|member| member.endpoint.port != port));
        scratch.sh(&format!("docker rm {container} >/dev/null")).await;
        let removed = refresh(query, server.id, &mut reading).await;
        assert_eq!(removed.evidence().current.scope, scope);
        assert_eq!(removed.evidence().current.coverage.containers.outcome, Outcome::Complete);
    }).await;
    restore(&scratch, &directory).await;
    query.close();
    scratch.cleanup().await;
    result.expect("discovery lifecycle assertions");
}

async fn restore(scratch: &Scratch, directory: &str) {
    scratch.sh(&format!("touch {directory}/release; for i in $(seq 100); do test -w /var/run/docker.sock && exit 0; sleep 0.1; done; exit 1")).await;
}
