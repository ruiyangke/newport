//! The Services tab: systemd services and timers through the server's agent
//! channel. Arguments are validated here and again by the agent; replies are
//! checked against this contract before they reach the UI.
use crate::{monitor, server_session::ServerSession};
use newport_protocol::monitor::services::{
    LogsRequest, MAX_LOG_ENTRIES, Priority, Reply, Request, Scope, Verb,
};
use serde_json::Value;
use std::time::Duration;

// Two thousand units' properties take a few batched systemctl calls.
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(30);
const READ_TIMEOUT: Duration = Duration::from_secs(20);
// A unit's stop or start may legitimately take its full timeout.
const ACTION_TIMEOUT: Duration = Duration::from_secs(75);

#[cfg(test)]
fn validated(request: &Request, reply: Value) -> Result<Value, String> {
    let reply = match request {
        Request::Snapshot => serde_json::from_value(reply).map(Reply::Snapshot),
        Request::Logs(_) => serde_json::from_value(reply).map(Reply::Logs),
        Request::Unit { .. } => serde_json::from_value(reply).map(Reply::Unit),
        Request::Action { .. } => serde_json::from_value(reply).map(Reply::Action),
    }
    .map_err(|e| format!("Invalid services reply: {e}"))?;
    reply.validate_for(request).map_err(|e| e.to_string())?;
    match reply {
        Reply::Snapshot(value) => serde_json::to_value(value),
        Reply::Logs(value) => serde_json::to_value(value),
        Reply::Unit(value) => serde_json::to_value(value),
        Reply::Action(value) => serde_json::to_value(value),
    }
    .map_err(|e| e.to_string())
}

async fn ask_typed(
    server: &std::sync::Arc<ServerSession>,
    request: Request,
    timeout: Duration,
) -> Result<Reply, String> {
    request.validate().map_err(|error| error.to_string())?;
    let reply = monitor::ask(
        server,
        &newport_protocol::monitor::Request::Services(request),
        timeout,
    )
    .await?;
    let newport_protocol::monitor::Reply::Services(reply) = reply else {
        return Err("Invalid services response operation.".into());
    };
    Ok(reply)
}
async fn ask(
    server: &std::sync::Arc<ServerSession>,
    request: Request,
    timeout: Duration,
) -> Result<Value, String> {
    match ask_typed(server, request, timeout).await? {
        Reply::Snapshot(value) => serde_json::to_value(value),
        Reply::Logs(value) => serde_json::to_value(value),
        Reply::Unit(value) => serde_json::to_value(value),
        Reply::Action(value) => serde_json::to_value(value),
    }
    .map_err(|error| error.to_string())
}
/// Typed service boundary shared by the cache and desktop protobuf adapter.
pub(crate) async fn request(
    server: &std::sync::Arc<ServerSession>,
    request: Request,
) -> Result<Reply, String> {
    let timeout = match &request {
        Request::Snapshot => SNAPSHOT_TIMEOUT,
        Request::Logs(_) | Request::Unit { .. } => READ_TIMEOUT,
        Request::Action { .. } => ACTION_TIMEOUT,
    };
    ask_typed(server, request, timeout).await
}

pub async fn snapshot(server: &std::sync::Arc<ServerSession>) -> Result<Value, String> {
    ask(server, Request::Snapshot, SNAPSHOT_TIMEOUT).await
}

pub async fn logs(
    server: &std::sync::Arc<ServerSession>,
    unit: Option<&str>,
    scope: Scope,
    priority: Option<Priority>,
    cursor: Option<&str>,
    lines: Option<u32>,
) -> Result<Value, String> {
    let request = LogsRequest {
        scope,
        unit: unit.map(str::to_owned),
        priority,
        cursor: cursor.map(str::to_owned),
        lines: lines.map(|n| n.clamp(1, MAX_LOG_ENTRIES as u32)),
    };
    ask(server, Request::Logs(request), READ_TIMEOUT).await
}

pub async fn unit_file(
    server: &std::sync::Arc<ServerSession>,
    unit: &str,
    scope: Scope,
) -> Result<Value, String> {
    ask(
        server,
        Request::Unit {
            scope,
            unit: unit.into(),
        },
        READ_TIMEOUT,
    )
    .await
}

pub async fn action(
    server: &std::sync::Arc<ServerSession>,
    unit: &str,
    scope: Scope,
    verb: Verb,
) -> Result<Value, String> {
    ask(
        server,
        Request::Action {
            scope,
            unit: unit.into(),
            verb,
        },
        ACTION_TIMEOUT,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use newport_protocol::monitor::services::valid_unit;
    use serde_json::json;

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
        assert_eq!(
            validated(&Request::Snapshot, snapshot.clone()).unwrap(),
            snapshot
        );
        let mut bad = snapshot.clone();
        bad["privilege"] = "admin".into();
        assert!(validated(&Request::Snapshot, bad).is_err());
        let mut bad = snapshot;
        bad["services"][0]["scope"] = "global".into();
        assert!(validated(&Request::Snapshot, bad).is_err());

        let action = Request::Action {
            scope: Scope::System,
            unit: "nginx.service".into(),
            verb: Verb::Stop,
        };
        assert_eq!(
            validated(&action, json!({"ok": true})).unwrap(),
            json!({"ok": true})
        );
        let password = json!({"ok": false, "needsPassword": true, "command": "sudo systemctl stop nginx.service"});
        assert_eq!(validated(&action, password.clone()).unwrap(), password);
        assert!(
            validated(
                &action,
                json!({"ok": true, "needsPassword": true, "command": "sudo"})
            )
            .is_err()
        );
        let logs = json!({"entries": [{"at": 1, "priority": 3, "pid": null, "identifier": "x", "message": "m"}], "cursor": "c"});
        assert_eq!(
            validated(&Request::Logs(LogsRequest::default()), logs.clone()).unwrap(),
            logs
        );
        assert!(
            validated(
                &Request::Logs(LogsRequest::default()),
                json!({"entries": [{"message": 5}], "cursor": null})
            )
            .is_err()
        );
    }
}
