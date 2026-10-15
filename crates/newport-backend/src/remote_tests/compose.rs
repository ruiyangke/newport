//! Compose actions through the agent against the Docker fixture's own engine
//! (tests/remote/docker) and its real Compose plugin. Each test writes its own
//! project into the fixture user's home, builds only `FROM` the locally
//! imported busybox image and removes everything afterwards. Nothing is pulled
//! from a registry: the one pull names an image under `registry.invalid`,
//! which can never resolve, to check that a failed pull reads cleanly.
use super::containers::{Scratch, docker};
use crate::{
    compose::{self, ComposeAction},
    model::Server,
    monitor,
};
use serde_json::{Value, json};
use tokio::time::{Duration, Instant, sleep};

const IMAGE: &str = "newport-test/busybox:local";

/// The project's compose file: `web` with a named volume, `worker` built
/// locally, `slow` whose build never ends on its own, and `mirror`, which
/// can only be pulled.
fn compose_yaml(tag: &str) -> String {
    format!(
        "services:
  web:
    image: {IMAGE}
    pull_policy: never
    init: true
    command: sleep 600
    volumes:
      - data:/data
  worker:
    build: ./worker
    image: registry.invalid/np{tag}-worker:local
    init: true
    command: sleep 600
  slow:
    build: ./slow
    image: registry.invalid/np{tag}-slow:local
    profiles: [slow]
  mirror:
    image: registry.invalid/np{tag}-worker:local
    profiles: [extra]
volumes:
  data: {{}}
"
    )
}

/// Writes the project and starts `web` with the CLI, so its containers carry
/// the labels Newport finds the files by.
async fn project(scratch: &Scratch) -> String {
    let (tag, project) = (&scratch.tag, scratch.project());
    let yaml = compose_yaml(tag);
    scratch
        .sh(&format!(
            "mkdir -p ~/{project}/worker ~/{project}/slow && cd ~/{project} && \
             cat > compose.yaml <<'EOF'\n{yaml}EOF\n\
             printf 'FROM {IMAGE}\\nRUN echo built {tag} > /built\\n' > worker/Dockerfile && \
             printf 'FROM {IMAGE}\\nRUN echo starting slow step {tag} && sleep 120\\n' > slow/Dockerfile && \
             docker compose -p {project} up -d web 2>&1"
        ))
        .await;
    yaml
}
async fn cleanup(scratch: Scratch) {
    let tag = &scratch.tag;
    scratch
        .sh(&format!(
            "docker image ls -q 'registry.invalid/np{tag}-*' | xargs -r docker image rm -f >/dev/null; \
             docker volume ls -q --filter label=com.docker.compose.project={} | xargs -r docker volume rm >/dev/null; true",
            scratch.project()
        ))
        .await;
    scratch.cleanup().await;
}

/// Full ids of the project's running containers for `service`, sorted.
async fn ids(scratch: &Scratch, service: &str) -> Vec<String> {
    let listed = scratch
        .sh(&format!(
            "docker ps -q --no-trunc --filter label=com.docker.compose.project={} --filter label=com.docker.compose.service={service}",
            scratch.project()
        ))
        .await;
    let mut ids: Vec<String> = listed.lines().map(str::to_owned).collect();
    ids.sort();
    ids
}

