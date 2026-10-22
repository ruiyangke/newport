//! Needle on a server: install (also update) and uninstall. Installing again
//! converges and uninstalling is final; nothing is journaled in between.
use crate::agent_payload::payload;
use crate::ssh::ExecSession;
use serde::Serialize;
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
static BUNDLED_RELEASE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/needle-release.json"));

#[cfg(test)]
#[path = "agent_release_tests.rs"]
mod release_tests;

pub fn payload_size(arch: &str) -> Option<usize> {
    payload(arch).map(|payload| payload.size)
}
/// Validate packaged resources without contacting a server or accessing profiles.
pub fn check_resources() -> Result<(), String> {
    crate::agent_payload::check_resources()
}
/// The outcome of installing Needle.
pub struct Deployment {
    pub output: String,
    pub build_sha256: String,
    /// Shell setup the person still has to do by hand.
    pub setup_notice: Option<String>,
}
/// Installs or updates Needle on the server: this Newport's build, verified,
/// then made active; with `integrations`, also the command links and shell
/// setup.
pub async fn manage(
    session: &ExecSession,
    progress: &crate::agent_install::Attempt,
    integrations: bool,
    trigger: Trigger,
) -> Result<Deployment, String> {
    if !session.server().agent_install_approved {
        return Err(
            "Needle installation requires approval. Open Overview to set up this server.".into(),
        );
    }
    let platform = session.execute("uname -s; uname -m", None).await?;
    let mut lines = platform.lines();
    if lines.next() != Some("Linux") {
        return Err("The Newport agent requires Linux.".into());
    }
    let architecture = lines.next().unwrap_or_default();
    let binary = payload(architecture)
        .ok_or("The Newport agent supports x86_64 and ARM64 Linux servers.")?;
    let hash = binary.sha256.to_owned();
    let status = check(session).await?;
    if !trigger.allows(&status) {
        return Err(
            "Needle changed since the automatic update check; leaving it unchanged.".into(),
        );
    }
    require_ordered_replacement(&status)?;
    let candidate = format!("\"$HOME/.local/share/newport/needle/builds/{hash}/newport-agent\"");
    let staged = session
        .execute(
            &format!("if test -x {candidate}; then printf yes; fi"),
            None,
        )
        .await?
        == "yes";
    if !staged {
        let command = format!(
            "sh -c '{}' newport-install {hash}",
            include_str!("agent-install.sh").replace('\'', "'\"'\"'")
        );
        progress.update("uploading", 0, binary.size, None);
        session
            .upload_resource(&command, binary.stream()?, &|sent| {
                progress.update(
                    if sent == binary.size {
                        "installing"
                    } else {
                        "uploading"
                    },
                    sent,
                    binary.size,
                    None,
                )
            })
            .await?;
    }
    progress.update("verifying", 0, 0, None);
    crate::agent_runtime::verify_candidate(session, &hash).await?;
    if !BUNDLED_RELEASE.is_empty() {
        record_release(
            session,
            &hash,
            architecture,
            BUNDLED_RELEASE,
            newport_release::needle::verify_production,
        )
        .await?;
    }
    progress.update("activating", 0, 0, None);
    #[derive(serde::Deserialize)]
    struct Installed {
        build: String,
        notice: Option<String>,
    }
    let installed: Installed = serde_json::from_str(
        &session
            .execute(
                &format!(
                    "{candidate} deployment install {hash}{}",
                    if integrations { " --integrations" } else { "" }
                ),
                None,
            )
            .await?,
    )
    .map_err(|e| format!("Invalid Needle install result: {e}"))?;
    if installed.build != hash {
        return Err("Needle didn't confirm the installed build".into());
    }
    Ok(Deployment {
        output: "Needle installed and verified.".into(),
        build_sha256: hash,
        setup_notice: installed.notice,
    })
}

/// The shell setup left to do by hand, from an install's outcome.
pub(crate) fn setup_notice(outcome: &str) -> Option<String> {
    outcome
        .find("Manual shell setup needed")
        .map(|start| outcome[start..].to_owned())
}

