//! Presentation of the selected runtime, separately from the server default.
use newport_domain::connectivity::Phase;
use newport_protocol::desktop::configuration::{
    BuildRelation, InstallationStatus, RuntimeSnapshot, RuntimeStatus,
};

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
    installed: Option<&InstallationStatus>,
    runtime: Option<&RuntimeSnapshot>,
    transport: Option<Phase>,
    missing: bool,
) -> Status {
    if missing {
        return Status::new("Not set up", None, Some((Action::Setup, "Set up Needle")));
    }
    match transport {
        Some(Phase::Disconnected) => {
            return Status::new(
                "Offline",
                Some("Connect to check the agent."),
                Some((Action::Inspect, "Connect")),
            );
        }
        Some(Phase::Failed) => {
            return Status::new("Connection failed", None, Some((Action::Inspect, "Retry")));
        }
        Some(Phase::Connecting) => return Status::new("Connecting…", None, None),
        Some(Phase::AwaitingApproval) => {
            return Status::new("Waiting for SSH approval", None, None);
        }
        Some(Phase::Reconnecting) => return Status::new("Reconnecting…", None, None),
        Some(Phase::Connected) | None => {}
    }
    let status = runtime.map(|runtime| runtime.status);
    let relation = installed.map(|installed| installed.build_relation);
    let build = runtime_build(runtime);
    let ready = status == Some(RuntimeStatus::Ready) && build.is_some();
    let own = build.is_some_and(|hash| {
        installed.and_then(|installed| installed.bundled_hash.as_deref()) == Some(hash)
    });
    let default = build.is_some_and(|hash| {
        installed.and_then(|installed| installed.installed_hash.as_deref()) == Some(hash)
    });
    let check_failed = installed
        .and_then(|installed| installed.error.as_deref())
        .is_some_and(|s| !s.is_empty());
    let older = relation == Some(BuildRelation::Older)
        && installed.is_some_and(|installed| installed.update_available == Some(true));
    let newer = relation == Some(BuildRelation::Newer);
    if ready && (own || !default || check_failed || !older) {
        return Status {
            label: "Ready",
            note: if check_failed {
                Some("Server default couldn’t be verified.")
            } else if default && newer {
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
        let required = status == Some(RuntimeStatus::Failed) || check_failed;
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
    if status == Some(RuntimeStatus::Starting) {
        return Status::new("Starting…", None, None);
    }
    if check_failed || status == Some(RuntimeStatus::Failed) {
        return Status {
            tone: Tone::Warning,
            ..Status::new(
                "Couldn’t check",
                Some("Agent status is unavailable."),
                Some((Action::Inspect, "Retry")),
            )
        };
    }
    if installed.is_some_and(|installed| installed.installed == Some(true)) {
        return Status::new(
            "Installed",
            newer.then_some("Update Newport to manage this agent."),
            None,
        );
    }
    Status::new("Not checked", None, Some((Action::Inspect, "Check again")))
}

pub(super) fn runtime_build(runtime: Option<&RuntimeSnapshot>) -> Option<&str> {
    runtime?
        .hello
        .as_ref()
        .map(|hello| hello.build_sha256.as_str())
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
    pub(super) fn of(
        installed: Option<&'a InstallationStatus>,
        runtime: Option<&'a RuntimeSnapshot>,
    ) -> Self {
        let valid = |value: Option<&'a String>| {
            value
                .map(String::as_str)
                .filter(|hash| newport_protocol::product::valid_build(hash))
        };
        Self {
            running: (
                runtime_build(runtime),
                version(running_version(installed, runtime)),
            ),
            bundled: (
                valid(installed.and_then(|installed| installed.bundled_hash.as_ref())),
                version(installed.and_then(|installed| installed.bundled_version.as_deref())),
            ),
            installed: (
                valid(installed.and_then(|installed| installed.installed_hash.as_ref())),
                version(installed.and_then(|installed| installed.installed_version.as_deref())),
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

/// The running build's version: the release Newport knows by its hash, or
/// else the version the agent reported in its hello.
pub(super) fn running_version<'a>(
    installed: Option<&'a InstallationStatus>,
    runtime: Option<&'a RuntimeSnapshot>,
) -> Option<&'a str> {
    let hash = runtime_build(runtime)?;
    release(installed, Some(hash))
        .or_else(|| runtime?.hello.as_ref()?.agent_version.as_deref())
        .filter(|version| newport_protocol::product::valid_version(version))
}

pub(super) fn release<'a>(
    installed: Option<&'a InstallationStatus>,
    hash: Option<&str>,
) -> Option<&'a str> {
    let hash = hash?;
    let installed = installed?;
    (installed.bundled_hash.as_deref() == Some(hash))
        .then_some(installed.bundled_version.as_deref())
        .flatten()
        .or_else(|| {
            (installed.installed_hash.as_deref() == Some(hash))
                .then_some(installed.installed_version.as_deref())
                .flatten()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot_fixture as fixture;

    /// An agent check that names the server default and the bundled build.
    fn builds(installed: (&str, &str), bundled: (&str, Option<&str>)) -> InstallationStatus {
        InstallationStatus {
            installed_hash: Some(installed.0.into()),
            installed_version: Some(installed.1.into()),
            bundled_hash: Some(bundled.0.into()),
            bundled_version: bundled.1.map(Into::into),
            ..fixture::installation()
        }
    }
    /// A runtime running `build`, which reported `version` in its hello.
    fn running(status: RuntimeStatus, build: &str, version: Option<&str>) -> RuntimeSnapshot {
        let mut hello = fixture::hello(build);
        hello.agent_version = version.map(Into::into);
        fixture::agent_runtime(status, Some(hello))
    }

    #[test]
    fn builds_of_one_version_show_their_short_ids() {
        let a = "a1b2c3d".to_owned() + &"0".repeat(57);
        let b = "f289e54".to_owned() + &"0".repeat(57);
        let installed = builds((&a, "0.7.0"), (&b, Some("0.7.0")));
        let runtime = running(RuntimeStatus::Unknown, &a, None);
        let builds_shown = Builds::of(Some(&installed), Some(&runtime));
        assert_eq!(builds_shown.running(), "0.7.0 · a1b2c3d");
        assert_eq!(builds_shown.bundled(), "0.7.0 · f289e54");
        assert_eq!(builds_shown.installed(), "0.7.0 · a1b2c3d");
        // One build, or builds whose versions differ: versions alone.
        let same = builds((&b, "0.7.0"), (&b, Some("0.7.0")));
        let on_same = running(RuntimeStatus::Unknown, &b, None);
        let shown = Builds::of(Some(&same), Some(&on_same));
        assert_eq!(
            (shown.running(), shown.bundled()),
            ("0.7.0".into(), "0.7.0".into())
        );
        let older = builds((&a, "0.6.4"), (&b, Some("0.7.0")));
        let shown = Builds::of(Some(&older), Some(&on_same));
        assert_eq!(
            (shown.installed(), shown.bundled()),
            ("0.6.4".into(), "0.7.0".into())
        );
    }

    #[test]
    fn versions_follow_the_exact_runtime_and_do_not_fall_back_to_hashes() {
        let installed = builds((&"a".repeat(64), "0.7.0"), (&"b".repeat(64), Some("0.6.4")));
        let runtime = running(RuntimeStatus::Ready, &"b".repeat(64), Some("0.6.4"));
        assert_eq!(
            running_version(Some(&installed), Some(&runtime)),
            Some("0.6.4")
        );
        let other = running(RuntimeStatus::Ready, &"c".repeat(64), Some("0.8.0-rc.1"));
        assert_eq!(
            running_version(Some(&installed), Some(&other)),
            Some("0.8.0-rc.1")
        );
        let unknown = running(RuntimeStatus::Ready, &"c".repeat(64), None);
        assert_eq!(running_version(Some(&installed), Some(&unknown)), None);
        assert_eq!(version(None), "Version unavailable");
        assert_eq!(version(Some(&"a".repeat(64))), "Version unavailable");
    }

    #[test]
    fn a_build_newport_does_not_know_shows_the_version_its_agent_reported() {
        let runtime = running(RuntimeStatus::Ready, &"c".repeat(64), Some("0.7.4"));
        assert_eq!(running_version(None, Some(&runtime)), Some("0.7.4"));
        let installed = builds((&"a".repeat(64), "0.7.0"), (&"b".repeat(64), None));
        assert_eq!(
            running_version(Some(&installed), Some(&runtime)),
            Some("0.7.4")
        );
        assert_eq!(
            Builds::of(Some(&installed), Some(&runtime)).running(),
            "0.7.4"
        );
    }

    #[test]
    fn disconnected_transport_invalidates_a_retained_ready_hello() {
        let runtime = running(RuntimeStatus::Ready, &"b".repeat(64), None);
        let installed = fixture::installed(true);
        let status = present(
            Some(&installed),
            Some(&runtime),
            Some(Phase::Disconnected),
            false,
        );
        assert_eq!(status.label, "Offline");
        assert!(!status.active);
        let approval = present(
            Some(&installed),
            Some(&runtime),
            Some(Phase::AwaitingApproval),
            false,
        );
        assert_eq!(approval.label, "Waiting for SSH approval");
        assert!(!approval.active);
    }

    #[test]
    fn unknown_check_failure_never_promises_an_automatic_update() {
        let installed = InstallationStatus {
            update_available: Some(true),
            error: Some("failed".into()),
            ..fixture::installed(true)
        };
        let status = present(Some(&installed), None, None, false);
        assert_eq!(status.label, "Couldn’t check");
        assert_eq!(status.action, Some((Action::Inspect, "Retry")));
    }

    #[test]
    fn signed_default_version_does_not_label_another_running_build() {
        let installed = InstallationStatus {
            installed_hash: Some("a".repeat(64)),
            installed_version: Some("0.7.0".into()),
            bundled_hash: Some("b".repeat(64)),
            ..fixture::installation()
        };
        assert_eq!(release(Some(&installed), Some(&"b".repeat(64))), None);
        assert_eq!(
            release(Some(&installed), Some(&"a".repeat(64))),
            Some("0.7.0")
        );
    }

    #[test]
    fn a_known_old_protocol_offers_explicit_installation_despite_unknown_product() {
        let installed = InstallationStatus {
            build_relation: BuildRelation::Older,
            update_available: Some(true),
            error: Some("product unknown".into()),
            ..fixture::installed(true)
        };
        let runtime = fixture::agent_runtime(RuntimeStatus::Failed, None);
        let status = present(Some(&installed), Some(&runtime), None, false);
        assert_eq!(status.label, "Update required");
        assert_eq!(status.action, Some((Action::Update, "Update")));
    }
}
