//! Presentation of the selected runtime, separately from the server default.
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action {
    Setup,
    Inspect,
    Update,
    Uninstall,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tone {
    Quiet,
    Ready,
    Warning,
}

pub(super) struct Status {
    pub label: &'static str,
    pub note: Option<&'static str>,
    pub action: Option<(Action, &'static str)>,
    pub tone: Tone,
    pub active: bool,
}

impl Status {
    fn new(
        label: &'static str,
        note: Option<&'static str>,
        action: Option<(Action, &'static str)>,
    ) -> Self {
        Self {
            label,
            note,
            action,
            tone: Tone::Quiet,
            active: false,
        }
    }
}

pub(super) fn present(
    installed: &Value,
    runtime: &Value,
    transport: Option<&str>,
    missing: bool,
) -> Status {
    if missing {
        return Status::new("Not set up", None, Some((Action::Setup, "Set up Needle")));
    }
    match transport {
        Some("disconnected") => {
            return Status::new(
                "Offline",
                Some("Connect to check the agent."),
                Some((Action::Inspect, "Connect")),
            );
        }
        Some("failed") => {
            return Status::new("Connection failed", None, Some((Action::Inspect, "Retry")));
        }
        Some("connecting") => return Status::new("Connecting…", None, None),
        Some("awaitingApproval") => return Status::new("Waiting for SSH approval", None, None),
        Some("recovering") => return Status::new("Reconnecting…", None, None),
        _ => {}
    }
    let build = runtime_build(runtime);
    let ready = runtime["status"] == "ready" && build.is_some();
    let own = build.is_some_and(|hash| installed["bundledHash"] == hash);
    let default = build.is_some_and(|hash| installed["installedHash"] == hash);
    let check_failed = installed["error"].as_str().is_some_and(|s| !s.is_empty());
    let older = installed["buildRelation"] == "older" && installed["updateAvailable"] == true;
    if ready && (own || !default || check_failed || !older) {
        return Status {
            label: "Ready",
            note: if check_failed {
                Some("Server default couldn’t be verified.")
            } else if default && installed["buildRelation"] == "newer" {
                Some("Update Newport to manage this agent.")
            } else {
                None
            },
            action: None,
            tone: Tone::Ready,
            active: true,
        };
    }
    if older {
        let required = runtime["status"] == "failed" || check_failed;
        return Status {
            label: if required {
                "Update required"
            } else {
                "Update available"
            },
            note: Some(if required {
                "Install the bundled agent to reconnect."
            } else {
                "Updates automatically while connected."
            }),
            action: Some((
                Action::Update,
                if required { "Update" } else { "Update now" },
            )),
            tone: if required { Tone::Warning } else { Tone::Quiet },
            active: ready,
        };
    }
    if runtime["status"] == "starting" {
        return Status::new("Starting…", None, None);
    }
    if check_failed || runtime["status"] == "failed" {
        return Status {
            tone: Tone::Warning,
            ..Status::new(
                "Couldn’t check",
                Some("Agent status is unavailable."),
                Some((Action::Inspect, "Retry")),
            )
        };
    }
    if installed["installed"] == true {
        return Status::new(
            "Installed",
            (installed["buildRelation"] == "newer")
                .then_some("Update Newport to manage this agent."),
            None,
        );
    }
    Status::new("Not checked", None, Some((Action::Inspect, "Check again")))
}

pub(super) fn runtime_build(runtime: &Value) -> Option<&str> {
    runtime["hello"]["buildSha256"]
        .as_str()
        .filter(|hash| newport_protocol::product::valid_build(hash))
}

/// The builds shown beside one another, each with its version: the one this
/// connection uses, the one included with Newport and the server default.
pub(super) struct Builds<'a> {
    running: (Option<&'a str>, String),
    bundled: (Option<&'a str>, String),
    installed: (Option<&'a str>, String),
}
impl<'a> Builds<'a> {
    pub(super) fn of(installed: &'a Value, runtime: &'a Value) -> Self {
        let valid = |value: &'a Value| {
            value
                .as_str()
                .filter(|hash| newport_protocol::product::valid_build(hash))
        };
        Self {
            running: (
                runtime_build(runtime),
                version(running_version(installed, runtime)),
            ),
            bundled: (
                valid(&installed["bundledHash"]),
                version(installed["bundledVersion"].as_str()),
            ),
            installed: (
                valid(&installed["installedHash"]),
                version(installed["installedVersion"].as_str()),
            ),
        }
    }
    /// The version, followed by the build's short ID when another build
    /// shown has the same version, so the two can be told apart.
    fn label(&self, (hash, version): &(Option<&str>, String)) -> String {
        let shared = [&self.running, &self.bundled, &self.installed]
            .into_iter()
            .any(|(other, other_version)| {
                other.is_some_and(|other| Some(other) != *hash) && other_version == version
            });
        match hash {
            Some(hash) if shared => format!("{version} · {}", &hash[..7]),
            _ => version.clone(),
        }
    }
    pub(super) fn running(&self) -> String {
        self.label(&self.running)
    }
    pub(super) fn bundled(&self) -> String {
        self.label(&self.bundled)
    }
    pub(super) fn installed(&self) -> String {
        self.label(&self.installed)
    }
}

pub(super) fn version(value: Option<&str>) -> String {
    value
        .filter(|version| newport_protocol::product::valid_version(version))
        .unwrap_or("Version unavailable")
        .to_owned()
}

pub(super) fn running_version<'a>(installed: &'a Value, runtime: &'a Value) -> Option<&'a str> {
    let hash = runtime_build(runtime)?;
    release(installed, Some(hash))
        .or_else(|| runtime["hello"]["agentVersion"].as_str())
        .filter(|version| newport_protocol::product::valid_version(version))
}

pub(super) fn release<'a>(installed: &'a Value, hash: Option<&str>) -> Option<&'a str> {
    let hash = hash?;
    (installed["bundledHash"] == hash)
        .then(|| installed["bundledVersion"].as_str())
        .flatten()
        .or_else(|| {
            (installed["installedHash"] == hash)
                .then(|| installed["installedVersion"].as_str())
                .flatten()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn builds_of_one_version_show_their_short_ids() {
        let a = "a1b2c3d".to_owned() + &"0".repeat(57);
        let b = "f289e54".to_owned() + &"0".repeat(57);
        let installed = json!({"installedHash":a,"installedVersion":"0.7.0","bundledHash":b,"bundledVersion":"0.7.0"});
        let runtime = json!({"hello":{"buildSha256":a}});
        let builds = Builds::of(&installed, &runtime);
        assert_eq!(builds.running(), "0.7.0 · a1b2c3d");
        assert_eq!(builds.bundled(), "0.7.0 · f289e54");
        assert_eq!(builds.installed(), "0.7.0 · a1b2c3d");
        // One build, or builds whose versions differ: versions alone.
        let same = json!({"installedHash":b,"installedVersion":"0.7.0","bundledHash":b,"bundledVersion":"0.7.0"});
        let on_same = json!({"hello":{"buildSha256":b}});
        let builds = Builds::of(&same, &on_same);
        assert_eq!(
            (builds.running(), builds.bundled()),
            ("0.7.0".into(), "0.7.0".into())
        );
        let older = json!({"installedHash":a,"installedVersion":"0.6.4","bundledHash":b,"bundledVersion":"0.7.0"});
        let builds = Builds::of(&older, &on_same);
        assert_eq!(
            (builds.installed(), builds.bundled()),
            ("0.6.4".into(), "0.7.0".into())
        );
    }

    #[test]
    fn versions_follow_the_exact_runtime_and_do_not_fall_back_to_hashes() {
        let installed = json!({
            "installedHash":"a".repeat(64),"installedVersion":"0.7.0",
            "bundledHash":"b".repeat(64),"bundledVersion":"0.6.4"
        });
        let runtime = json!({"hello":{"buildSha256":"b".repeat(64),"agentVersion":"0.6.4"}});
        assert_eq!(running_version(&installed, &runtime), Some("0.6.4"));
        let other = json!({"hello":{"buildSha256":"c".repeat(64),"agentVersion":"0.8.0-rc.1"}});
        assert_eq!(running_version(&installed, &other), Some("0.8.0-rc.1"));
        let unknown = json!({"hello":{"buildSha256":"c".repeat(64)}});
        assert_eq!(running_version(&installed, &unknown), None);
        assert_eq!(version(None), "Version unavailable");
        assert_eq!(version(Some(&"a".repeat(64))), "Version unavailable");
    }

    #[test]
    fn disconnected_transport_invalidates_a_retained_ready_hello() {
        let runtime = json!({"status":"ready","hello":{"buildSha256":"b".repeat(64)}});
        let status = present(
            &json!({"installed":true}),
            &runtime,
            Some("disconnected"),
            false,
        );
        assert_eq!(status.label, "Offline");
        assert!(!status.active);
        let approval = present(
            &json!({"installed":true}),
            &runtime,
            Some("awaitingApproval"),
            false,
        );
        assert_eq!(approval.label, "Waiting for SSH approval");
        assert!(!approval.active);
    }

    #[test]
    fn unknown_check_failure_never_promises_an_automatic_update() {
        let status = present(
            &json!({"installed":true,"updateAvailable":true,"error":"failed"}),
            &Value::Null,
            None,
            false,
        );
        assert_eq!(status.label, "Couldn’t check");
        assert_eq!(status.action, Some((Action::Inspect, "Retry")));
    }

    #[test]
    fn signed_default_version_does_not_label_another_running_build() {
        let installed = json!({"installedHash":"a".repeat(64),"installedVersion":"0.7.0","bundledHash":"b".repeat(64)});
        assert_eq!(release(&installed, Some(&"b".repeat(64))), None);
        assert_eq!(release(&installed, Some(&"a".repeat(64))), Some("0.7.0"));
    }

    #[test]
    fn a_known_old_protocol_offers_explicit_installation_despite_unknown_product() {
        let status = present(
            &json!({"installed":true,"buildRelation":"older","updateAvailable":true,"error":"product unknown"}),
            &json!({"status":"failed"}),
            None,
            false,
        );
        assert_eq!(status.label, "Update required");
        assert_eq!(status.action, Some((Action::Update, "Update")));
    }
}