/// Uninstalls Needle: stops it, takes out its command links and shell setup,
/// and deletes everything it keeps. Final; running it again finishes an
/// interrupted run. Needs no Needle executable on the server.
pub(crate) async fn uninstall(session: &ExecSession) -> Result<(), String> {
    let output = session
        .execute(
            &format!(
                "sh -c {}",
                crate::ssh::shell_quote(include_str!("agent-uninstall.sh"))
            ),
            None,
        )
        .await?;
    if output.trim() != "uninstalled" {
        return Err("Needle didn't confirm it was uninstalled".into());
    }
    Ok(())
}
/// Different remains unordered unless both identities have trusted release metadata.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BuildRelation {
    #[default]
    Unknown,
    Missing,
    Bundled,
    Different,
    Older,
    Newer,
    SameRelease,
}
#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// None means discovery has not succeeded; false means verified missing.
    pub installed: Option<bool>,
    /// None until release metadata can establish ordering for a different build.
    pub update_available: Option<bool>,
    pub build_relation: BuildRelation,
    pub installed_hash: Option<String>,
    pub installed_version: Option<String>,
    pub bundled_version: Option<String>,
    pub checked_at: u64,
    pub error: Option<String>,
}
impl Status {
    /// Needle follows this Newport: an installed Needle that isn't the
    /// bundled build is replaced, without asking, wherever installing was
    /// approved. Only a release known to be newer stays, as replacing it would
    /// go back; Newport is the one to update then.
    pub(crate) fn automatic_update_available(&self) -> bool {
        self.installed == Some(true)
            && self.error.is_none()
            && matches!(
                self.build_relation,
                BuildRelation::Older | BuildRelation::Different | BuildRelation::SameRelease
            )
    }
}

/// Who started a Needle deployment, which decides what it may replace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    /// The person chose to install, reinstall or update.
    Manual,
    /// A newer signed release is bundled.
    Automatic,
    /// A feature found the installed Needle too old to answer it.
    Required,
}
impl Trigger {
    pub fn allows(self, status: &Status) -> bool {
        match self {
            Self::Manual => true,
            Self::Automatic => status.automatic_update_available(),
            Self::Required => status.automatic_update_available(),
        }
    }
}

/// A required update's progress, so the feature waiting on it can say so.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum RequiredUpdate {
    Running,
    Failed { error: String, at: Instant },
}
#[derive(Default)]
struct Observation {
    status: Status,
    probe: Option<(u64, Instant)>,
}
/// One instance per ServerSession. Network operations are serialized independently
/// of snapshot reads and of other servers. No global status or progress maps.
#[derive(Default)]
pub struct State {
    pub(crate) tasks: crate::lifecycle_task::Slot,
    pub(crate) operation: tokio::sync::Mutex<()>,
    pub progress: Arc<crate::agent_install::State>,
    observation: Mutex<Observation>,
    automatic_attempt: Mutex<Option<Instant>>,
    required: Mutex<Option<RequiredUpdate>>,
}
impl State {
    pub fn snapshot(&self) -> Status {
        self.observation.lock().unwrap().status.clone()
    }
    pub(crate) fn due(&self, generation: u64, now: Instant) -> bool {
        let observation = self.observation.lock().unwrap();
        observation.probe.is_none_or(|(seen, at)| {
            seen != generation || now.saturating_duration_since(at) >= Duration::from_secs(3600)
        })
    }
    pub(crate) fn record(&self, generation: u64, result: &Result<Status, String>) {
        let mut observation = self.observation.lock().unwrap();
        observation.probe = Some((generation, Instant::now()));
        match result {
            Ok(status) => observation.status = status.clone(),
            // Preserve last verified evidence; an unreachable server is not missing.
            Err(error) => observation.status.error = Some(error.clone()),
        }
    }
    /// A feature found Needle missing: every page hears it now, not at the
    /// next check.
    pub(crate) fn found_missing(&self, generation: u64) {
        let mut observation = self.observation.lock().unwrap();
        observation.probe = Some((generation, Instant::now()));
        observation.status = Status {
            installed: Some(false),
            update_available: Some(false),
            build_relation: BuildRelation::Missing,
            bundled_version: observation.status.bundled_version.take(),
            checked_at: crate::metrics_db::now_ms() as u64,
            ..Status::default()
        };
    }
    /// A reconnect must not repeatedly start upgrades.
    pub(crate) fn claim_automatic_update(&self, now: Instant) -> bool {
        let mut previous = self.automatic_attempt.lock().unwrap();
        if previous.is_some_and(|at| now.saturating_duration_since(at) < Duration::from_secs(3600))
        {
            return false;
        }
        *previous = Some(now);
        true
    }
    /// Claims a required update unless one is running, or failed less than
    /// ten minutes ago (then that failure is returned to report).
    pub(crate) fn claim_required_update(&self, now: Instant) -> Result<(), RequiredUpdate> {
        let mut required = self.required.lock().unwrap();
        match &*required {
            Some(RequiredUpdate::Running) => return Err(RequiredUpdate::Running),
            Some(failed @ RequiredUpdate::Failed { at, .. })
                if now.saturating_duration_since(*at) < Duration::from_secs(600) =>
            {
                return Err(failed.clone());
            }
            _ => {}
        }
        *required = Some(RequiredUpdate::Running);
        Ok(())
    }
    pub(crate) fn required_update(&self) -> Option<RequiredUpdate> {
        self.required.lock().unwrap().clone()
    }
    pub(crate) fn finish_required_update(&self, result: &Result<String, String>) {
        *self.required.lock().unwrap() =
            result.as_ref().err().map(|error| RequiredUpdate::Failed {
                error: error.clone(),
                at: Instant::now(),
            });
    }
    pub(crate) fn automatic_update_unblocked(&self, server: &crate::model::Server) -> bool {
        server.agent_install_approved && self.tasks.active().is_none()
    }
}
fn parse_check(output: &str) -> Result<Status, String> {
    parse_release_check(
        output,
        BUNDLED_RELEASE,
        newport_release::needle::verify_production,
    )
}
fn parse_release_check(
    output: &str,
    bundled_metadata: &[u8],
    verify: impl Fn(&[u8]) -> Result<newport_release::needle::VerifiedRelease, String>,
) -> Result<Status, String> {
    parse_candidate_check(output, bundled_metadata, None, verify)
}

