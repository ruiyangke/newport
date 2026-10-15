//! The Services tab through the agent against systemd as PID 1
//! (tests/remote/systemd): `fixture` has passwordless sudo for systemctl, the
//! journal, and a lingering user manager; `keeper` would need a password.
use crate::{
    model::Server,
    services::{self, Priority, Scope, Verb},
};
use serde_json::{Value, json};
use tokio::time::{Duration, sleep};

async fn systemd(user: &str) -> Server {
    super::installed_server("NEWPORT_TEST_SYSTEMD_PORT", "Disposable systemd", user).await
}
fn unit<'a>(snapshot: &'a Value, kind: &str, name: &str, scope: &str) -> Option<&'a Value> {
    snapshot[kind]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["name"] == name && u["scope"] == scope)
}
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}
/// Snapshots until `ready` holds; units settle on their own schedule.
async fn settled(server: &Server, ready: impl Fn(&Value) -> bool) -> Value {
    let mut snapshot = Value::Null;
    for _ in 0..40 {
        snapshot = services::snapshot(&crate::remote_tests::owner(server))
            .await
            .unwrap();
        if ready(&snapshot) {
            return snapshot;
        }
        sleep(Duration::from_millis(500)).await;
    }
    panic!("services never settled: {snapshot:#}");
}

#[tokio::test]
#[ignore = "Requires npm run test:remote"]
async fn snapshot_reports_services_timers_and_the_user_manager() {
    let server = systemd("fixture").await;
    let snapshot = settled(&server, |s| {
        unit(s, "services", "newport-broken.service", "system").is_some_and(|u| u["active"] == "failed")
            && unit(s, "timers", "newport-demo.timer", "system").is_some_and(|t| t["last"].is_i64())
            && unit(s, "services", "newport-user.service", "user").is_some()
            // CPU is a delta, so it appears from the second reading on.
            && unit(s, "services", "newport-demo.service", "system").is_some_and(|u| u["cpu"].is_f64())
    })
    .await;
    assert_eq!(snapshot["privilege"], "sudo");
    assert_eq!(snapshot["user"], json!({"available": true, "reason": null}));
    let services = snapshot["services"].as_array().unwrap();
    assert!(services.len() > 10 && services.len() <= 2000);

    let demo = unit(&snapshot, "services", "newport-demo.service", "system").unwrap();
    for (field, value) in [
        ("description", json!("Newport demo worker")),
        ("load", json!("loaded")),
        ("active", json!("active")),
        ("sub", json!("running")),
        ("enabled", json!("enabled")),
        ("restarts", json!(0)),
        ("result", json!("success")),
        ("exitCode", Value::Null),
        ("exitKind", Value::Null),
        (
            "fragment",
            json!("/etc/systemd/system/newport-demo.service"),
        ),
        (
            "dropIns",
            json!(["/etc/systemd/system/newport-demo.service.d/override.conf"]),
        ),
        ("user", json!("nobody")),
    ] {
        assert_eq!(demo[field], value, "{field} in {demo:#}");
    }
    assert!(demo["mainPid"].as_u64().is_some_and(|pid| pid > 1));
    assert!(demo["since"].as_i64().is_some_and(|t| t > 0 && t <= now()));
    assert!(demo["memory"].as_u64().is_some_and(|m| m > 0), "{demo:#}");
    assert!(
        demo["cpu"]
            .as_f64()
            .is_some_and(|c| (0.0..=100.0).contains(&c))
    );

    let broken = unit(&snapshot, "services", "newport-broken.service", "system").unwrap();
    assert_eq!(broken["sub"], "failed");
    assert_eq!(broken["result"], "exit-code");
    assert_eq!(broken["exitCode"], 3);
    assert_eq!(broken["exitKind"], "exited");
    assert_eq!(broken["mainPid"], Value::Null);
    assert_eq!(broken["memory"], Value::Null);

    let tick = unit(&snapshot, "services", "newport-tick.service", "system").unwrap();
    assert_eq!(tick["triggeredBy"], json!(["newport-demo.timer"]));
    let timer = unit(&snapshot, "timers", "newport-demo.timer", "system").unwrap();
    assert_eq!(timer["description"], "Newport demo schedule");
    assert_eq!(timer["active"], "active");
    assert_eq!(timer["unit"], "newport-tick.service");
    let (next, last) = (
        timer["next"].as_i64().unwrap(),
        timer["last"].as_i64().unwrap(),
    );
    assert!(
        last <= now() && next > now() && next - now() <= 3_600_000,
        "{timer:#}"
    );

    let user = unit(&snapshot, "services", "newport-user.service", "user").unwrap();
    assert_eq!(user["active"], "active");
    assert_eq!(user["enabled"], "enabled");
    assert!(user["mainPid"].as_u64().is_some());
    assert!(
        user["fragment"]
            .as_str()
            .unwrap()
            .ends_with("/.config/systemd/user/newport-user.service")
    );
}