async fn start(server: &Server, project: &str, action: ComposeAction) -> String {
    compose::run(&crate::remote_tests::owner(server), project, &action)
        .await
        .unwrap_or_else(|e| panic!("{action:?}: {e}"))["job"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// Polls a job by offset until it ends: offsets only grow, and the pieces
/// put together are exactly the output read from the start.
async fn finished(server: &Server, job: &str) -> (Value, String) {
    let deadline = Instant::now() + Duration::from_secs(180);
    let mut offset = 0;
    let mut output = String::new();
    loop {
        let page = compose::job(&crate::remote_tests::owner(server), job, offset)
            .await
            .unwrap();
        let next = page["offset"].as_u64().unwrap();
        assert!(next >= offset, "{page:#}");
        assert_eq!(next - offset, page["output"].as_str().unwrap().len() as u64);
        output += page["output"].as_str().unwrap();
        offset = next;
        if page["state"] != "running" {
            let whole = compose::job(&crate::remote_tests::owner(server), job, 0)
                .await
                .unwrap();
            assert_eq!(whole["output"].as_str().unwrap(), output);
            assert!(page["finishedAt"].as_i64().unwrap() >= page["startedAt"].as_i64().unwrap());
            return (page, output);
        }
        assert!(Instant::now() < deadline, "job never finished: {output}");
        sleep(Duration::from_millis(300)).await;
    }
}
async fn succeeded(server: &Server, project: &str, action: ComposeAction) -> (Value, String) {
    let job = start(server, project, action).await;
    let (done, output) = finished(server, &job).await;
    assert_eq!(done["state"], "succeeded", "{output}");
    assert_eq!(done["exitCode"], 0);
    (done, output)
}

fn up(services: &[&str], force_recreate: bool) -> ComposeAction {
    ComposeAction::Up {
        services: services.iter().map(|s| (*s).to_owned()).collect(),
        build: false,
        pull: false,
        force_recreate,
    }
}

#[tokio::test]
#[ignore = "Requires npm run test:remote"]
async fn project_view_reports_cli_files_services_and_config_errors() {
    let server = docker("fixture").await;
    let scratch = Scratch::new(&server).await;
    let name = scratch.project();
    assert_eq!(
        compose::project(&crate::remote_tests::owner(&server), &name)
            .await
            .unwrap_err(),
        format!("No containers remain for {name}, so Newport can't find its compose files.")
    );
    let yaml = project(&scratch).await;
    let dir = format!("/home/fixture/{name}");
    let file = format!("{dir}/compose.yaml");

    let view = compose::project(&crate::remote_tests::owner(&server), &name)
        .await
        .unwrap();
    let version = scratch.sh("docker compose version --short").await;
    assert_eq!(
        view["cli"],
        json!({"kind": "plugin", "version": version.trim_start_matches('v')})
    );
    assert_eq!(view["cliError"], Value::Null);
    assert_eq!(view["workingDir"], json!(dir));
    assert_eq!(view["configFiles"], json!([file]));
    assert_eq!(
        view["files"],
        json!([{"path": file, "text": yaml, "error": null}])
    );
    assert_eq!(view["configError"], Value::Null);
    let tag = &scratch.tag;
    // Every profile is listed, by name.
    assert_eq!(
        view["services"],
        json!([
            {"name": "mirror", "image": format!("registry.invalid/np{tag}-worker:local"), "build": false, "profiles": ["extra"]},
            {"name": "slow", "image": format!("registry.invalid/np{tag}-slow:local"), "build": true, "profiles": ["slow"]},
            {"name": "web", "image": IMAGE, "build": false, "profiles": []},
            {"name": "worker", "image": format!("registry.invalid/np{tag}-worker:local"), "build": true, "profiles": []},
        ])
    );

    // A broken file: its text is shown with Compose's own complaint, and a
    // run fails with readable output.
    let broken = "services:\n  web: [\n";
    scratch
        .sh(&format!(
            "printf '{}' > {file}",
            broken.replace('\n', "\\n")
        ))
        .await;
    let view = compose::project(&crate::remote_tests::owner(&server), &name)
        .await
        .unwrap();
    assert_eq!(view["files"][0]["text"], broken);
    assert_eq!(view["services"], Value::Null);
    let error = view["configError"].as_str().unwrap();
    assert!(
        error.contains("yaml") && error.len() <= 4096 && !error.contains('\x1b'),
        "{error}"
    );
    let job = start(&server, &name, up(&[], false)).await;
    let (done, output) = finished(&server, &job).await;
    assert_eq!(done["state"], "failed");
    assert_ne!(done["exitCode"], 0);
    assert!(output.contains("yaml"), "{output}");

    // A missing file: named in the view, and refused before anything runs.
    scratch.sh(&format!("mv {file} {dir}/moved.yaml")).await;
    let view = compose::project(&crate::remote_tests::owner(&server), &name)
        .await
        .unwrap();
    assert_eq!(
        view["files"],
        json!([{"path": file, "text": null, "error": "No such file"}])
    );
    assert!(view["configError"].is_string());
    let build = ComposeAction::Build {
        services: vec![],
        no_cache: false,
    };
    assert_eq!(
        compose::run(&crate::remote_tests::owner(&server), &name, &build)
            .await
            .unwrap_err(),
        format!("Newport can't read {file}: No such file.")
    );

    // An account outside the docker group is told why by the engine.
    let outsider = compose::project(
        &crate::remote_tests::owner(&docker("outsider").await),
        &name,
    )
    .await
    .unwrap_err();
    assert!(
        outsider.starts_with("This account can't use Docker"),
        "{outsider}"
    );
    cleanup(scratch).await;
}

#[tokio::test]
#[ignore = "Requires npm run test:remote"]
async fn up_recreate_scale_build_pull_and_down() {
    let server = docker("fixture").await;
    let scratch = Scratch::new(&server).await;
    let name = scratch.project();
    project(&scratch).await;
    assert_eq!(ids(&scratch, "worker").await.len(), 0);

    // Everything up: `worker` is built from the local image first.
    let (done, output) = succeeded(&server, &name, up(&[], false)).await;
    assert_eq!(done["command"], format!("docker compose -p {name} up -d"));
    assert_eq!(done["project"], json!(name));
    assert!(output.contains("worker"), "{output}");
    assert!(
        !output.contains('\x1b') && !output.contains('\r'),
        "{output:?}"
    );
    let web = ids(&scratch, "web").await;
    let worker = ids(&scratch, "worker").await;
    assert_eq!((web.len(), worker.len()), (1, 1));

    // One service recreated, its neighbour left alone.
    let (done, _) = succeeded(&server, &name, up(&["web"], true)).await;
    assert_eq!(
        done["command"],
        format!("docker compose -p {name} up -d --force-recreate --no-deps web")
    );
    let recreated = ids(&scratch, "web").await;
    assert_eq!(recreated.len(), 1);
    assert_ne!(recreated, web);
    assert_eq!(ids(&scratch, "worker").await, worker);

    let scale = ComposeAction::Scale {
        service: "web".into(),
        replicas: 2,
    };
    let (done, _) = succeeded(&server, &name, scale).await;
    assert_eq!(
        done["command"],
        format!("docker compose -p {name} up -d --no-deps --scale web=2 web")
    );
    assert_eq!(ids(&scratch, "web").await.len(), 2);

    let build = ComposeAction::Build {
        services: vec!["worker".into()],
        no_cache: true,
    };
    let (done, output) = succeeded(&server, &name, build).await;
    assert_eq!(
        done["command"],
        format!("docker compose -p {name} build --no-cache worker")
    );
    assert!(output.contains("RUN echo built"), "{output}");

    // A pull that cannot succeed fails cleanly, in the CLI's words.
    let pull = ComposeAction::Pull {
        services: vec!["mirror".into()],
    };
    let job = start(&server, &name, pull).await;
    let (done, output) = finished(&server, &job).await;
    assert_eq!(done["state"], "failed", "{output}");
    assert_eq!(
        done["command"],
        format!("docker compose -p {name} pull mirror")
    );
    assert!(done["exitCode"].as_i64().is_some_and(|c| c != 0));
    assert!(output.contains("registry.invalid"), "{output}");

    // Down keeps the named volume...
    let down = |volumes| ComposeAction::Down {
        volumes,
        remove_orphans: volumes,
    };
    succeeded(&server, &name, down(false)).await;
    let left = scratch
        .sh(&format!(
            "docker ps -aq --filter label=com.docker.compose.project={name}"
        ))
        .await;
    assert_eq!(left, "");
    let volume = format!("{name}_data");
    let volumes = format!("docker volume ls -q --filter name=^{volume}$");
    assert_eq!(scratch.sh(&volumes).await, volume);
    // ...and with no containers left, files can't be found, but `down`
    // still works from the project name and takes the volume with it.
    assert_eq!(
        compose::project(&crate::remote_tests::owner(&server), &name)
            .await
            .unwrap_err(),
        format!("No containers remain for {name}, so Newport can't find its compose files.")
    );
    assert_eq!(
        compose::run(&crate::remote_tests::owner(&server), &name, &up(&[], false))
            .await
            .unwrap_err(),
        format!("No containers remain for {name}, so Newport can't find its compose files.")
    );
    let (done, _) = succeeded(&server, &name, down(true)).await;
    assert_eq!(
        done["command"],
        format!("docker compose -p {name} down --volumes --remove-orphans")
    );
    assert_eq!(scratch.sh(&volumes).await, "");
    cleanup(scratch).await;
}

#[tokio::test]
#[ignore = "Requires npm run test:remote"]
async fn one_job_per_project_streaming_and_cancel() {
    let server = docker("fixture").await;
    let scratch = Scratch::new(&server).await;
    let name = scratch.project();
    project(&scratch).await;

    let slow = ComposeAction::Build {
        services: vec!["slow".into()],
        no_cache: false,
    };
    let job = start(&server, &name, slow).await;
    assert_eq!(
        compose::run(&crate::remote_tests::owner(&server), &name, &up(&[], false))
            .await
            .unwrap_err(),
        format!("Another Compose command is running for {name}.")
    );

    // Output streams by offset while the build sleeps.
    let deadline = Instant::now() + Duration::from_secs(60);
    let (mut offset, mut output, mut pages) = (0, String::new(), 0);
    let marker = format!("starting slow step {}", scratch.tag);
    while !output.lines().any(|l| l.ends_with(&marker)) {
        assert!(Instant::now() < deadline, "no build output: {output}");
        let page = compose::job(&crate::remote_tests::owner(&server), &job, offset)
            .await
            .unwrap();
        assert_eq!(page["state"], "running", "{output}");
        let next = page["offset"].as_u64().unwrap();
        assert_eq!(next - offset, page["output"].as_str().unwrap().len() as u64);
        if next > offset {
            pages += 1;
        }
        output += page["output"].as_str().unwrap();
        offset = next;
        sleep(Duration::from_millis(200)).await;
    }
    assert!(pages >= 1);
    let again = compose::job(&crate::remote_tests::owner(&server), &job, offset)
        .await
        .unwrap();
    assert!(again["offset"].as_u64().unwrap() >= offset);
    let whole = compose::job(&crate::remote_tests::owner(&server), &job, 0)
        .await
        .unwrap();
    assert!(whole["output"].as_str().unwrap().starts_with(&output));

    // A reopened page finds it running.
    let listed = compose::jobs(&crate::remote_tests::owner(&server))
        .await
        .unwrap();
    let first = &listed["jobs"][0];
    assert_eq!(first["job"], json!(job));
    assert_eq!(first["state"], "running");
    assert_eq!(first["project"], json!(name));
    assert_eq!(
        first["command"],
        format!("docker compose -p {name} build slow")
    );
    assert_eq!(
        (first["output"].as_str(), first["offset"].as_u64()),
        (Some(""), Some(0))
    );

    let asked = Instant::now();
    assert_eq!(
        compose::cancel(&crate::remote_tests::owner(&server), &job)
            .await
            .unwrap(),
        json!({"ok": true})
    );
    let (done, _) = finished(&server, &job).await;
    assert_eq!(done["state"], "cancelled");
    assert!(asked.elapsed() < Duration::from_secs(20));
    // Cancelling a finished job is a no-op, and the project is free again.
    assert_eq!(
        compose::cancel(&crate::remote_tests::owner(&server), &job)
            .await
            .unwrap(),
        json!({"ok": true})
    );
    succeeded(&server, &name, up(&["web"], false)).await;

    assert_eq!(
        compose::job(
            &crate::remote_tests::owner(&server),
            "0123456789abcdef0123456789abcdef",
            0
        )
        .await
        .unwrap_err(),
        "That Compose command is no longer known; the agent may have restarted."
    );
    // The agent checks requests too, not only the desktop.
    let raw = json!({"op": "compose_run", "project": name,
                     "action": {"kind": "scale", "service": "web", "replicas": 21}});
    assert_eq!(
        monitor::ask(
            &crate::remote_tests::owner(&server),
            &raw,
            Duration::from_secs(10)
        )
        .await
        .unwrap_err(),
        "Replicas must be 0 to 20."
    );
    let raw = json!({"op": "compose_run", "project": name,
                     "action": {"kind": "up", "services": ["--volumes"]}});
    assert_eq!(
        monitor::ask(
            &crate::remote_tests::owner(&server),
            &raw,
            Duration::from_secs(10)
        )
        .await
        .unwrap_err(),
        "Not a valid Compose service: --volumes"
    );
    cleanup(scratch).await;
}
