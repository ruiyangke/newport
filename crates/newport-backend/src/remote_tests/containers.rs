//! The Containers tab through the agent against a real Docker Engine
//! (tests/remote/docker: Docker-in-Docker, so nothing reaches the host's
//! engine). `fixture` is in the docker group; `outsider` is not. Each test
//! labels what it creates with its own tag, asserts only about those, and
//! removes them afterwards. Compose projects are made with the real
//! `docker compose` plugin from a local image; nothing is pulled.
use crate::{
    containers::{self, ContainerVerb, ProjectVerb, PruneTarget},
    model::Server,
};
use serde_json::{Value, json};
use tokio::time::{Duration, sleep};
use uuid::Uuid;

const IMAGE: &str = "newport-test/busybox:local";
const CHANGED: &str =
    "Project containers changed since you opened it. Refresh and review the project again.";

pub(super) async fn docker(user: &str) -> Server {
    super::installed_server("NEWPORT_TEST_DOCKER_PORT", "Disposable Docker", user).await
}
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

/// A test's own containers, volumes, networks and Compose project.
pub(super) struct Scratch {
    shell: crate::remote::Shell,
    pub(super) tag: String,
}
impl Scratch {
    pub(super) async fn new(server: &Server) -> Self {
        let tag = Uuid::new_v4().simple().to_string()[..10].to_owned();
        Self {
            shell: crate::remote::Shell::new(server),
            tag,
        }
    }
    pub(super) async fn sh(&self, command: &str) -> String {
        self.shell
            .execute(command, None)
            .await
            .unwrap_or_else(|e| panic!("{command}: {e}"))
            .trim()
            .to_owned()
    }
    fn name(&self, role: &str) -> String {
        format!("np-{}-{role}", self.tag)
    }
    pub(super) fn project(&self) -> String {
        format!("np{}", self.tag)
    }
    /// `docker run -d` with this test's label; returns the full id.
    async fn run(&self, role: &str, options: &str, command: &str) -> String {
        self.sh(&format!(
            "docker run -d --name {} --label newport.test={} {options} {IMAGE} sh -c '{command}'",
            self.name(role),
            self.tag
        ))
        .await
    }
    /// A real Compose project from `services` (YAML under `services:`).
    async fn compose(&self, services: &str) {
        let dir = format!("~/{}", self.project());
        self.sh(&format!(
            "mkdir -p {dir} && cat > {dir}/compose.yaml <<'EOF'\nservices:\n{services}\nEOF\ncd {dir} && docker compose -p {} up -d --quiet-pull 2>&1",
            self.project()
        ))
        .await;
    }
    pub(super) async fn cleanup(self) {
        let (tag, project) = (&self.tag, self.project());
        self.sh(&format!(
            "docker ps -aq --filter label=newport.test={tag} | xargs -r docker rm -f >/dev/null; \
             docker ps -aq --filter label=com.docker.compose.project={project} | xargs -r docker rm -f >/dev/null; \
             docker volume ls -q --filter label=newport.test={tag} | xargs -r docker volume rm >/dev/null; \
             docker network ls -q --filter label=newport.test={tag} | xargs -r docker network rm >/dev/null; \
             docker network ls -q --filter label=com.docker.compose.project={project} | xargs -r docker network rm >/dev/null; \
             rm -rf ~/{project}; true"
        ))
        .await;
    }
}

