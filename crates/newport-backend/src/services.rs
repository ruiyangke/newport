//! The Services tab: systemd services and timers through the server's agent
//! channel. Arguments are validated here and again by the agent; replies are
//! checked against this contract before they reach the UI.
use crate::{monitor, server_session::ServerSession};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Duration;

// Two thousand units' properties take a few batched systemctl calls.
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(30);
const READ_TIMEOUT: Duration = Duration::from_secs(20);
// A unit's stop or start may legitimately take its full timeout.
const ACTION_TIMEOUT: Duration = Duration::from_secs(75);

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    System,
    User,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Verb {
    Start,
    Stop,
    Restart,
    Reload,
    Enable,
    Disable,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    Err,
    Warning,
}
#[derive(Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Privilege {
    Root,
    Sudo,
    Password,
    None,
}

/// systemd's unit-name characters (with `\` escapes), no leading dash, and
/// the expected suffix; timers only where reading them makes sense.
pub fn valid_unit(name: &str, timers: bool) -> bool {
    let stem = name
        .strip_suffix(".service")
        .or_else(|| timers.then(|| name.strip_suffix(".timer")).flatten());
    stem.is_some_and(|stem| {
        !stem.is_empty()
            && stem.len() <= 255
            && !name.starts_with('-')
            && stem
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b":_.@-\\".contains(&b))
    })
}
fn checked(unit: &str, timers: bool) -> Result<(), String> {
    if valid_unit(unit, timers) {
        Ok(())
    } else {
        Err(format!("Not a valid unit name: {unit}"))
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Service {
    name: String,
    scope: Scope,
    description: String,
    load: String,
    active: String,
    sub: String,
    enabled: Option<String>,
    main_pid: Option<u32>,
    since: Option<i64>,
    memory: Option<u64>,
    cpu: Option<f64>,
    restarts: Option<u64>,
    result: Option<String>,
    exit_code: Option<i64>,
    exit_kind: Option<String>,
    fragment: Option<String>,
    drop_ins: Vec<String>,
    triggered_by: Vec<String>,
    user: Option<String>,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Timer {
    name: String,
    scope: Scope,
    description: String,
    active: String,
    next: Option<i64>,
    last: Option<i64>,
    unit: Option<String>,
}
#[derive(Debug, Deserialize, Serialize)]
struct UserManager {
    available: bool,
    reason: Option<String>,
}
#[derive(Debug, Deserialize, Serialize)]
struct Snapshot {
    services: Vec<Service>,
    timers: Vec<Timer>,
    user: UserManager,
    privilege: Privilege,
}
#[derive(Debug, Deserialize, Serialize)]
struct Entry {
    at: Option<i64>,
    priority: Option<u8>,
    pid: Option<u32>,
    identifier: Option<String>,
    message: String,
}
#[derive(Debug, Deserialize, Serialize)]
struct Logs {
    entries: Vec<Entry>,
    cursor: Option<String>,
}
#[derive(Debug, Deserialize, Serialize)]
struct UnitFile {
    text: String,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ActionReply {
    ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    needs_password: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    command: Option<String>,
}

fn validated<T: serde::de::DeserializeOwned + Serialize>(reply: Value) -> Result<Value, String> {
    let typed: T =
        serde_json::from_value(reply).map_err(|e| format!("Invalid services reply: {e}"))?;
    serde_json::to_value(typed).map_err(|e| e.to_string())
}

pub async fn snapshot(server: &std::sync::Arc<ServerSession>) -> Result<Value, String> {
    let reply = monitor::ask(server, &json!({"op": "services"}), SNAPSHOT_TIMEOUT).await?;
    validated::<Snapshot>(reply)
}

pub async fn logs(
    server: &std::sync::Arc<ServerSession>,
    unit: Option<&str>,
    scope: Scope,
    priority: Option<Priority>,
    cursor: Option<&str>,
    lines: Option<u32>,
) -> Result<Value, String> {
    if let Some(unit) = unit {
        checked(unit, true)?;
    }
    if cursor.is_some_and(|c| c.len() > 512 || c.chars().any(char::is_control)) {
        return Err("Invalid journal cursor.".into());
    }
    let request = json!({
        "op": "logs", "unit": unit, "scope": scope, "priority": priority,
        "cursor": cursor, "lines": lines.map(|n| n.clamp(1, 1000)),
    });
    validated::<Logs>(monitor::ask(server, &request, READ_TIMEOUT).await?)
}

pub async fn unit_file(
    server: &std::sync::Arc<ServerSession>,
    unit: &str,
    scope: Scope,
) -> Result<Value, String> {
    checked(unit, true)?;
    let request = json!({"op": "unit", "unit": unit, "scope": scope});
    validated::<UnitFile>(monitor::ask(server, &request, READ_TIMEOUT).await?)
}

pub async fn action(
    server: &std::sync::Arc<ServerSession>,
    unit: &str,
    scope: Scope,
    verb: Verb,
) -> Result<Value, String> {
    checked(unit, false)?;
    let request = json!({"op": "action", "unit": unit, "scope": scope, "verb": verb});
    validated::<ActionReply>(monitor::ask(server, &request, ACTION_TIMEOUT).await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_names_cannot_smuggle_arguments() {
        for good in [
            "nginx.service",
            "postgresql@16-main.service",
            "dev-sda1\\x2d.service",
        ] {
            assert!(valid_unit(good, false), "{good}");
        }
        assert!(valid_unit("certbot.timer", true));
        for bad in [
            "certbot.timer",
            "-x.service",
            "a b.service",
            "$(id).service",
            "x.socket",
            ".service",
        ] {
            assert!(!valid_unit(bad, false), "{bad}");
        }
    }

    #[test]
    fn replies_are_held_to_the_contract() {
        let snapshot = json!({
            "services": [{
                "name": "nginx.service", "scope": "system", "description": "web", "load": "loaded",
                "active": "active", "sub": "running", "enabled": "enabled", "mainPid": 812,
                "since": 1791109265000i64, "memory": 43122688, "cpu": 1.5, "restarts": 0,
                "result": "success", "exitCode": null, "exitKind": null,
                "fragment": "/lib/systemd/system/nginx.service", "dropIns": [], "triggeredBy": [], "user": null
            }],
            "timers": [{"name": "certbot.timer", "scope": "system", "description": "", "active": "active",
                        "next": 1791115200000i64, "last": null, "unit": "certbot.service"}],
            "user": {"available": false, "reason": "Failed to connect to bus"},
            "privilege": "password"
        });
        assert_eq!(validated::<Snapshot>(snapshot.clone()).unwrap(), snapshot);
        let mut bad = snapshot.clone();
        bad["privilege"] = "admin".into();
        assert!(validated::<Snapshot>(bad).is_err());
        let mut bad = snapshot;
        bad["services"][0]["scope"] = "global".into();
        assert!(validated::<Snapshot>(bad).is_err());

        assert_eq!(
            validated::<ActionReply>(json!({"ok": true})).unwrap(),
            json!({"ok": true})
        );
        let password = json!({"ok": false, "needsPassword": true, "command": "sudo systemctl stop nginx.service"});
        assert_eq!(
            validated::<ActionReply>(password.clone()).unwrap(),
            password
        );
        let logs = json!({"entries": [{"at": 1, "priority": 3, "pid": null, "identifier": "x", "message": "m"}], "cursor": "c"});
        assert_eq!(validated::<Logs>(logs.clone()).unwrap(), logs);
        assert!(validated::<Logs>(json!({"entries": [{"message": 5}], "cursor": null})).is_err());
    }
}
