//! Docker operations through the monitor service, projected into UI replies.
use crate::{monitor, server_session::ServerSession};
use newport_protocol::monitor::containers::*;
use serde_json::Value;
use std::{sync::Arc, time::Duration};

const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(20);
const READ_TIMEOUT: Duration = Duration::from_secs(20);
const ACTION_TIMEOUT: Duration = Duration::from_secs(75);
const PRUNE_TIMEOUT: Duration = Duration::from_secs(120);

#[cfg(test)]
fn validated(request: &Request, reply: Value) -> Result<Value, String> {
    let reply = match request {
        Request::Snapshot => serde_json::from_value(reply).map(Reply::Snapshot),
        Request::Inspect { .. } => serde_json::from_value(reply).map(Reply::Inspect),
        Request::Logs(_) => serde_json::from_value(reply).map(Reply::Logs),
        Request::Action { .. } => serde_json::from_value(reply).map(Reply::Action),
        Request::ProjectAction { .. } => serde_json::from_value(reply).map(Reply::ProjectAction),
        Request::Remove { .. } => serde_json::from_value(reply).map(Reply::Removed),
        Request::Disk { .. } => serde_json::from_value(reply).map(Reply::Disk),
        Request::Prune { .. } => serde_json::from_value(reply).map(Reply::Pruned),
    }
    .map_err(|e| format!("Invalid containers reply: {e}"))?;
    reply
        .validate_for(request)
        .map_err(|e| format!("Invalid containers reply: {e}"))?;
    serde_json::to_value(reply).map_err(|e| e.to_string())
}
async fn ask_typed(
    server: &Arc<ServerSession>,
    request: Request,
    timeout: Duration,
) -> Result<Reply, String> {
    request.validate().map_err(|error| error.to_string())?;
    let reply = monitor::ask(
        server,
        &newport_protocol::monitor::Request::Containers(request),
        timeout,
    )
    .await?;
    let newport_protocol::monitor::Reply::Containers(reply) = reply else {
        return Err("Invalid containers response operation.".into());
    };
    Ok(reply)
}
async fn ask(
    server: &Arc<ServerSession>,
    request: Request,
    timeout: Duration,
) -> Result<Value, String> {
    serde_json::to_value(ask_typed(server, request, timeout).await?)
        .map_err(|error| error.to_string())
}
/// Typed service boundary shared by the cache and desktop protobuf adapter.
pub(crate) async fn request(
    server: &std::sync::Arc<ServerSession>,
    request: Request,
) -> Result<Reply, String> {
    let timeout = match &request {
        Request::Snapshot => SNAPSHOT_TIMEOUT,
        Request::Inspect { .. } | Request::Logs(_) | Request::Disk { .. } => READ_TIMEOUT,
        Request::Action { .. } | Request::ProjectAction { .. } | Request::Remove { .. } => {
            ACTION_TIMEOUT
        }
        Request::Prune { .. } => PRUNE_TIMEOUT,
    };
    ask_typed(server, request, timeout).await
}

