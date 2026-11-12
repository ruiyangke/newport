//! Compose operations through the monitor service, projected into UI replies.
use crate::{monitor, server_session::ServerSession};
use newport_protocol::monitor::compose::Reply;
use newport_protocol::monitor::compose::{Action, Request};
use serde_json::Value;
use std::{sync::Arc, time::Duration};

const PROJECT_TIMEOUT: Duration = Duration::from_secs(30);
const RUN_TIMEOUT: Duration = Duration::from_secs(10);
const JOB_TIMEOUT: Duration = Duration::from_secs(10);

#[cfg(test)]
fn validated(request: &Request, reply: Value) -> Result<Value, String> {
    let reply = match request {
        Request::Project { .. } => serde_json::from_value(reply).map(Reply::Project),
        Request::Run { .. } => serde_json::from_value(reply).map(Reply::Started),
        Request::Job { .. } => serde_json::from_value(reply).map(Reply::Job),
        Request::Cancel { .. } => serde_json::from_value(reply).map(Reply::Cancelled),
        Request::Jobs => serde_json::from_value(reply).map(Reply::Jobs),
    }
    .map_err(|e| format!("Invalid compose reply: {e}"))?;
    reply
        .validate_for(request)
        .map_err(|e| format!("Invalid compose reply: {e}"))?;
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
        &newport_protocol::monitor::Request::Compose(request),
        timeout,
    )
    .await?;
    let newport_protocol::monitor::Reply::Compose(reply) = reply else {
        return Err("Invalid compose response operation.".into());
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
        Request::Project { .. } => PROJECT_TIMEOUT,
        Request::Run { .. } | Request::Cancel { .. } => RUN_TIMEOUT,
        Request::Job { .. } | Request::Jobs => JOB_TIMEOUT,
    };
    ask_typed(server, request, timeout).await
}