fn parse_candidate_check(
    output: &str,
    bundled_metadata: &[u8],
    candidate_hash: Option<&str>,
    verify: impl Fn(&[u8]) -> Result<newport_release::needle::VerifiedRelease, String>,
) -> Result<Status, String> {
    let mut lines = output.splitn(4, '\n');
    if lines.next() != Some("Linux") {
        return Err("Needle requires Linux.".into());
    }
    let architecture = lines.next().unwrap_or_default();
    let binary = payload(architecture).ok_or("Unsupported server architecture.")?;
    let installed_hash = lines
        .next()
        .ok_or("Incomplete Needle inspection response.")?
        .split_whitespace()
        .next()
        .unwrap_or_default();
    let missing = installed_hash == "missing";
    if !missing
        && (installed_hash.len() != 64 || !installed_hash.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err("Could not read the installed Needle checksum.".into());
    }
    let expected = candidate_hash.unwrap_or(binary.sha256).to_owned();
    let mut relation = if missing {
        BuildRelation::Missing
    } else if installed_hash.eq_ignore_ascii_case(&expected) {
        BuildRelation::Bundled
    } else {
        BuildRelation::Different
    };
    let installed_hash = (!missing).then(|| installed_hash.to_ascii_lowercase());
    let metadata = lines.next().unwrap_or_default();
    let installed = if metadata.is_empty() {
        None
    } else {
        Some(verify(metadata.as_bytes())?)
    };
    let bundled = if bundled_metadata.is_empty() {
        None
    } else {
        Some(verify(bundled_metadata)?)
    };
    let installed_version = match (&installed, &installed_hash) {
        (Some(release), Some(hash)) => Some(
            release
                .version_for(architecture, hash)
                .ok_or("Needle release metadata does not match the installed executable")?
                .to_string(),
        ),
        (Some(_), None) => return Err("Missing Needle has unexpected release metadata".into()),
        _ => None,
    };
    let bundled_version = bundled
        .as_ref()
        .map(|release| {
            release
                .version_for(architecture, &expected)
                .map(ToString::to_string)
                .ok_or("Needle release metadata does not match the bundled executable")
        })
        .transpose()?;
    if relation == BuildRelation::Different
        && let (Some(installed), Some(bundled), Some(hash)) =
            (&installed, &bundled, &installed_hash)
    {
        relation = match installed.compare_builds(hash, bundled, &expected, architecture) {
            Some(std::cmp::Ordering::Less) => BuildRelation::Older,
            Some(std::cmp::Ordering::Equal) => BuildRelation::SameRelease,
            Some(std::cmp::Ordering::Greater) => BuildRelation::Newer,
            None => BuildRelation::Different,
        };
    }
    Ok(Status {
        installed: Some(!missing),
        update_available: match relation {
            BuildRelation::Missing
            | BuildRelation::Bundled
            | BuildRelation::Newer
            | BuildRelation::SameRelease => Some(false),
            BuildRelation::Older => Some(true),
            _ => None,
        },
        build_relation: relation,
        installed_hash,
        installed_version,
        bundled_version,
        checked_at: crate::metrics_db::now_ms() as u64,
        error: None,
    })
}
// A failed hash or an unexpected/truncated response must never be read as absence.
pub(crate) const CHECK: &str = include_str!("agent-release-check.sh");
pub(crate) async fn check(session: &ExecSession) -> Result<Status, String> {
    parse_check(&session.execute(CHECK, None).await?)
}
/// Observe and upgrade only existing sessions. This never connects a disconnected
/// profile. Connection establishment wakes this worker immediately.
pub async fn run(state: crate::manager::Shared) {
    let registry = state.sessions.clone();
    let mut timer = tokio::time::interval(Duration::from_secs(60));
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = registry.connected.notified() => {},
            _ = timer.tick() => {},
        }
        let mut checks = tokio::task::JoinSet::new();
        for owner in registry.all() {
            let Some(lease) = owner.connected_lease().await else {
                continue;
            };
            if !owner.lifecycle.due(lease.generation(), Instant::now())
                && !owner.lifecycle.snapshot().automatic_update_available()
            {
                continue;
            }
            if checks.len() >= 4 {
                let _ = checks.join_next().await;
            }
            checks.spawn(async move {
                if let Ok(status) = lease.check_installation_if_due().await
                    && let Err(error) = lease.automatically_update_needle(&status).await
                {
                    log::warn!("Automatic Needle update did not complete: {error}");
                }
            });
        }
        while checks.join_next().await.is_some() {}
    }
}

