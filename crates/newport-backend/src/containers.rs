//! The Containers tab: Docker (or Podman) through the server's agent channel,
//! which talks to the engine's API as the SSH user. Arguments are validated
//! here and again by the agent; replies are checked against this contract
//! before they reach the UI.
use crate::{monitor, server_session::ServerSession};
pub use newport_domain::containers::{
    ContainerInspect as Inspect, ContainerLogPage as LogPage, ContainerVerb,
    ContainersSnapshot as Snapshot, DockerDisk as Disk, ProjectVerb, PruneTarget, Pruned,
    valid_container, valid_project,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Duration;

const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(20);
const READ_TIMEOUT: Duration = Duration::from_secs(20);
// A stop waits up to ten seconds per container before the engine kills it.
const ACTION_TIMEOUT: Duration = Duration::from_secs(75);
const PRUNE_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_IDS: usize = 500;
const MAX_CURSOR: usize = 4096;
const MAX_LINES: u32 = 2000;

fn valid_id(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit())
}
fn container(target: &str) -> Result<(), String> {
    if valid_container(target) {
        Ok(())
    } else {
        Err(format!("Not a valid container: {target}"))
    }
}
fn project(name: &str) -> Result<(), String> {
    if valid_project(name) {
        Ok(())
    } else {
        Err(format!("Not a valid Compose project: {name}"))
    }
}
fn cursor(text: &str) -> Result<(), String> {
    if text.len() <= MAX_CURSOR
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".,_-".contains(&b))
    {
        Ok(())
    } else {
        Err("Invalid log cursor.".into())
    }
}
fn listed(items: &[String], what: &str) -> Result<(), String> {
    if items.is_empty() || items.len() > MAX_IDS {
        Err(format!("Choose 1 to {MAX_IDS} {what}."))
    } else {
        Ok(())
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct Done {
    ok: bool,
}
#[derive(Debug, Deserialize, Serialize)]
struct Removed {
    removed: u64,
}

fn validated<T: serde::de::DeserializeOwned + Serialize>(reply: Value) -> Result<Value, String> {
    let typed: T =
        serde_json::from_value(reply).map_err(|e| format!("Invalid containers reply: {e}"))?;
    serde_json::to_value(typed).map_err(|e| e.to_string())
}

pub async fn snapshot(server: &std::sync::Arc<ServerSession>) -> Result<Value, String> {
    let reply = monitor::ask(server, &json!({"op": "containers"}), SNAPSHOT_TIMEOUT).await?;
    validated::<Snapshot>(reply)
}

pub async fn inspect(
    server: &std::sync::Arc<ServerSession>,
    target: &str,
) -> Result<Value, String> {
    container(target)?;
    let request = json!({"op": "container_inspect", "container": target});
    validated::<Inspect>(monitor::ask(server, &request, READ_TIMEOUT).await?)
}

fn logs_request(
    target: Option<&str>,
    compose: Option<&str>,
    from: Option<&str>,
    lines: Option<u32>,
) -> Result<Value, String> {
    match (target, compose) {
        (Some(target), None) => container(target)?,
        (None, Some(name)) => project(name)?,
        _ => return Err("Choose either a container or a Compose project.".into()),
    }
    if let Some(from) = from {
        cursor(from)?;
    }
    Ok(json!({
        "op": "container_logs", "container": target, "project": compose,
        "cursor": from, "lines": lines.map(|n| n.clamp(1, MAX_LINES)),
    }))
}

pub async fn logs(
    server: &std::sync::Arc<ServerSession>,
    target: Option<&str>,
    compose: Option<&str>,
    from: Option<&str>,
    lines: Option<u32>,
) -> Result<Value, String> {
    let request = logs_request(target, compose, from, lines)?;
    validated::<LogPage>(monitor::ask(server, &request, READ_TIMEOUT).await?)
}

pub async fn action(
    server: &std::sync::Arc<ServerSession>,
    target: &str,
    verb: ContainerVerb,
) -> Result<Value, String> {
    container(target)?;
    let request = json!({"op": "container_action", "container": target, "verb": verb});
    validated::<Done>(monitor::ask(server, &request, ACTION_TIMEOUT).await?)
}

fn project_request(name: &str, verb: ProjectVerb, expected: &[String]) -> Result<Value, String> {
    project(name)?;
    listed(expected, "containers")?;
    if let Some(bad) = expected.iter().find(|id| !valid_id(id)) {
        return Err(format!("Not a valid container id: {bad}"));
    }
    Ok(json!({"op": "project_action", "project": name, "verb": verb, "expectedIds": expected}))
}

pub async fn project_action(
    server: &std::sync::Arc<ServerSession>,
    name: &str,
    verb: ProjectVerb,
    expected: &[String],
) -> Result<Value, String> {
    let request = project_request(name, verb, expected)?;
    validated::<Done>(monitor::ask(server, &request, ACTION_TIMEOUT).await?)
}

fn remove_request(targets: &[String]) -> Result<Value, String> {
    listed(targets, "containers")?;
    for target in targets {
        container(target)?;
    }
    Ok(json!({"op": "containers_remove", "containers": targets}))
}

pub async fn remove(
    server: &std::sync::Arc<ServerSession>,
    targets: &[String],
) -> Result<Value, String> {
    let request = remove_request(targets)?;
    validated::<Removed>(monitor::ask(server, &request, ACTION_TIMEOUT).await?)
}

pub(crate) async fn disk(
    server: &std::sync::Arc<ServerSession>,
    part: crate::server_queries::DiskPart,
) -> Result<Value, String> {
    let mut request = json!({"op": "docker_disk"});
    if let Some(part) = part.name() {
        request["part"] = json!(part);
    }
    validated::<Disk>(monitor::ask(server, &request, READ_TIMEOUT).await?)
}

pub async fn prune(
    server: &std::sync::Arc<ServerSession>,
    what: PruneTarget,
) -> Result<Value, String> {
    let request = json!({"op": "docker_prune", "what": what});
    validated::<Pruned>(monitor::ask(server, &request, PRUNE_TIMEOUT).await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn targets_cannot_smuggle_paths_or_arguments() {
        for good in [ID, &ID[..12], "web", "app-web-1", "my_app.web"] {
            assert!(valid_container(good), "{good}");
        }
        for bad in [
            "", "-x", "../x", "a/b", "a b", "a?b", "a#b", "a%2f", "$(id)", "a\nb",
        ] {
            assert!(!valid_container(bad), "{bad:?}");
        }
        assert!(!valid_container(&"a".repeat(129)));
        for good in ["app", "my-app_1"] {
            assert!(valid_project(good), "{good}");
        }
        for bad in ["", "-app", "App", "a b", "a;id", "a.b", "a\nb"] {
            assert!(!valid_project(bad), "{bad:?}");
        }
        assert!(cursor("c1.1791109265000000002.2").is_ok());
        assert!(cursor("p1.aaaaaaaaaaaa-5-1,bbbbbbbbbbbb-7-3").is_ok());
        for bad in ["c1 5", "c1;5", "c1\n", &"9".repeat(MAX_CURSOR + 1)] {
            assert_eq!(cursor(bad).unwrap_err(), "Invalid log cursor.");
        }
    }

    #[test]
    fn requests_are_checked_before_they_are_sent() {
        assert_eq!(
            logs_request(Some("web"), None, Some("c1.5.1"), Some(9000)).unwrap(),
            json!({"op": "container_logs", "container": "web", "project": null, "cursor": "c1.5.1", "lines": 2000})
        );
        assert_eq!(
            logs_request(None, Some("app"), None, Some(0)).unwrap()["lines"],
            1
        );
        assert!(logs_request(Some("web"), Some("app"), None, None).is_err());
        assert!(logs_request(None, None, None, None).is_err());
        assert!(logs_request(None, Some("App"), None, None).is_err());
        assert!(logs_request(Some("a/b"), None, None, None).is_err());

        let ids = vec![ID.to_owned()];
        assert_eq!(
            project_request("app", ProjectVerb::Restart, &ids).unwrap(),
            json!({"op": "project_action", "project": "app", "verb": "restart", "expectedIds": [ID]})
        );
        assert!(project_request("app", ProjectVerb::Stop, &[]).is_err());
        assert!(project_request("app", ProjectVerb::Stop, &["abc".into()]).is_err());
        assert!(project_request("app", ProjectVerb::Stop, &vec![ID.to_owned(); 501]).is_err());
        assert!(project_request("a;b", ProjectVerb::Stop, &ids).is_err());

        assert_eq!(
            remove_request(&["web".into(), ID.into()]).unwrap(),
            json!({"op": "containers_remove", "containers": ["web", ID]})
        );
        assert!(remove_request(&[]).is_err());
        assert!(remove_request(&["web".into(), "-f".into()]).is_err());

        assert_eq!(json!(PruneTarget::DanglingImages), json!("dangling-images"));
        assert_eq!(json!(PruneTarget::BuildCache), json!("build-cache"));
        assert!(serde_json::from_value::<PruneTarget>(json!("everything")).is_err());
        assert!(serde_json::from_value::<ContainerVerb>(json!("kill")).is_err());
        assert!(serde_json::from_value::<ProjectVerb>(json!("pause")).is_err());
    }

    fn container_row() -> Value {
        json!({
            "id": ID, "name": "web", "image": "nginx:1.27", "imageId": "sha256:1", "command": "nginx",
            "state": "running", "status": "Up 2 hours (healthy)", "health": "healthy", "exitCode": null,
            "oomKilled": false, "error": null, "restartCount": 0, "restartPolicy": "always",
            "created": 1791109265000i64, "startedAt": 1791109265000i64, "finishedAt": null,
            "ports": [{"ip": "0.0.0.0", "hostPort": 8080, "containerPort": 80, "protocol": "tcp"},
                      {"ip": null, "hostPort": null, "containerPort": 9000, "protocol": "udp"}],
            "compose": {"project": "app", "service": "web", "oneoff": false, "number": 1,
                        "workingDir": "/srv/app", "configFiles": null},
            "cpu": 1.5, "memory": 9000000, "memoryLimit": null
        })
    }

    #[test]
    fn replies_are_held_to_the_contract() {
        let snapshot = json!({
            "engine": {"version": "29.4.0", "apiVersion": "1.47", "os": "Debian GNU/Linux 13 (trixie)", "rootless": false, "podman": false},
            "containers": [container_row()],
            "truncated": false
        });
        assert_eq!(validated::<Snapshot>(snapshot.clone()).unwrap(), snapshot);
        for (field, value) in [
            ("state", json!("zombie")),
            ("health", json!("none")),
            ("created", json!("yesterday")),
            (
                "ports",
                json!([{"ip": null, "hostPort": null, "containerPort": 80, "protocol": "icmp"}]),
            ),
            ("compose", json!({"project": "app"})),
        ] {
            let mut bad = snapshot.clone();
            bad["containers"][0][field] = value;
            let error = validated::<Snapshot>(bad).unwrap_err();
            assert!(
                error.starts_with("Invalid containers reply: "),
                "{field}: {error}"
            );
        }

        let inspect = json!({
            "id": ID, "name": "web", "image": "nginx:1.27", "created": 1791109265000i64,
            "entrypoint": null, "cmd": ["nginx"], "workingDir": null, "user": null, "hostname": "abc",
            "env": [{"name": "TOKEN", "value": "secret"}],
            "mounts": [{"type": "volume", "source": null, "destination": "/data", "mode": null, "rw": true, "name": "data"}],
            "networks": [{"name": "app_default", "ipAddress": "172.18.0.2", "aliases": ["web"]}],
            "restartPolicy": "no", "labels": {"a": "1"},
            "healthcheck": {"test": ["CMD-SHELL", "true"], "interval": 2000},
            "lastHealth": [{"at": 1791109265000i64, "exitCode": 0, "output": "ok"}]
        });
        assert_eq!(validated::<Inspect>(inspect.clone()).unwrap(), inspect);
        let mut bad = inspect;
        bad["env"] = json!(["TOKEN=secret"]);
        assert!(validated::<Inspect>(bad).is_err());

        let page = json!({"entries": [{"at": 1, "stream": "stderr", "message": "m", "container": "web"},
                                      {"at": null, "stream": "stdout", "message": "", "container": null}],
                          "cursor": "c1.1.1"});
        assert_eq!(validated::<LogPage>(page.clone()).unwrap(), page);
        assert!(validated::<LogPage>(json!({"entries": [{"at": 1, "stream": "stdin", "message": "m", "container": null}], "cursor": null})).is_err());

        assert_eq!(
            validated::<Done>(json!({"ok": true})).unwrap(),
            json!({"ok": true})
        );
        assert_eq!(
            validated::<Removed>(json!({"removed": 2})).unwrap(),
            json!({"removed": 2})
        );
        assert!(validated::<Removed>(json!({"removed": -1})).is_err());
        assert_eq!(
            validated::<Pruned>(json!({"removed": 1, "reclaimed": 4096})).unwrap(),
            json!({"removed": 1, "reclaimed": 4096})
        );

        let total = json!({"count": 1, "size": 10, "reclaimable": 0});
        let disk = json!({
            "images": [{"id": "sha256:1", "tags": [], "size": 10, "sharedSize": null, "created": 1000, "containers": 0, "dangling": true}],
            "volumes": [{"name": ID, "driver": "local", "size": null, "refCount": null, "anonymous": true, "compose": null}],
            "totals": {"images": total, "containers": total, "volumes": total, "buildCache": total}
        });
        // Replies from agents before the quick/volumes split count as measured.
        let mut normalized = disk.clone();
        normalized["volumesMeasured"] = json!(true);
        assert_eq!(validated::<Disk>(disk.clone()).unwrap(), normalized);
        let mut bad = disk;
        bad["totals"]["buildCache"] = Value::Null;
        assert!(validated::<Disk>(bad).is_err());
    }
}