fn checked(request: Request) -> Result<Request, String> {
    request.validate().map_err(|e| e.to_string())?;
    Ok(request)
}
fn project_request(name: &str) -> Result<Request, String> {
    checked(Request::Project {
        project: name.into(),
    })
}
fn run_request(name: &str, action: &Action) -> Result<Request, String> {
    checked(Request::Run {
        project: name.into(),
        action: action.clone(),
    })
}
fn job_request(id: &str, offset: u64) -> Result<Request, String> {
    checked(Request::Job {
        job: id.into(),
        offset,
    })
}
fn cancel_request(id: &str) -> Result<Request, String> {
    checked(Request::Cancel { job: id.into() })
}
pub async fn project(server: &Arc<ServerSession>, name: &str) -> Result<Value, String> {
    ask(server, project_request(name)?, PROJECT_TIMEOUT).await
}
pub async fn run(
    server: &Arc<ServerSession>,
    name: &str,
    action: &Action,
) -> Result<Value, String> {
    ask(server, run_request(name, action)?, RUN_TIMEOUT).await
}
pub async fn job(server: &Arc<ServerSession>, id: &str, offset: u64) -> Result<Value, String> {
    ask(server, job_request(id, offset)?, JOB_TIMEOUT).await
}
pub async fn cancel(server: &Arc<ServerSession>, id: &str) -> Result<Value, String> {
    ask(server, cancel_request(id)?, RUN_TIMEOUT).await
}
pub async fn jobs(server: &Arc<ServerSession>) -> Result<Value, String> {
    ask(server, Request::Jobs, JOB_TIMEOUT).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn action(value: Value) -> Result<Action, String> {
        serde_json::from_value(value).map_err(|e| e.to_string())
    }

    #[test]
    fn arguments_are_checked_before_they_are_sent() {
        assert_eq!(
            json!(project_request("app").unwrap()),
            json!({"op": "compose_project", "project": "app"})
        );
        for bad in ["", "App", "a b", "-p", "a;id", "a.b"] {
            assert!(project_request(bad).is_err(), "{bad:?}");
        }

        let up = action(json!({"kind": "up", "services": ["web"], "build": true, "pull": false, "forceRecreate": true})).unwrap();
        assert_eq!(
            json!(run_request("app", &up).unwrap()),
            json!({"op": "compose_run", "project": "app", "action": {
                "kind": "up", "services": ["web"], "build": true, "pull": false, "forceRecreate": true}})
        );
        // Flags the UI leaves out are off.
        assert_eq!(
            action(json!({"kind": "down"})).unwrap(),
            Action::Down {
                volumes: false,
                remove_orphans: false
            }
        );
        assert_eq!(
            json!(action(json!({"kind": "build", "noCache": true})).unwrap()),
            json!({"kind": "build", "services": [], "noCache": true})
        );
        assert_eq!(
            json!(action(json!({"kind": "scale", "service": "web", "replicas": 2})).unwrap()),
            json!({"kind": "scale", "service": "web", "replicas": 2})
        );
        assert!(run_request("App", &up).is_err());
        for bad in [
            "",
            "-d",
            "--build",
            "a b",
            "a/b",
            "$(id)",
            "é",
            &"a".repeat(65),
        ] {
            let pull = Action::Pull {
                services: vec![bad.to_owned()],
            };
            assert_eq!(
                run_request("app", &pull).unwrap_err(),
                format!("Not a valid Compose service: {bad}")
            );
        }
        let many = Action::Build {
            services: (0..51).map(|i| format!("s{i}")).collect(),
            no_cache: false,
        };
        assert_eq!(
            run_request("app", &many).unwrap_err(),
            "Choose at most 50 services."
        );
        let scale = |service: &str, replicas: u32| Action::Scale {
            service: service.into(),
            replicas,
        };
        assert!(run_request("app", &scale("web", 0)).is_ok());
        assert!(run_request("app", &scale("web", 20)).is_ok());
        assert_eq!(
            run_request("app", &scale("web", 21)).unwrap_err(),
            "Replicas must be 0 to 20."
        );
        assert!(run_request("app", &scale("-x", 1)).is_err());
        for bad in [
            json!({"kind": "restart"}),
            json!({"kind": "scale", "service": "web", "replicas": -1}),
            json!({"kind": "scale", "service": "web", "replicas": 1.5}),
            json!({"kind": "scale", "service": "web"}),
            json!({"kind": "up", "build": "yes"}),
            json!({"services": []}),
        ] {
            assert!(action(bad.clone()).is_err(), "{bad}");
        }

        let id = "0123456789abcdef0123456789abcdef";
        assert_eq!(
            json!(job_request(id, 42).unwrap()),
            json!({"op": "compose_job", "job": id, "offset": 42})
        );
        assert_eq!(
            json!(cancel_request(id).unwrap()),
            json!({"op": "compose_cancel", "job": id})
        );
        for bad in ["", "a b", "../x", "a;b", &"a".repeat(65)] {
            assert!(job_request(bad, 0).is_err(), "{bad:?}");
            assert!(cancel_request(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn replies_are_held_to_the_contract() {
        let view = json!({
            "cli": {"kind": "plugin", "version": "2.29.1"},
            "cliError": null,
            "workingDir": "/srv/app",
            "configFiles": ["/srv/app/compose.yaml"],
            "files": [{"path": "/srv/app/compose.yaml", "text": "services: {}\n", "error": null},
                      {"path": "/srv/app/gone.yaml", "text": null, "error": "No such file"}],
            "services": [{"name": "web", "image": "nginx:1.27", "build": false, "profiles": []},
                         {"name": "tool", "image": null, "build": true, "profiles": ["debug"]}],
            "configError": null
        });
        assert_eq!(
            validated(
                &Request::Project {
                    project: "app".into()
                },
                view.clone()
            )
            .unwrap(),
            view
        );
        let missing = json!({
            "cli": null, "cliError": "The Compose CLI isn't installed on this server.",
            "workingDir": null, "configFiles": [], "files": [], "services": null, "configError": null
        });
        assert_eq!(
            validated(
                &Request::Project {
                    project: "app".into()
                },
                missing.clone()
            )
            .unwrap(),
            missing
        );
        for (field, value) in [
            ("cli", json!({"kind": "nerdctl", "version": "1"})),
            ("configFiles", json!("/srv/app/compose.yaml")),
            ("files", json!([{"path": "/x", "text": 5, "error": null}])),
            (
                "services",
                json!([{"name": "web", "image": null, "build": "yes", "profiles": []}]),
            ),
        ] {
            let mut bad = view.clone();
            bad[field] = value;
            let error = validated(
                &Request::Project {
                    project: "app".into(),
                },
                bad,
            )
            .unwrap_err();
            assert!(
                error.starts_with("Invalid compose reply: "),
                "{field}: {error}"
            );
        }

        assert_eq!(
            validated(
                &Request::Run {
                    project: "app".into(),
                    action: Action::Down {
                        volumes: false,
                        remove_orphans: false
                    }
                },
                json!({"job": "abc"})
            )
            .unwrap(),
            json!({"job": "abc"})
        );
        assert!(
            validated(
                &Request::Run {
                    project: "app".into(),
                    action: Action::Down {
                        volumes: false,
                        remove_orphans: false
                    }
                },
                json!({"ok": true})
            )
            .is_err()
        );

        let job = json!({
            "job": "abc", "project": "app", "command": "docker compose -p app up -d",
            "state": "running", "exitCode": null, "startedAt": 1791109265000i64,
            "finishedAt": null, "output": "line\n", "offset": 5
        });
        assert_eq!(
            validated(
                &Request::Job {
                    job: "abc".into(),
                    offset: 0
                },
                job.clone()
            )
            .unwrap(),
            job
        );
        for state in ["succeeded", "failed", "cancelled"] {
            let mut done = job.clone();
            done["state"] = json!(state);
            done["finishedAt"] = json!(1791109265001i64);
            done["exitCode"] = if state == "succeeded" {
                json!(0)
            } else {
                Value::Null
            };
            assert!(
                validated(
                    &Request::Job {
                        job: "abc".into(),
                        offset: 0
                    },
                    done
                )
                .is_ok()
            );
        }
        for (field, value) in [
            ("state", json!("stopped")),
            ("offset", json!(-1)),
            ("output", Value::Null),
            ("startedAt", json!("now")),
        ] {
            let mut bad = job.clone();
            bad[field] = value;
            assert!(
                validated(
                    &Request::Job {
                        job: "abc".into(),
                        offset: 0
                    },
                    bad
                )
                .is_err(),
                "{field}"
            );
        }
        let mut listed_job = job;
        listed_job["output"] = json!("");
        listed_job["offset"] = json!(0);
        let listed = json!({"jobs": [listed_job]});
        assert_eq!(validated(&Request::Jobs, listed.clone()).unwrap(), listed);
        assert!(validated(&Request::Jobs, json!({"jobs": null})).is_err());
        assert_eq!(
            validated(&Request::Cancel { job: "abc".into() }, json!({"ok": true})).unwrap(),
            json!({"ok": true})
        );
    }
}