// Disposable remote test setup uses the same installer with explicitly owned
// state. Production callers must go through ServerSession's lifecycle lease.
#[cfg(test)]
pub(crate) async fn manage_test(
    session: &ExecSession,
    _force: bool,
    integrations: bool,
) -> Result<String, String> {
    let state = State::default();
    let progress = state.progress.attempt();
    let result = manage(session, &progress, integrations, Trigger::Manual)
        .await
        .map(|d| d.output);
    progress.finish(&result);
    result
}
/// Disposable Linux fixtures use an executable-equivalent ELF with a distinct
/// digest to exercise a real build transition without publishing another release.
#[cfg(test)]
pub(crate) async fn activate_variant_test(session: &ExecSession) -> String {
    session
        .execute(
            r#"set -e
t=$(mktemp)
cat "$HOME/.local/bin/newport-agent" > "$t"
printf '\000fixture-older-build' >> "$t"
h=$(sha256sum "$t"); h=${h%% *}
d="$HOME/.local/share/newport/needle/builds/$h"
mkdir -m 700 -p "$d"
mv "$t" "$d/newport-agent"
chmod 500 "$d/newport-agent"
"$d/newport-agent" deployment install "$h" >/dev/null
printf '%s' "$h"
"#,
            None,
        )
        .await
        .unwrap()
}
fn require_ordered_replacement(status: &Status) -> Result<(), String> {
    if status.build_relation == BuildRelation::Newer {
        return Err(
            "A newer Needle release is installed. Update Newport before replacing it.".into(),
        );
    }
    Ok(())
}

async fn record_release(
    session: &ExecSession,
    hash: &str,
    architecture: &str,
    metadata: &[u8],
    verify: impl Fn(&[u8]) -> Result<newport_release::needle::VerifiedRelease, String>,
) -> Result<(), String> {
    verify(metadata)?
        .version_for(architecture, hash)
        .ok_or("Release metadata does not identify the candidate")?;
    let command = format!(
        "sh -c '{}' newport-release {hash}",
        include_str!("agent-release-record.sh").replace('\'', "'\"'\"'")
    );
    session.execute(&command, Some(metadata)).await?;
    let stored = read_release(session, hash).await?;
    verify(stored.as_bytes())?
        .version_for(architecture, hash)
        .ok_or("Stored release metadata does not identify the candidate")?;
    Ok(())
}