fn find<'a>(snapshot: &'a Value, name: &str) -> Option<&'a Value> {
    snapshot["containers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
}
/// Snapshots until `ready` holds; health checks and restarts take a moment.
async fn settled(server: &Server, ready: impl Fn(&Value) -> bool) -> Value {
    let mut snapshot = Value::Null;
    for _ in 0..60 {
        snapshot = containers::snapshot(&crate::remote_tests::owner(server))
            .await
            .unwrap();
        if ready(&snapshot) {
            return snapshot;
        }
        sleep(Duration::from_millis(500)).await;
    }
    panic!("containers never settled: {snapshot:#}");
}
async fn state_of(server: &Server, name: &str) -> String {
    let snapshot = containers::snapshot(&crate::remote_tests::owner(server))
        .await
        .unwrap();
    find(&snapshot, name).unwrap()["state"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
#[ignore = "Requires cargo xtask test remote"]
async fn snapshot_reports_state_health_ports_compose_and_figures() {
    let server = docker("fixture").await;
    let scratch = Scratch::new(&server).await;
    let web = scratch
        .run(
            "web",
            "--init -p 127.0.0.1::8080 --expose 9000 --memory 64m --restart unless-stopped \
             --health-cmd true --health-interval 1s",
            "while :; do i=0; while [ $i -lt 20000 ]; do i=$((i+1)); done; sleep 0.2; done",
        )
        .await;
    let job = scratch.run("job", "", "exit 3").await;
    scratch.sh(&format!("docker wait {job}")).await;
    scratch
        .compose(&format!(
            "  api:\n    image: {IMAGE}\n    pull_policy: never\n    init: true\n    command: sleep 600\n  db:\n    image: {IMAGE}\n    pull_policy: never\n    init: true\n    command: sleep 600"
        ))
        .await;

    // The agent behind this server id is new: CPU needs a second reading.
    let first = containers::snapshot(&crate::remote_tests::owner(&server))
        .await
        .unwrap();
    let row = find(&first, &scratch.name("web")).unwrap();
    assert_eq!(row["cpu"], Value::Null, "{row:#}");
    assert!(row["memory"].as_u64().is_some_and(|m| m > 0), "{row:#}");

    let snapshot = settled(&server, |s| {
        find(s, &scratch.name("web")).is_some_and(|w| w["health"] == "healthy" && w["cpu"].is_f64())
    })
    .await;
    assert!(
        snapshot["engine"]["version"]
            .as_str()
            .is_some_and(|v| !v.is_empty())
    );
    assert_eq!(snapshot["engine"]["apiVersion"], "1.47");
    assert_eq!(snapshot["engine"]["rootless"], false);
    assert_eq!(snapshot["engine"]["podman"], false);
    assert_eq!(snapshot["truncated"], false);
    // Newest first.
    let created: Vec<i64> = snapshot["containers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["created"].as_i64().unwrap())
        .collect();
    assert!(created.windows(2).all(|w| w[0] >= w[1]));

    let row = find(&snapshot, &scratch.name("web")).unwrap();
    assert_eq!(row["id"], web);
    for (field, value) in [
        ("image", json!(IMAGE)),
        ("state", json!("running")),
        ("health", json!("healthy")),
        ("exitCode", Value::Null),
        ("oomKilled", json!(false)),
        ("error", Value::Null),
        ("restartCount", json!(0)),
        ("restartPolicy", json!("unless-stopped")),
        ("finishedAt", Value::Null),
        ("compose", Value::Null),
        ("memoryLimit", json!(64 << 20)),
    ] {
        assert_eq!(row[field], value, "{field} in {row:#}");
    }
    assert!(row["status"].as_str().unwrap().starts_with("Up"));
    assert!(row["imageId"].as_str().unwrap().starts_with("sha256:"));
    assert!(row["command"].as_str().unwrap().contains("sh -c"));
    let started = row["startedAt"].as_i64().unwrap();
    assert!(started >= row["created"].as_i64().unwrap() - 1000 && started <= now());
    assert!(row["cpu"].as_f64().is_some_and(|c| c >= 0.0), "{row:#}");
    assert!(
        row["memory"]
            .as_u64()
            .is_some_and(|m| m > 0 && m < 64 << 20)
    );
    let ports = row["ports"].as_array().unwrap();
    assert_eq!(ports.len(), 2, "{ports:?}");
    assert_eq!(ports[0]["ip"], "127.0.0.1");
    assert!(ports[0]["hostPort"].as_u64().is_some_and(|p| p > 0));
    assert_eq!(ports[0]["containerPort"], 8080);
    assert_eq!(ports[0]["protocol"], "tcp");
    assert_eq!(
        ports[1],
        json!({"ip": null, "hostPort": null, "containerPort": 9000, "protocol": "tcp"})
    );

    let row = find(&snapshot, &scratch.name("job")).unwrap();
    assert_eq!(row["state"], "exited");
    assert_eq!(row["exitCode"], 3);
    assert_eq!(row["health"], Value::Null);
    assert_eq!(row["restartPolicy"], "no");
    assert!(row["finishedAt"].as_i64().is_some_and(|f| f <= now()));
    assert!(row["status"].as_str().unwrap().starts_with("Exited (3)"));
    for field in ["cpu", "memory", "memoryLimit"] {
        assert_eq!(row[field], Value::Null, "{field}");
    }

    let project = scratch.project();
    let api = find(&snapshot, &format!("{project}-api-1")).unwrap();
    assert_eq!(api["state"], "running");
    assert_eq!(api["compose"]["project"], json!(project));
    assert_eq!(api["compose"]["service"], "api");
    assert_eq!(api["compose"]["oneoff"], false);
    assert_eq!(api["compose"]["number"], 1);
    assert_eq!(
        api["compose"]["workingDir"],
        json!(format!("/home/fixture/{project}"))
    );
    assert_eq!(
        api["compose"]["configFiles"],
        json!(format!("/home/fixture/{project}/compose.yaml"))
    );
    assert_eq!(api["memoryLimit"], Value::Null, "unlimited");
    assert!(find(&snapshot, &format!("{project}-db-1")).is_some());
    scratch.cleanup().await;
}

#[tokio::test]
#[ignore = "Requires cargo xtask test remote"]
async fn inspect_reports_env_mounts_networks_and_labels() {
    let server = docker("fixture").await;
    let scratch = Scratch::new(&server).await;
    let (volume, network) = (scratch.name("data"), scratch.name("net"));
    scratch
        .sh(&format!(
            "docker volume create --label newport.test={0} {volume} >/dev/null && docker network create --label newport.test={0} {network} >/dev/null",
            scratch.tag
        ))
        .await;
    let id = scratch
        .run(
            "app",
            &format!(
                "--init -e TOKEN=s3cret=value -e EMPTY= -v {volume}:/data -v /tmp:/host-tmp:ro --tmpfs /scratch \
                 --network {network} --network-alias api --label extra=yes --workdir /data --user 1000 \
                 --hostname np-host --restart on-failure:3 --health-cmd 'echo ok' --health-interval 1s"
            ),
            "sleep 600",
        )
        .await;
    let mut inspect = Value::Null;
    for _ in 0..30 {
        inspect = containers::inspect(&crate::remote_tests::owner(&server), &scratch.name("app"))
            .await
            .unwrap();
        if !inspect["lastHealth"].as_array().unwrap().is_empty() {
            break;
        }
        sleep(Duration::from_millis(500)).await;
    }
    assert_eq!(inspect["id"], id);
    assert_eq!(inspect["name"], json!(scratch.name("app")));
    assert_eq!(inspect["image"], IMAGE);
    assert!(inspect["created"].as_i64().is_some_and(|c| c <= now()));
    assert_eq!(inspect["entrypoint"], Value::Null);
    assert_eq!(inspect["cmd"], json!(["sh", "-c", "sleep 600"]));
    assert_eq!(inspect["workingDir"], "/data");
    assert_eq!(inspect["user"], "1000");
    assert_eq!(inspect["hostname"], "np-host");
    assert_eq!(inspect["restartPolicy"], "on-failure");
    let env = inspect["env"].as_array().unwrap();
    assert!(env.contains(&json!({"name": "TOKEN", "value": "s3cret=value"})));
    assert!(env.contains(&json!({"name": "EMPTY", "value": ""})));
    let mounts = inspect["mounts"].as_array().unwrap();
    let data = mounts.iter().find(|m| m["destination"] == "/data").unwrap();
    assert_eq!(data["type"], "volume");
    assert_eq!(data["name"], json!(volume));
    assert_eq!(data["rw"], true);
    let host = mounts
        .iter()
        .find(|m| m["destination"] == "/host-tmp")
        .unwrap();
    assert_eq!(host["type"], "bind");
    assert_eq!(host["source"], "/tmp");
    assert_eq!(host["rw"], false);
    assert_eq!(host["name"], Value::Null);
    let networks = inspect["networks"].as_array().unwrap();
    assert_eq!(networks.len(), 1);
    assert_eq!(networks[0]["name"], json!(network));
    assert!(
        networks[0]["ipAddress"]
            .as_str()
            .is_some_and(|ip| ip.contains('.'))
    );
    assert!(
        networks[0]["aliases"]
            .as_array()
            .unwrap()
            .contains(&json!("api"))
    );
    assert_eq!(inspect["labels"]["extra"], "yes");
    assert_eq!(inspect["labels"]["newport.test"], json!(scratch.tag));
    assert_eq!(
        inspect["healthcheck"],
        json!({"test": ["CMD-SHELL", "echo ok"], "interval": 1000})
    );
    let health = inspect["lastHealth"].as_array().unwrap();
    assert!(!health.is_empty() && health.len() <= 5);
    assert_eq!(health.last().unwrap()["exitCode"], 0);
    assert_eq!(health.last().unwrap()["output"], "ok\n");

    // The id prefix works as well as the name; a missing container is the
    // engine's own error; a bad target never leaves the desktop.
    assert_eq!(
        containers::inspect(&crate::remote_tests::owner(&server), &id[..12])
            .await
            .unwrap()["id"],
        id
    );
    let missing = containers::inspect(&crate::remote_tests::owner(&server), &scratch.name("gone"))
        .await
        .unwrap_err();
    assert!(missing.contains("No such container"), "{missing}");
    assert_eq!(
        containers::inspect(&crate::remote_tests::owner(&server), "../info")
            .await
            .unwrap_err(),
        "Not a valid container: ../info"
    );
    scratch.cleanup().await;
}

fn messages(page: &Value) -> Vec<String> {
    page["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["message"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
#[ignore = "Requires cargo xtask test remote"]
async fn logs_page_tail_streams_and_project_merge() {
    let server = docker("fixture").await;
    let scratch = Scratch::new(&server).await;
    let name = scratch.name("talker");
    scratch
        .run(
            "talker",
            "",
            "for i in 1 2 3 4 5; do echo out-$i; sleep 0.1; echo err-$i >&2; sleep 0.1; done; exec sleep 600",
        )
        .await;
    let mut first = Value::Null;
    for _ in 0..30 {
        first = containers::logs(
            &crate::remote_tests::owner(&server),
            Some(&name),
            None,
            None,
            Some(4),
        )
        .await
        .unwrap();
        if messages(&first).last().map(String::as_str) == Some("err-5") {
            break;
        }
        sleep(Duration::from_millis(300)).await;
    }
    assert_eq!(messages(&first), ["out-4", "err-4", "out-5", "err-5"]);
    let entries = first["entries"].as_array().unwrap();
    let streams: Vec<&str> = entries
        .iter()
        .map(|e| e["stream"].as_str().unwrap())
        .collect();
    assert_eq!(streams, ["stdout", "stderr", "stdout", "stderr"]);
    assert!(entries.iter().all(|e| e["container"].is_null()));
    let times: Vec<i64> = entries.iter().map(|e| e["at"].as_i64().unwrap()).collect();
    assert!(times.windows(2).all(|w| w[0] <= w[1]) && times[3] <= now());
    let all = containers::logs(
        &crate::remote_tests::owner(&server),
        Some(&name),
        None,
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(messages(&all).len(), 10);

    // Nothing new: no entries and the same cursor.
    let cursor = first["cursor"].as_str().unwrap().to_owned();
    let quiet = containers::logs(
        &crate::remote_tests::owner(&server),
        Some(&name),
        None,
        Some(&cursor),
        None,
    )
    .await
    .unwrap();
    assert_eq!(quiet, json!({"entries": [], "cursor": cursor}));

    // New lines (written to the container's own stdout) arrive once each.
    scratch
        .sh(&format!(
            "docker exec {name} sh -c 'echo later-1 > /proc/1/fd/1; echo later-2 > /proc/1/fd/1'"
        ))
        .await;
    let mut seen = Vec::new();
    let mut cursor = cursor;
    for _ in 0..20 {
        let page = containers::logs(
            &crate::remote_tests::owner(&server),
            Some(&name),
            None,
            Some(&cursor),
            None,
        )
        .await
        .unwrap();
        seen.extend(messages(&page));
        cursor = page["cursor"].as_str().unwrap().to_owned();
        if seen.len() >= 2 {
            break;
        }
        sleep(Duration::from_millis(250)).await;
    }
    assert_eq!(seen, ["later-1", "later-2"]);
    let after = containers::logs(
        &crate::remote_tests::owner(&server),
        Some(&name),
        None,
        Some(&cursor),
        None,
    )
    .await
    .unwrap();
    assert!(after["entries"].as_array().unwrap().is_empty());

    // A TTY container's output is raw, without the terminal's carriage return.
    let tty = scratch.name("tty");
    scratch
        .run("tty", "-t", "echo tty-line; exec sleep 600")
        .await;
    let mut page = Value::Null;
    for _ in 0..20 {
        page = containers::logs(
            &crate::remote_tests::owner(&server),
            Some(&tty),
            None,
            None,
            None,
        )
        .await
        .unwrap();
        if !messages(&page).is_empty() {
            break;
        }
        sleep(Duration::from_millis(250)).await;
    }
    assert_eq!(messages(&page), ["tty-line"]);
    assert_eq!(page["entries"][0]["stream"], "stdout");

    // A project's services, merged by time and named by service.
    scratch
        .compose(&format!(
            "  a:\n    image: {IMAGE}\n    pull_policy: never\n    init: true\n    command: sh -c 'echo a-1; sleep 1; echo a-2; exec sleep 600'\n  b:\n    image: {IMAGE}\n    pull_policy: never\n    init: true\n    command: sh -c 'sleep 0.5; echo b-1; sleep 1; echo b-2; exec sleep 600'"
        ))
        .await;
    let project = scratch.project();
    let mut merged = Value::Null;
    for _ in 0..30 {
        merged = containers::logs(
            &crate::remote_tests::owner(&server),
            None,
            Some(&project),
            None,
            None,
        )
        .await
        .unwrap();
        if messages(&merged).len() >= 4 {
            break;
        }
        sleep(Duration::from_millis(300)).await;
    }
    // Merged by time; each service keeps its own order and is named by service.
    let entries = merged["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 4, "{merged:#}");
    let times: Vec<i64> = entries.iter().map(|e| e["at"].as_i64().unwrap()).collect();
    assert!(times.windows(2).all(|w| w[0] <= w[1]), "{merged:#}");
    for service in ["a", "b"] {
        let own: Vec<String> = entries
            .iter()
            .filter(|e| e["container"] == service)
            .map(|e| e["message"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(own, [format!("{service}-1"), format!("{service}-2")]);
    }
    let cursor = merged["cursor"].as_str().unwrap().to_owned();
    let quiet = containers::logs(
        &crate::remote_tests::owner(&server),
        None,
        Some(&project),
        Some(&cursor),
        None,
    )
    .await
    .unwrap();
    assert_eq!(quiet, json!({"entries": [], "cursor": cursor}));
    scratch
        .sh(&format!(
            "docker exec {project}-b-1 sh -c 'echo b-3 > /proc/1/fd/1'"
        ))
        .await;
    let mut tail = Value::Null;
    for _ in 0..20 {
        tail = containers::logs(
            &crate::remote_tests::owner(&server),
            None,
            Some(&project),
            Some(&cursor),
            None,
        )
        .await
        .unwrap();
        if !messages(&tail).is_empty() {
            break;
        }
        sleep(Duration::from_millis(250)).await;
    }
    assert_eq!(messages(&tail), ["b-3"]);
    assert_eq!(tail["entries"][0]["container"], "b");

    assert!(
        containers::logs(
            &crate::remote_tests::owner(&server),
            Some(&name),
            Some(&project),
            None,
            None
        )
        .await
        .is_err()
    );
    scratch.cleanup().await;
}

#[tokio::test]
#[ignore = "Requires cargo xtask test remote"]
async fn actions_change_state_and_pass_on_engine_errors() {
    let server = docker("fixture").await;
    let scratch = Scratch::new(&server).await;
    let name = scratch.name("svc");
    scratch.run("svc", "--init", "exec sleep 600").await;
    let started = |snapshot: &Value| find(snapshot, &name).unwrap()["startedAt"].as_i64();
    let before = started(
        &containers::snapshot(&crate::remote_tests::owner(&server))
            .await
            .unwrap(),
    );

    containers::action(
        &crate::remote_tests::owner(&server),
        &name,
        ContainerVerb::Stop,
    )
    .await
    .unwrap();
    assert_eq!(state_of(&server, &name).await, "exited");
    let stopped = containers::snapshot(&crate::remote_tests::owner(&server))
        .await
        .unwrap();
    // `--init` forwards SIGTERM, so the stop is clean rather than a kill.
    assert_eq!(find(&stopped, &name).unwrap()["exitCode"], 143);
    let error = containers::action(
        &crate::remote_tests::owner(&server),
        &name,
        ContainerVerb::Pause,
    )
    .await
    .unwrap_err();
    assert!(error.contains("is not running"), "{error}");

    assert_eq!(
        containers::action(
            &crate::remote_tests::owner(&server),
            &name,
            ContainerVerb::Start
        )
        .await
        .unwrap(),
        json!({"ok": true})
    );
    assert_eq!(state_of(&server, &name).await, "running");
    // Starting a running container is not an error.
    containers::action(
        &crate::remote_tests::owner(&server),
        &name,
        ContainerVerb::Start,
    )
    .await
    .unwrap();
    sleep(Duration::from_millis(1100)).await;
    containers::action(
        &crate::remote_tests::owner(&server),
        &name,
        ContainerVerb::Restart,
    )
    .await
    .unwrap();
    let restarted = containers::snapshot(&crate::remote_tests::owner(&server))
        .await
        .unwrap();
    assert_eq!(find(&restarted, &name).unwrap()["state"], "running");
    assert!(started(&restarted) > before, "restart moves startedAt");

    containers::action(
        &crate::remote_tests::owner(&server),
        &name,
        ContainerVerb::Pause,
    )
    .await
    .unwrap();
    assert_eq!(state_of(&server, &name).await, "paused");
    let error = containers::action(
        &crate::remote_tests::owner(&server),
        &name,
        ContainerVerb::Pause,
    )
    .await
    .unwrap_err();
    assert!(error.contains("already paused"), "{error}");
    containers::action(
        &crate::remote_tests::owner(&server),
        &name,
        ContainerVerb::Unpause,
    )
    .await
    .unwrap();
    assert_eq!(state_of(&server, &name).await, "running");

    let missing = containers::action(
        &crate::remote_tests::owner(&server),
        &scratch.name("gone"),
        ContainerVerb::Stop,
    )
    .await
    .unwrap_err();
    assert!(missing.contains("No such container"), "{missing}");
    assert!(
        containers::action(
            &crate::remote_tests::owner(&server),
            "a b",
            ContainerVerb::Stop
        )
        .await
        .is_err()
    );
    scratch.cleanup().await;
}

#[tokio::test]
#[ignore = "Requires cargo xtask test remote"]
async fn project_action_requires_the_reviewed_containers() {
    let server = docker("fixture").await;
    let scratch = Scratch::new(&server).await;
    scratch
        .compose(&format!(
            "  web:\n    image: {IMAGE}\n    pull_policy: never\n    init: true\n    command: sleep 600\n    depends_on: [db]\n  db:\n    image: {IMAGE}\n    pull_policy: never\n    init: true\n    command: sleep 600"
        ))
        .await;
    let project = scratch.project();
    // A one-off `compose run` belongs to the project but not to its actions.
    scratch
        .sh(&format!(
            "cd ~/{project} && docker compose -p {project} run -d --name {project}-job web sleep 600"
        ))
        .await;
    let snapshot = containers::snapshot(&crate::remote_tests::owner(&server))
        .await
        .unwrap();
    let members: Vec<&Value> = snapshot["containers"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["compose"]["project"] == json!(project))
        .collect();
    assert_eq!(members.len(), 3);
    let id = |c: &&Value| c["id"].as_str().unwrap().to_owned();
    let services: Vec<String> = members
        .iter()
        .filter(|c| c["compose"]["oneoff"] == false)
        .map(id)
        .collect();
    let oneoff = members
        .iter()
        .find(|c| c["compose"]["oneoff"] == true)
        .map(id)
        .unwrap();
    assert_eq!(services.len(), 2);

    let partial = containers::project_action(
        &crate::remote_tests::owner(&server),
        &project,
        ProjectVerb::Stop,
        &services[..1],
    )
    .await
    .unwrap_err();
    assert_eq!(partial, CHANGED);
    let mut extra = services.clone();
    extra.push(oneoff.clone());
    let with_oneoff = containers::project_action(
        &crate::remote_tests::owner(&server),
        &project,
        ProjectVerb::Stop,
        &extra,
    )
    .await
    .unwrap_err();
    assert_eq!(with_oneoff, CHANGED);

    assert_eq!(
        containers::project_action(
            &crate::remote_tests::owner(&server),
            &project,
            ProjectVerb::Stop,
            &services
        )
        .await
        .unwrap(),
        json!({"ok": true})
    );
    let after = containers::snapshot(&crate::remote_tests::owner(&server))
        .await
        .unwrap();
    for name in ["web", "db"] {
        assert_eq!(
            find(&after, &format!("{project}-{name}-1")).unwrap()["state"],
            "exited"
        );
    }
    assert_eq!(
        find(&after, &format!("{project}-job")).unwrap()["state"],
        "running"
    );
    // Stopped in reverse creation order: the dependent first.
    let finished = |name: &str| {
        find(&after, &format!("{project}-{name}-1")).unwrap()["finishedAt"]
            .as_i64()
            .unwrap()
    };
    assert!(finished("web") <= finished("db"));

    containers::project_action(
        &crate::remote_tests::owner(&server),
        &project,
        ProjectVerb::Start,
        &services,
    )
    .await
    .unwrap();
    containers::project_action(
        &crate::remote_tests::owner(&server),
        &project,
        ProjectVerb::Restart,
        &services,
    )
    .await
    .unwrap();
    let after = containers::snapshot(&crate::remote_tests::owner(&server))
        .await
        .unwrap();
    for name in ["web", "db"] {
        assert_eq!(
            find(&after, &format!("{project}-{name}-1")).unwrap()["state"],
            "running"
        );
    }
    // A removed member is a change too.
    scratch
        .sh(&format!("docker rm -f {project}-db-1 >/dev/null"))
        .await;
    assert_eq!(
        containers::project_action(
            &crate::remote_tests::owner(&server),
            &project,
            ProjectVerb::Stop,
            &services
        )
        .await
        .unwrap_err(),
        CHANGED
    );
    scratch.cleanup().await;
}

#[tokio::test]
#[ignore = "Requires cargo xtask test remote"]
async fn remove_refuses_running_containers_and_keeps_volumes() {
    let server = docker("fixture").await;
    let scratch = Scratch::new(&server).await;
    let running = scratch.run("up", "--init", "exec sleep 600").await;
    let done = scratch.run("done", "-v /data", "echo bye").await;
    scratch.sh(&format!("docker wait {done}")).await;
    let volume = scratch
        .sh(&format!(
            "docker inspect -f '{{{{range .Mounts}}}}{{{{.Name}}}}{{{{end}}}}' {done}"
        ))
        .await;
    assert_eq!(volume.len(), 64, "anonymous volume: {volume}");

    let refused = containers::remove(
        &crate::remote_tests::owner(&server),
        &[done.clone(), running.clone()],
    )
    .await
    .unwrap_err();
    assert_eq!(
        refused,
        format!("{} is running. Stop it first.", scratch.name("up"))
    );
    // Nothing was removed, not even the stopped one.
    assert_eq!(state_of(&server, &scratch.name("done")).await, "exited");

    assert_eq!(
        containers::remove(
            &crate::remote_tests::owner(&server),
            std::slice::from_ref(&done)
        )
        .await
        .unwrap(),
        json!({"removed": 1})
    );
    let snapshot = containers::snapshot(&crate::remote_tests::owner(&server))
        .await
        .unwrap();
    assert!(find(&snapshot, &scratch.name("done")).is_none());
    assert!(find(&snapshot, &scratch.name("up")).is_some());
    // Volumes are never removed with their container.
    assert_eq!(
        scratch
            .sh(&format!("docker volume ls -q --filter name={volume}"))
            .await,
        volume
    );
    scratch
        .sh(&format!("docker volume rm {volume} >/dev/null"))
        .await;
    scratch.cleanup().await;
}

#[tokio::test]
#[ignore = "Requires cargo xtask test remote"]
async fn disk_usage_and_prune() {
    let server = docker("fixture").await;
    let scratch = Scratch::new(&server).await;
    let named = scratch.name("keep");
    scratch
        .sh(&format!(
            "docker volume create --label newport.test={} {named} >/dev/null",
            scratch.tag
        ))
        .await;
    // An anonymous volume, left behind by a container removed without -v.
    let holder = scratch.run("holder", "-v /cache", "true").await;
    let anonymous = scratch
        .sh(&format!(
            "docker wait {holder} >/dev/null && docker inspect -f '{{{{range .Mounts}}}}{{{{.Name}}}}{{{{end}}}}' {holder} && docker rm {holder} >/dev/null"
        ))
        .await;
    assert_eq!(anonymous.len(), 64, "{anonymous}");
    // A dangling image: a second commit takes the tag from the first.
    let base = scratch.run("base", "", "echo one > /one").await;
    scratch.sh(&format!("docker wait {base}")).await;
    let tag = format!("newport-test/dangle:{}", scratch.tag);
    let old = scratch.sh(&format!("docker commit {base} {tag}")).await;
    scratch
        .sh(&format!(
            "docker commit --change 'ENV X=1' {base} {tag} >/dev/null"
        ))
        .await;

    let disk = containers::disk(
        &crate::remote_tests::owner(&server),
        crate::server_queries::DiskPart::Full,
    )
    .await
    .unwrap();
    let images = disk["images"].as_array().unwrap();
    let busybox = images.iter().find(|i| i["tags"] == json!([IMAGE])).unwrap();
    assert!(busybox["size"].as_u64().is_some_and(|s| s > 0));
    assert!(busybox["containers"].as_u64().is_some_and(|c| c >= 1));
    assert_eq!(busybox["dangling"], false);
    let dangling = images.iter().find(|i| i["id"] == json!(old)).unwrap();
    assert_eq!(dangling["dangling"], true);
    assert_eq!(dangling["tags"], json!([]));
    let volumes = disk["volumes"].as_array().unwrap();
    let keep = volumes.iter().find(|v| v["name"] == json!(named)).unwrap();
    assert_eq!(keep["anonymous"], false);
    assert_eq!(keep["refCount"], 0);
    assert_eq!(keep["driver"], "local");
    let anon = volumes
        .iter()
        .find(|v| v["name"] == json!(anonymous))
        .unwrap();
    assert_eq!(anon["anonymous"], true);
    let totals = &disk["totals"];
    for kind in ["images", "containers", "volumes", "buildCache"] {
        let total = &totals[kind];
        assert!(
            total["reclaimable"].as_u64().unwrap() <= total["size"].as_u64().unwrap(),
            "{kind}: {total}"
        );
    }
    assert!(totals["images"]["count"].as_u64().unwrap() >= 2);
    assert!(totals["containers"]["count"].as_u64().unwrap() >= 1);

    // Stopped containers go; running ones stay.
    let stopped = scratch.run("stopped", "", "true").await;
    scratch.sh(&format!("docker wait {stopped}")).await;
    let alive = scratch.run("alive", "--init", "exec sleep 600").await;
    let pruned = containers::prune(
        &crate::remote_tests::owner(&server),
        PruneTarget::Containers,
    )
    .await
    .unwrap();
    assert!(pruned["removed"].as_u64().unwrap() >= 2, "{pruned}");
    let snapshot = containers::snapshot(&crate::remote_tests::owner(&server))
        .await
        .unwrap();
    assert!(find(&snapshot, &scratch.name("stopped")).is_none());
    assert!(find(&snapshot, &scratch.name("base")).is_none());
    assert_eq!(
        find(&snapshot, &scratch.name("alive")).unwrap()["id"],
        alive
    );

    let images = containers::prune(
        &crate::remote_tests::owner(&server),
        PruneTarget::DanglingImages,
    )
    .await
    .unwrap();
    assert!(images["removed"].as_u64().unwrap() >= 1, "{images}");
    let left = scratch
        .sh(&format!("docker image ls -q --no-trunc {tag}"))
        .await;
    assert!(!left.is_empty(), "the tagged image stays");

    // Unused volumes: the engine's default removes anonymous ones only.
    containers::prune(&crate::remote_tests::owner(&server), PruneTarget::Volumes)
        .await
        .unwrap();
    let volumes = scratch.sh("docker volume ls -q").await;
    assert!(volumes.lines().any(|v| v == named), "{volumes}");
    assert!(!volumes.lines().any(|v| v == anonymous), "{volumes}");

    let cache = containers::prune(
        &crate::remote_tests::owner(&server),
        PruneTarget::BuildCache,
    )
    .await
    .unwrap();
    assert!(cache["removed"].is_u64() && cache["reclaimed"].is_u64());
    scratch
        .sh(&format!("docker image rm {tag} >/dev/null"))
        .await;
    scratch.cleanup().await;
}

#[tokio::test]
#[ignore = "Requires cargo xtask test remote"]
async fn an_account_without_docker_access_is_told_why() {
    let error = containers::snapshot(&crate::remote_tests::owner(&docker("outsider").await))
        .await
        .unwrap_err();
    assert_eq!(
        error,
        "This account can't use Docker: permission denied on /var/run/docker.sock. Add it to the docker group, or use rootless Docker."
    );
}