pub async fn snapshot(server: &Arc<ServerSession>) -> Result<Value, String> {
    ask(server, Request::Snapshot, SNAPSHOT_TIMEOUT).await
}
pub async fn inspect(server: &Arc<ServerSession>, target: &str) -> Result<Value, String> {
    ask(
        server,
        Request::Inspect {
            container: target.into(),
        },
        READ_TIMEOUT,
    )
    .await
}
fn logs_request(
    target: Option<&str>,
    project: Option<&str>,
    cursor: Option<&str>,
    lines: Option<u32>,
) -> Result<Request, String> {
    let target = match (target, project) {
        (Some(value), None) => LogTarget::Container(value.into()),
        (None, Some(value)) => LogTarget::Project(value.into()),
        _ => return Err("Choose either a container or a Compose project.".into()),
    };
    let request = Request::Logs(LogsRequest {
        target,
        cursor: cursor.map(str::to_owned),
        lines: lines.map(|n| n.clamp(1, MAX_LOG_ENTRIES as u32)),
    });
    request.validate().map_err(|e| e.to_string())?;
    Ok(request)
}
pub async fn logs(
    server: &Arc<ServerSession>,
    target: Option<&str>,
    project: Option<&str>,
    cursor: Option<&str>,
    lines: Option<u32>,
) -> Result<Value, String> {
    ask(
        server,
        logs_request(target, project, cursor, lines)?,
        READ_TIMEOUT,
    )
    .await
}
pub async fn action(
    server: &Arc<ServerSession>,
    target: &str,
    verb: ContainerVerb,
) -> Result<Value, String> {
    ask(
        server,
        Request::Action {
            container: target.into(),
            verb,
        },
        ACTION_TIMEOUT,
    )
    .await
}
fn project_request(
    project: &str,
    verb: ProjectVerb,
    expected: &[String],
) -> Result<Request, String> {
    let request = Request::ProjectAction {
        project: project.into(),
        verb,
        expected_ids: expected.into(),
    };
    request.validate().map_err(|e| e.to_string())?;
    Ok(request)
}
pub async fn project_action(
    server: &Arc<ServerSession>,
    project: &str,
    verb: ProjectVerb,
    expected: &[String],
) -> Result<Value, String> {
    ask(
        server,
        project_request(project, verb, expected)?,
        ACTION_TIMEOUT,
    )
    .await
}
fn remove_request(targets: &[String]) -> Result<Request, String> {
    let request = Request::Remove {
        containers: targets.into(),
    };
    request.validate().map_err(|e| e.to_string())?;
    Ok(request)
}
pub async fn remove(server: &Arc<ServerSession>, targets: &[String]) -> Result<Value, String> {
    ask(server, remove_request(targets)?, ACTION_TIMEOUT).await
}
pub async fn prune(server: &Arc<ServerSession>, what: PruneTarget) -> Result<Value, String> {
    ask(server, Request::Prune { what }, PRUNE_TIMEOUT).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
        assert!(valid_cursor("c1.1791109265000000002.2"));
        assert!(valid_cursor("p1.aaaaaaaaaaaa-5-1,bbbbbbbbbbbb-7-3"));
        for bad in ["c1 5", "c1;5", "c1\n", &"9".repeat(MAX_CURSOR + 1)] {
            assert!(!valid_cursor(bad));
        }
    }

    #[test]
    fn requests_are_checked_before_they_are_sent() {
        assert_eq!(
            json!(logs_request(Some("web"), None, Some("c1.5.1"), Some(9000)).unwrap()),
            json!({"op": "container_logs", "target": {"container": "web"}, "cursor": "c1.5.1", "lines": 2000})
        );
        assert_eq!(
            json!(logs_request(None, Some("app"), None, Some(0)).unwrap())["lines"],
            1
        );
        assert!(logs_request(Some("web"), Some("app"), None, None).is_err());
        assert!(logs_request(None, None, None, None).is_err());
        assert!(logs_request(None, Some("App"), None, None).is_err());
        assert!(logs_request(Some("a/b"), None, None, None).is_err());

        let ids = vec![ID.to_owned()];
        assert_eq!(
            json!(project_request("app", ProjectVerb::Restart, &ids).unwrap()),
            json!({"op": "project_action", "project": "app", "verb": "restart", "expectedIds": [ID]})
        );
        assert!(project_request("app", ProjectVerb::Stop, &[]).is_err());
        assert!(project_request("app", ProjectVerb::Stop, &["abc".into()]).is_err());
        assert!(project_request("app", ProjectVerb::Stop, &vec![ID.to_owned(); 501]).is_err());
        assert!(project_request("a;b", ProjectVerb::Stop, &ids).is_err());

        assert_eq!(
            json!(remove_request(&["web".into(), ID.into()]).unwrap()),
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
        assert_eq!(
            validated(&Request::Snapshot, snapshot.clone()).unwrap(),
            snapshot
        );
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
            let error = validated(&Request::Snapshot, bad).unwrap_err();
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
        assert_eq!(
            validated(
                &Request::Inspect {
                    container: "web".into()
                },
                inspect.clone()
            )
            .unwrap(),
            inspect
        );
        let mut bad = inspect;
        bad["env"] = json!(["TOKEN=secret"]);
        assert!(
            validated(
                &Request::Inspect {
                    container: "web".into()
                },
                bad
            )
            .is_err()
        );

        let page = json!({"entries": [{"at": 1, "stream": "stderr", "message": "m", "container": "web"},
                                      {"at": null, "stream": "stdout", "message": "", "container": null}],
                          "cursor": "c1.1.1"});
        assert_eq!(
            validated(
                &Request::Logs(LogsRequest {
                    target: LogTarget::Container("web".into()),
                    cursor: None,
                    lines: None
                }),
                page.clone()
            )
            .unwrap(),
            page
        );
        assert!(validated(&Request::Logs(LogsRequest { target: LogTarget::Container("web".into()), cursor: None, lines: None }), json!({"entries": [{"at": 1, "stream": "stdin", "message": "m", "container": null}], "cursor": null})).is_err());

        assert_eq!(
            validated(
                &Request::Action {
                    container: "web".into(),
                    verb: ContainerVerb::Start
                },
                json!({"ok": true})
            )
            .unwrap(),
            json!({"ok": true})
        );
        assert_eq!(
            validated(
                &Request::Remove {
                    containers: vec!["a".into(), "b".into()]
                },
                json!({"removed": 2})
            )
            .unwrap(),
            json!({"removed": 2})
        );
        assert!(
            validated(
                &Request::Remove {
                    containers: vec!["a".into(), "b".into()]
                },
                json!({"removed": -1})
            )
            .is_err()
        );
        assert_eq!(
            validated(
                &Request::Prune {
                    what: PruneTarget::DanglingImages
                },
                json!({"removed": 1, "reclaimed": 4096})
            )
            .unwrap(),
            json!({"removed": 1, "reclaimed": 4096})
        );

        let total = json!({"count": 1, "size": 10, "reclaimable": 0});
        let disk = json!({
            "images": [{"id": "sha256:1", "tags": [], "size": 10, "sharedSize": null, "created": 1000, "containers": 0, "dangling": true}],
            "volumes": [{"name": ID, "driver": "local", "size": null, "refCount": null, "anonymous": true, "compose": null}],
            "totals": {"images": total, "containers": total, "volumes": total, "buildCache": total}, "volumesMeasured": true
        });
        assert_eq!(
            validated(&Request::Disk { part: None }, disk.clone()).unwrap(),
            disk
        );
        let mut bad = disk;
        bad["totals"]["buildCache"] = Value::Null;
        assert!(validated(&Request::Disk { part: None }, bad).is_err());
    }
}
