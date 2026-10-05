//! Compose actions for the Containers tab: the server's agent runs the
//! Compose CLI as a background job (see tools/agent/src/compose.rs) and this
//! side polls it. Arguments are validated here and again by the agent;
//! replies are checked against the contract before they reach the UI.
use crate::{containers::valid_project, model::Server, monitor};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;

// `config` can be slow on a large project.
const PROJECT_TIMEOUT: Duration = Duration::from_secs(30);
const RUN_TIMEOUT: Duration = Duration::from_secs(10);
const JOB_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_SERVICES: usize = 50;
const MAX_SERVICE_NAME: usize = 64;
const MAX_REPLICAS: u32 = 20;
const MAX_JOB_ID: usize = 64;

/// Compose's service-name rules, kept to what is safe as an argument.
pub fn valid_service(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_SERVICE_NAME
        && name.as_bytes()[0].is_ascii_alphanumeric()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}
fn valid_job(job: &str) -> bool {
    !job.is_empty()
        && job.len() <= MAX_JOB_ID
        && job.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn check_project(name: &str) -> Result<(), String> {
    if valid_project(name) {
        Ok(())
    } else {
        Err(format!("Not a valid Compose project: {name}"))
    }
}
fn check_job(id: &str) -> Result<(), String> {
    if valid_job(id) {
        Ok(())
    } else {
        Err(format!("Not a valid Compose command: {id}"))
    }
}
fn check_services(names: &[String]) -> Result<(), String> {
    if names.len() > MAX_SERVICES {
        return Err(format!("Choose at most {MAX_SERVICES} services."));
    }
    match names.iter().find(|n| !valid_service(n)) {
        Some(bad) => Err(format!("Not a valid Compose service: {bad}")),
        None => Ok(()),
    }
}

/// What to run, as the UI names it.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(
    tag = "kind",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum ComposeAction {
    Up {
        #[serde(default)]
        services: Vec<String>,
        #[serde(default)]
        build: bool,
        #[serde(default)]
        pull: bool,
        #[serde(default)]
        force_recreate: bool,
    },
    Down {
        #[serde(default)]
        volumes: bool,
        #[serde(default)]
        remove_orphans: bool,
    },
    Pull {
        #[serde(default)]
        services: Vec<String>,
    },
    Build {
        #[serde(default)]
        services: Vec<String>,
        #[serde(default)]
        no_cache: bool,
    },
    Scale {
        service: String,
        replicas: u32,
    },
}
impl ComposeAction {
    fn check(&self) -> Result<(), String> {
        match self {
            Self::Up {
                services: names, ..
            }
            | Self::Pull { services: names }
            | Self::Build {
                services: names, ..
            } => check_services(names),
            Self::Down { .. } => Ok(()),
            Self::Scale { service, replicas } => {
                if !valid_service(service) {
                    Err(format!("Not a valid Compose service: {service}"))
                } else if *replicas > MAX_REPLICAS {
                    Err(format!("Replicas must be 0 to {MAX_REPLICAS}."))
                } else {
                    Ok(())
                }
            }
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum CliKind {
    Plugin,
    Standalone,
    Podman,
}
#[derive(Debug, Deserialize, Serialize)]
struct Cli {
    kind: CliKind,
    version: String,
}
#[derive(Debug, Deserialize, Serialize)]
struct File {
    path: String,
    text: Option<String>,
    error: Option<String>,
}
#[derive(Debug, Deserialize, Serialize)]
struct Service {
    name: String,
    image: Option<String>,
    build: bool,
    profiles: Vec<String>,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Project {
    cli: Option<Cli>,
    cli_error: Option<String>,
    working_dir: Option<String>,
    config_files: Vec<String>,
    files: Vec<File>,
    services: Option<Vec<Service>>,
    config_error: Option<String>,
}
#[derive(Debug, Deserialize, Serialize)]
struct Started {
    job: String,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum State {
    Running,
    Succeeded,
    Failed,
    Cancelled,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Job {
    job: String,
    project: String,
    command: String,
    state: State,
    exit_code: Option<i64>,
    started_at: i64,
    finished_at: Option<i64>,
    output: String,
    offset: u64,
}
#[derive(Debug, Deserialize, Serialize)]
struct Jobs {
    jobs: Vec<Job>,
}
#[derive(Debug, Deserialize, Serialize)]
struct Done {
    ok: bool,
}

fn validated<T: serde::de::DeserializeOwned + Serialize>(reply: Value) -> Result<Value, String> {
    let typed: T =
        serde_json::from_value(reply).map_err(|e| format!("Invalid compose reply: {e}"))?;
    serde_json::to_value(typed).map_err(|e| e.to_string())
}

fn project_request(name: &str) -> Result<Value, String> {
    check_project(name)?;
    Ok(json!({"op": "compose_project", "project": name}))
}
fn run_request(name: &str, action: &ComposeAction) -> Result<Value, String> {
    check_project(name)?;
    action.check()?;
    Ok(json!({"op": "compose_run", "project": name, "action": action}))
}
fn job_request(id: &str, offset: u64) -> Result<Value, String> {
    check_job(id)?;
    Ok(json!({"op": "compose_job", "job": id, "offset": offset}))
}
fn cancel_request(id: &str) -> Result<Value, String> {
    check_job(id)?;
    Ok(json!({"op": "compose_cancel", "job": id}))
}

pub async fn project(server: &Server, name: &str) -> Result<Value, String> {
    let request = project_request(name)?;
    validated::<Project>(monitor::ask(server, &request, PROJECT_TIMEOUT).await?)
}

pub async fn run(server: &Server, name: &str, action: &ComposeAction) -> Result<Value, String> {
    let request = run_request(name, action)?;
    validated::<Started>(monitor::ask(server, &request, RUN_TIMEOUT).await?)
}

pub async fn job(server: &Server, id: &str, offset: u64) -> Result<Value, String> {
    let request = job_request(id, offset)?;
    validated::<Job>(monitor::ask(server, &request, JOB_TIMEOUT).await?)
}

pub async fn cancel(server: &Server, id: &str) -> Result<Value, String> {
    let request = cancel_request(id)?;
    validated::<Done>(monitor::ask(server, &request, RUN_TIMEOUT).await?)
}

pub async fn jobs(server: &Server) -> Result<Value, String> {
    let reply = monitor::ask(server, &json!({"op": "compose_jobs"}), JOB_TIMEOUT).await?;
    validated::<Jobs>(reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action(value: Value) -> Result<ComposeAction, String> {
        serde_json::from_value(value).map_err(|e| e.to_string())
    }

    #[test]
    fn arguments_are_checked_before_they_are_sent() {
        assert_eq!(
            project_request("app").unwrap(),
            json!({"op": "compose_project", "project": "app"})
        );
        for bad in ["", "App", "a b", "-p", "a;id", "a.b"] {
            assert!(project_request(bad).is_err(), "{bad:?}");
        }

        let up = action(json!({"kind": "up", "services": ["web"], "build": true, "pull": false, "forceRecreate": true})).unwrap();
        assert_eq!(
            run_request("app", &up).unwrap(),
            json!({"op": "compose_run", "project": "app", "action": {
                "kind": "up", "services": ["web"], "build": true, "pull": false, "forceRecreate": true}})
        );
        // Flags the UI leaves out are off.
        assert_eq!(
            action(json!({"kind": "down"})).unwrap(),
            ComposeAction::Down {
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
            let pull = ComposeAction::Pull {
                services: vec![bad.to_owned()],
            };
            assert_eq!(
                run_request("app", &pull).unwrap_err(),
                format!("Not a valid Compose service: {bad}")
            );
        }
        let many = ComposeAction::Build {
            services: (0..51).map(|i| format!("s{i}")).collect(),
            no_cache: false,
        };
        assert_eq!(
            run_request("app", &many).unwrap_err(),
            "Choose at most 50 services."
        );
        let scale = |service: &str, replicas: u32| ComposeAction::Scale {
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
            job_request(id, 42).unwrap(),
            json!({"op": "compose_job", "job": id, "offset": 42})
        );
        assert_eq!(
            cancel_request(id).unwrap(),
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
        assert_eq!(validated::<Project>(view.clone()).unwrap(), view);
        let missing = json!({
            "cli": null, "cliError": "The Compose CLI isn't installed on this server.",
            "workingDir": null, "configFiles": [], "files": [], "services": null, "configError": null
        });
        assert_eq!(validated::<Project>(missing.clone()).unwrap(), missing);
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
            let error = validated::<Project>(bad).unwrap_err();
            assert!(
                error.starts_with("Invalid compose reply: "),
                "{field}: {error}"
            );
        }

        assert_eq!(
            validated::<Started>(json!({"job": "abc"})).unwrap(),
            json!({"job": "abc"})
        );
        assert!(validated::<Started>(json!({"ok": true})).is_err());

        let job = json!({
            "job": "abc", "project": "app", "command": "docker compose -p app up -d",
            "state": "running", "exitCode": null, "startedAt": 1791109265000i64,
            "finishedAt": null, "output": "line\n", "offset": 5
        });
        assert_eq!(validated::<Job>(job.clone()).unwrap(), job);
        for state in ["succeeded", "failed", "cancelled"] {
            let mut done = job.clone();
            done["state"] = json!(state);
            assert!(validated::<Job>(done).is_ok());
        }
        for (field, value) in [
            ("state", json!("stopped")),
            ("offset", json!(-1)),
            ("output", Value::Null),
            ("startedAt", json!("now")),
        ] {
            let mut bad = job.clone();
            bad[field] = value;
            assert!(validated::<Job>(bad).is_err(), "{field}");
        }
        let listed = json!({"jobs": [job]});
        assert_eq!(validated::<Jobs>(listed.clone()).unwrap(), listed);
        assert!(validated::<Jobs>(json!({"jobs": null})).is_err());
        assert_eq!(
            validated::<Done>(json!({"ok": true})).unwrap(),
            json!({"ok": true})
        );
    }
}