#[tokio::test]
#[ignore = "Requires npm run test:remote"]
async fn logs_follow_a_cursor_and_unit_files_include_drop_ins() {
    let server = systemd("fixture").await;
    let demo = Some("newport-demo.service");
    let first = services::logs(
        &crate::remote_tests::owner(&server),
        demo,
        Scope::System,
        None,
        None,
        Some(5),
    )
    .await
    .unwrap();
    let entries = first["entries"].as_array().unwrap();
    assert!(!entries.is_empty() && entries.len() <= 5, "{first:#}");
    let tick = entries
        .iter()
        .find(|e| e["message"] == "demo tick")
        .unwrap();
    assert_eq!(tick["identifier"], "sh");
    assert_eq!(tick["priority"], 6);
    assert!(tick["pid"].as_u64().is_some());
    let cursor = first["cursor"].as_str().unwrap().to_owned();
    let last_at = entries.last().unwrap()["at"].as_i64().unwrap();
    assert!(
        entries
            .windows(2)
            .all(|w| w[0]["at"].as_i64() <= w[1]["at"].as_i64())
    );

    // The loop writes every two seconds; a tail returns only what is new.
    sleep(Duration::from_millis(4500)).await;
    let tail = services::logs(
        &crate::remote_tests::owner(&server),
        demo,
        Scope::System,
        None,
        Some(&cursor),
        None,
    )
    .await
    .unwrap();
    let new = tail["entries"].as_array().unwrap();
    assert!(!new.is_empty(), "{tail:#}");
    assert!(new.iter().all(|e| e["at"].as_i64().unwrap() >= last_at));
    assert_ne!(tail["cursor"], json!(cursor));
    // Nothing new keeps the caller's cursor.
    let quiet = services::logs(
        &crate::remote_tests::owner(&server),
        Some("newport-broken.service"),
        Scope::System,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let mark = quiet["cursor"].as_str().unwrap();
    let still = services::logs(
        &crate::remote_tests::owner(&server),
        Some("newport-broken.service"),
        Scope::System,
        None,
        Some(mark),
        None,
    )
    .await
    .unwrap();
    assert_eq!(still, json!({"entries": [], "cursor": mark}));

    let failures = services::logs(
        &crate::remote_tests::owner(&server),
        Some("newport-broken.service"),
        Scope::System,
        Some(Priority::Warning),
        None,
        None,
    )
    .await
    .unwrap();
    let failures = failures["entries"].as_array().unwrap();
    assert!(
        failures
            .iter()
            .any(|e| e["message"].as_str().unwrap().contains("exit-code"))
    );
    assert!(failures.iter().all(|e| e["priority"].as_u64() <= Some(4)));

    let user = services::logs(
        &crate::remote_tests::owner(&server),
        Some("newport-user.service"),
        Scope::User,
        None,
        None,
        Some(3),
    )
    .await
    .unwrap();
    assert!(
        user["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["message"] == "user tick")
    );
    let all_user = services::logs(
        &crate::remote_tests::owner(&server),
        None,
        Scope::User,
        None,
        None,
        Some(20),
    )
    .await
    .unwrap();
    assert!(!all_user["entries"].as_array().unwrap().is_empty());

    let file = services::unit_file(
        &crate::remote_tests::owner(&server),
        "newport-demo.service",
        Scope::System,
    )
    .await
    .unwrap();
    let text = file["text"].as_str().unwrap();
    assert!(text.contains("ExecStart=/bin/sh -c") && text.contains("NEWPORT_DROP_IN=1"));
    let timer = services::unit_file(
        &crate::remote_tests::owner(&server),
        "newport-demo.timer",
        Scope::System,
    )
    .await
    .unwrap();
    assert!(
        timer["text"]
            .as_str()
            .unwrap()
            .contains("OnCalendar=hourly")
    );
    let user = services::unit_file(
        &crate::remote_tests::owner(&server),
        "newport-user.service",
        Scope::User,
    )
    .await
    .unwrap();
    assert!(user["text"].as_str().unwrap().contains("user tick"));
    let missing = services::unit_file(
        &crate::remote_tests::owner(&server),
        "newport-absent.service",
        Scope::System,
    )
    .await;
    assert!(missing.is_err());
    let error = services::unit_file(
        &crate::remote_tests::owner(&server),
        "-H.service",
        Scope::System,
    )
    .await
    .unwrap_err();
    assert!(error.contains("Not a valid unit name"), "{error}");

    // Not in systemd-journal: the system journal is closed to this account,
    // whether journalctl fails (no journal of its own) or, once the account
    // has logged something, quietly shows nothing.
    let keeper = systemd("keeper").await;
    for setup in ["true", "logger newport && sleep 1"] {
        crate::ssh::execute(&keeper, setup, None).await.unwrap();
        let error = services::logs(
            &crate::remote_tests::owner(&keeper),
            demo,
            Scope::System,
            None,
            None,
            None,
        )
        .await
        .unwrap_err();
        assert!(error.contains("cannot read that journal"), "{error}");
    }
}

#[tokio::test]
#[ignore = "Requires npm run test:remote"]
async fn actions_use_sudo_or_ask_for_a_password() {
    let server = systemd("fixture").await;
    let demo = "newport-demo.service";
    let pid_of = |s: &Value| unit(s, "services", demo, "system").unwrap()["mainPid"].clone();
    let before = pid_of(&settled(&server, |s| pid_of(s).is_u64()).await);
    for verb in [Verb::Restart, Verb::Stop] {
        let reply = services::action(
            &crate::remote_tests::owner(&server),
            demo,
            Scope::System,
            verb,
        )
        .await;
        assert_eq!(reply.unwrap(), json!({"ok": true}));
    }
    settled(&server, |s| {
        unit(s, "services", demo, "system").unwrap()["active"] == "inactive"
    })
    .await;
    for verb in [Verb::Start, Verb::Disable] {
        let reply = services::action(
            &crate::remote_tests::owner(&server),
            demo,
            Scope::System,
            verb,
        )
        .await;
        assert_eq!(reply.unwrap(), json!({"ok": true}));
    }
    let after = settled(&server, |s| {
        let u = unit(s, "services", demo, "system").unwrap();
        u["active"] == "active" && u["enabled"] == "disabled"
    })
    .await;
    assert_ne!(pid_of(&after), before);
    let reply = services::action(
        &crate::remote_tests::owner(&server),
        demo,
        Scope::System,
        Verb::Enable,
    )
    .await;
    assert_eq!(reply.unwrap(), json!({"ok": true}));
    settled(&server, |s| {
        unit(s, "services", demo, "system").unwrap()["enabled"] == "enabled"
    })
    .await;

    // The user manager is the account's own: no sudo involved.
    let reply = services::action(
        &crate::remote_tests::owner(&server),
        "newport-user.service",
        Scope::User,
        Verb::Restart,
    )
    .await;
    assert_eq!(reply.unwrap(), json!({"ok": true}));
    let error = services::action(
        &crate::remote_tests::owner(&server),
        "newport-absent.service",
        Scope::System,
        Verb::Start,
    )
    .await
    .unwrap_err();
    assert!(error.contains("newport-absent.service"), "{error}");
    let error = services::action(
        &crate::remote_tests::owner(&server),
        "newport-demo.timer",
        Scope::System,
        Verb::Start,
    )
    .await
    .unwrap_err();
    assert!(error.contains("Not a valid unit name"), "{error}");

    // An ordinary sudoer: shown the command to run instead of a hang or an
    // auth failure, and without a lingering manager no user services.
    let keeper = systemd("keeper").await;
    let snapshot = services::snapshot(&crate::remote_tests::owner(&keeper))
        .await
        .unwrap();
    assert_eq!(snapshot["privilege"], "password");
    assert_eq!(snapshot["user"]["available"], false);
    assert!(
        snapshot["user"]["reason"]
            .as_str()
            .is_some_and(|r| !r.is_empty())
    );
    assert!(unit(&snapshot, "services", demo, "system").is_some());
    let reply = services::action(
        &crate::remote_tests::owner(&keeper),
        demo,
        Scope::System,
        Verb::Restart,
    )
    .await;
    assert_eq!(
        reply.unwrap(),
        json!({"ok": false, "needsPassword": true, "command": "sudo systemctl restart newport-demo.service"})
    );
}