async fn read_release(session: &ExecSession, hash: &str) -> Result<String, String> {
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("Invalid Needle build identity".into());
    }
    session.execute(&format!(
        "set -e; p=\"$HOME/.local/share/newport/needle/builds/{hash}/needle-release.json\"; if test -e \"$p\" || test -L \"$p\"; then test -f \"$p\" && ! test -L \"$p\"; head -c 8193 \"$p\"; fi"
    ), None).await
}

/// Explicit rollback of one unfinished, connection-scoped deployment. No upload or install.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needle_is_replaced_automatically_unless_a_newer_release_is_installed() {
        let status = |relation, version: Option<&str>| Status {
            installed: Some(true),
            build_relation: relation,
            installed_version: version.map(str::to_owned),
            ..Status::default()
        };
        for trigger in [Trigger::Automatic, Trigger::Required] {
            // An older release, another build of this release, a build without
            // release order (development, or this Newport has none).
            assert!(trigger.allows(&status(BuildRelation::Older, Some("1.0.0"))));
            assert!(trigger.allows(&status(BuildRelation::SameRelease, Some("1.0.0"))));
            assert!(trigger.allows(&status(BuildRelation::Different, None)));
            assert!(trigger.allows(&status(BuildRelation::Different, Some("1.0.0"))));
            // Never a newer release, the bundled build, or an uninstalled one.
            assert!(!trigger.allows(&status(BuildRelation::Newer, Some("9.0.0"))));
            assert!(!trigger.allows(&status(BuildRelation::Bundled, None)));
            assert!(!trigger.allows(&Status {
                installed: Some(false),
                ..status(BuildRelation::Missing, None)
            }));
        }
    }

    #[test]
    fn required_updates_run_once_and_back_off_after_failing() {
        let state = State::default();
        let now = Instant::now();
        assert_eq!(state.claim_required_update(now), Ok(()));
        assert_eq!(
            state.claim_required_update(now),
            Err(RequiredUpdate::Running)
        );
        state.finish_required_update(&Err("upload failed".into()));
        assert!(matches!(
            state.claim_required_update(now),
            Err(RequiredUpdate::Failed { error, .. }) if error == "upload failed"
        ));
        assert_eq!(
            state.claim_required_update(now + Duration::from_secs(601)),
            Ok(())
        );
        state.finish_required_update(&Ok("Needle installed and verified.".into()));
        assert_eq!(state.required_update(), None);
    }
    #[tokio::test]
    #[ignore = "Requires the disposable Linux OpenSSH fixture"]
    async fn a_different_needle_is_replaced_with_newports_own_without_asking() {
        let server: crate::model::Server = serde_json::from_value(serde_json::json!({
            "id": uuid::Uuid::new_v4(), "name": "Automatic update fixture", "sshUser": "fixture",
            "sshHost": "127.0.0.1", "sshPort": std::env::var("NEWPORT_TEST_SSH_PORT").unwrap().parse::<u16>().unwrap(),
            "identityFile": std::env::var("NEWPORT_TEST_IDENTITY").unwrap(), "agentInstallApproved": true
        })).unwrap();
        let registry = crate::server_session::Registry::default();
        let owner = registry.session(&server, 0);
        let lease = owner.acquire(&server).await.unwrap();
        lease.install(false).await.unwrap();
        let bundled = lease
            .check_installation()
            .await
            .unwrap()
            .installed_hash
            .unwrap();
        // Another build at the entry point, as an earlier Newport or another
        // developer's left it: Needle's own bytes plus one, so it still runs.
        lease
            .execute(
                "set -e; d=\"$HOME/.local/share/newport/needle/builds\"; t=$(mktemp); cat \"$(readlink -f \"$HOME/.local/bin/newport-agent\")\" > \"$t\"; printf x >> \"$t\"; h=$(sha256sum \"$t\" | cut -d' ' -f1); mkdir -m 700 \"$d/$h\"; mv \"$t\" \"$d/$h/newport-agent\"; chmod 500 \"$d/$h/newport-agent\"; ln -sfn \"$d/$h/newport-agent\" \"$HOME/.local/bin/newport-agent\"",
                None,
            )
            .await
            .unwrap();
        let other = lease.check_installation().await.unwrap();
        assert_ne!(other.installed_hash.as_deref(), Some(bundled.as_str()));
        assert!(
            other.automatic_update_available(),
            "{:?}",
            other.build_relation
        );
        lease.automatically_update_needle(&other).await.unwrap();
        assert_eq!(
            lease.check_installation().await.unwrap().installed_hash,
            Some(bundled)
        );
        registry.shutdown().await;
    }
    #[tokio::test]
    #[ignore = "Requires the disposable Linux OpenSSH fixture"]
    async fn staged_activation_verifies_before_switch_and_preserves_live_runtime() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let server: crate::model::Server = serde_json::from_value(serde_json::json!({
            "id": uuid::Uuid::new_v4(), "name": "Activation fixture", "sshUser": "fixture",
            "sshHost": "127.0.0.1", "sshPort": std::env::var("NEWPORT_TEST_SSH_PORT").unwrap().parse::<u16>().unwrap(),
            "identityFile": std::env::var("NEWPORT_TEST_IDENTITY").unwrap(), "agentInstallApproved": true
        })).unwrap();
        let registry = crate::server_session::Registry::default();
        let owner = registry.session(&server, 0);
        let lease = owner.acquire(&server).await.unwrap();
        lease.install(false).await.unwrap();
        let status = lease.check_installation().await.unwrap();
        assert_eq!(status.build_relation, BuildRelation::Bundled);
        let hash = status.installed_hash.unwrap();
        let link = lease
            .execute("readlink \"$HOME/.local/bin/newport-agent\"", None)
            .await
            .unwrap();
        assert!(
            link.trim()
                .ends_with(&format!("/builds/{hash}/newport-agent"))
        );
        let worker = owner.acquire(&server).await.unwrap();
        let (stream, _) = worker.launch(crate::agent::Service::Git).await.unwrap();
        let runtime = owner.agent_runtime.snapshot().hello.unwrap().instance_id;
        lease.install(false).await.unwrap();
        assert!(
            !worker.is_closed(),
            "explicit activation must not cancel admitted Git work"
        );
        assert_eq!(
            owner.agent_runtime.snapshot().hello.unwrap().instance_id,
            runtime
        );
        // A candidate that can execute but cannot complete the runtime handshake
        // must fail verification while the verified entry point remains usable.
        let broken = lease.execute("set -e; d=\"$HOME/.local/share/newport/needle/builds\"; t=$(mktemp); printf '#!/bin/sh\\nexit 1\\n' > \"$t\"; h=$(sha256sum \"$t\"); h=${h%% *}; mkdir -m 700 -p \"$d/$h\"; mv \"$t\" \"$d/$h/newport-agent\"; chmod 500 \"$d/$h/newport-agent\"; printf '%s' \"$h\"", None).await.unwrap();
        let connection = ExecSession::connect(&server).await.unwrap();
        assert!(
            crate::agent_runtime::verify_candidate(&connection, &broken)
                .await
                .is_err()
        );
        assert!(
            owner
                .agent_runtime
                .adopt(
                    &connection,
                    owner.integration_session_id(),
                    1,
                    &broken,
                    || Ok(())
                )
                .await
                .is_err()
        );
        assert_eq!(
            owner.agent_runtime.snapshot().hello.unwrap().instance_id,
            runtime
        );
        assert!(!worker.is_draining());
        assert_eq!(
            check(&connection).await.unwrap().installed_hash.as_deref(),
            Some(hash.as_str())
        );
        // An occupied `newport-browser` fails the install after Needle itself
        // is in place; someone else's file is never touched. Point the entry
        // point at another build first, so the switch is visible.
        lease.execute("printf 'unrelated' > \"$HOME/.local/bin/newport-browser\"; builds=\"$HOME/.local/share/newport/needle/builds\"; stub=$(mktemp); printf '#!/bin/sh\\necho newport-agent\\n' > \"$stub\"; other=\"$builds/$(sha256sum \"$stub\" | cut -d' ' -f1)\"; mkdir -m 700 \"$other\"; mv \"$stub\" \"$other/newport-agent\"; chmod 700 \"$other/newport-agent\"; ln -sfn \"$other/newport-agent\" \"$HOME/.local/bin/newport-agent\"", None).await.unwrap();
        let other = check(&connection).await.unwrap().installed_hash.unwrap();
        assert_ne!(other, hash);
        let error = lease.install(true).await.unwrap_err();
        assert!(
            error.contains("occupied by an unrelated command"),
            "{error}"
        );
        assert_eq!(
            check(&connection).await.unwrap().installed_hash.as_deref(),
            Some(hash.as_str())
        );
        assert_eq!(
            lease
                .execute("cat \"$HOME/.local/bin/newport-browser\"", None)
                .await
                .unwrap(),
            "unrelated"
        );
        assert!(!worker.is_closed());
        // New work on the surviving runtime launches its own build.
        let monitor = owner.acquire(&server).await.unwrap();
        let (mut channel, _) = monitor
            .launch(crate::agent::Service::Monitor)
            .await
            .unwrap();
        channel.write_all(b"{\"op\":\"snapshot\"}\n").await.unwrap();
        channel.shutdown().await.unwrap();
        let mut bytes = Vec::new();
        tokio::time::timeout(Duration::from_secs(10), channel.read_to_end(&mut bytes))
            .await
            .unwrap()
            .unwrap();
        let metrics: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(
            metrics.is_object() && metrics.get("error").is_none_or(serde_json::Value::is_null),
            "{metrics}"
        );
        drop(stream);
        connection.close().await;
        registry.shutdown().await;
    }
    #[test]
    fn checksum_difference_cannot_establish_release_ordering() {
        let missing = parse_check("Linux\nx86_64\nmissing\n").unwrap();
        assert_eq!(missing.installed, Some(false));
        assert_eq!(missing.build_relation, BuildRelation::Missing);
        let hash = payload("x86_64").unwrap().sha256;
        let current = parse_check(&format!("Linux\nx86_64\n{hash}  agent")).unwrap();
        assert_eq!(current.installed, Some(true));
        assert_eq!(current.update_available, Some(false));
        assert_eq!(current.build_relation, BuildRelation::Bundled);
        let different = parse_check(&format!("Linux\nx86_64\n{}  agent", "0".repeat(64))).unwrap();
        assert_eq!(different.installed, Some(true));
        assert_eq!(different.update_available, None);
        assert_eq!(different.build_relation, BuildRelation::Different);
        assert!(parse_check("Linux\nx86_64\nbroken").is_err());
        assert!(parse_check("Linux\nx86_64\n").is_err());
    }
    #[test]
    fn discovery_errors_preserve_unknown_or_last_verified_installation() {
        let state = State::default();
        assert!(state.due(1, Instant::now()));
        state.record(1, &Err("offline".into()));
        assert_eq!(state.snapshot().installed, None);
        assert!(!state.due(1, Instant::now()));
        assert!(state.due(2, Instant::now()));
        assert!(state.due(1, Instant::now() + Duration::from_secs(3601)));
        let known = parse_check(&format!("Linux\nx86_64\n{}  agent", "0".repeat(64))).unwrap();
        state.record(2, &Ok(known));
        let checked = state.snapshot().checked_at;
        state.record(2, &Err("timeout".into()));
        let retained = state.snapshot();
        assert_eq!(retained.installed, Some(true));
        assert_eq!(retained.checked_at, checked);
        assert_eq!(retained.error.as_deref(), Some("timeout"));
        assert_eq!(retained.build_relation, BuildRelation::Different);
    }
    #[test]
    fn a_feature_finding_needle_missing_tells_every_page_at_once() {
        let state = State::default();
        let known = parse_check(&format!("Linux\nx86_64\n{}  agent", "0".repeat(64))).unwrap();
        state.record(1, &Ok(known));
        state.found_missing(1);
        let missing = state.snapshot();
        assert_eq!(missing.installed, Some(false));
        assert_eq!(missing.build_relation, BuildRelation::Missing);
        assert_eq!(missing.installed_hash, None);
        assert!(
            !state.due(1, Instant::now()),
            "no check is needed to know it"
        );
    }
}
