//! Managed Needle lifecycle. Feature channels never deploy or upgrade binaries.
use crate::ssh::ExecSession;
#[path = "agent_helper_history.rs"]
pub(crate) mod helpers;
#[path = "agent_removal.rs"]
pub(crate) mod removal;
use crate::agent_payload::payload;
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
/// The only operation allowed to copy or replace Needle. Automatic upgrades use
/// the same verified, journaled deployment as explicit installation actions.
pub struct Deployment {
    pub output: String,
    pub build_sha256: String,
    pub already_complete: bool,
    pub setup_notice: Option<String>,
}
pub async fn manage(
    session: &ExecSession,
    progress: &crate::agent_install::Attempt,
    force: bool,
    integrations: bool,
    receipts: &crate::lifecycle_receipts::Store,
    operation_id: uuid::Uuid,
    trigger: Trigger,
) -> Result<Deployment, String> {
    deploy(
        session,
        progress,
        force,
        integrations,
        receipts,
        operation_id,
        trigger,
    )
    .await
}
/// Explicit, durable shell setup. Never uploads or replaces the installed binary.
pub async fn configure_integrations(
    session: &ExecSession,
    receipts: &crate::lifecycle_receipts::Store,
    id: uuid::Uuid,
) -> Result<String, String> {
    use crate::lifecycle_receipts::{
        Phase, Record,
        protocol::{Intent, Kind, Observation, State},
    };
    if id.is_nil() {
        return Err("Invalid shell setup operation ID".into());
    }
    if !session.server().agent_install_approved {
        return Err("Approve Needle setup before configuring shell helpers".into());
    }
    let connection = crate::lifecycle_receipts::scope(session.server());
    let prior = receipts.get(id)?;
    if let Some(record) = &prior {
        if record.server_id != session.server().id
            || record.connection != connection
            || record.intent.kind != Kind::Helpers
        {
            return Err(
                "Shell setup operation belongs to a different request or connection".into(),
            );
        }
        if record.phase == Phase::Ready {
            return Ok(record
                .error
                .clone()
                .unwrap_or_else(|| "Shell setup already completed.".into()));
        }
        if matches!(record.phase, Phase::RollingBack | Phase::RolledBack) {
            return Err("This shell setup was restored or is being restored; resolve it before starting again".into());
        }
    }
    let active = "\"$HOME/.local/bin/newport-agent\"";
    let record = match prior {
        Some(record) => record,
        None => {
            crate::agent::require_installed(session).await?;
            let hash = check(session)
                .await?
                .installed_hash
                .ok_or("Needle is not installed")?;
            helper_protocol(session, active, &hash).await?;
            receipts
                .begin(Record {
                    server_id: session.server().id,
                    connection,
                    intent: Intent {
                        kind: Kind::Helpers,
                        operation_id: id,
                        candidate: hash.clone(),
                        previous: hash,
                        integrations: true,
                    },
                    phase: Phase::Preparing,
                    error: None,
                })
                .await?
        }
    };
    let hash = &record.intent.candidate;
    let pinned = format!("\"$HOME/.local/share/newport/needle/builds/{hash}/newport-agent\"");
    let retained = session
        .execute(&format!("if test -x {pinned}; then printf yes; fi"), None)
        .await?
        == "yes";
    let command = if retained { pinned.as_str() } else { active };
    helper_protocol(session, command, hash).await?;
    let mut observed: Observation = if record.phase == Phase::NeedsResolution {
        serde_json::from_str(
            &session
                .execute(&format!("{command} deployment operation {id}"), None)
                .await?,
        )
        .map_err(|e| format!("Invalid shell setup receipt: {e}"))?
    } else {
        receipts.update(id, Some(Phase::Activating), None).await?;
        serde_json::from_str(
            &session
                .execute(
                    &format!("{command} deployment helpers {hash} --operation {id}"),
                    None,
                )
                .await?,
        )
        .map_err(|e| format!("Invalid shell setup result: {e}"))?
    };
    if observed
        .receipt
        .as_ref()
        .is_some_and(|r| r.intent != record.intent)
    {
        return Err("Shell setup receipt does not match the approved request".into());
    }
    if observed
        .receipt
        .as_ref()
        .is_some_and(|r| matches!(r.state, State::Prepared | State::Activated))
    {
        observed = serde_json::from_str(
            &session
                .execute(&format!("{command} deployment reconcile {id}"), None)
                .await?,
        )
        .map_err(|e| format!("Invalid reconciled shell setup receipt: {e}"))?;
    }
    let remote = observed
        .receipt
        .ok_or("Shell setup receipt is missing; review the operation before proceeding")?;
    if remote.intent != record.intent {
        return Err("Shell setup receipt does not match the approved request".into());
    }
    if remote.state != State::Succeeded {
        let error = remote.error.unwrap_or_else(|| {
            "Shell setup needs review; its helper changes are not confirmed".into()
        });
        receipts
            .update(
                id,
                Some(if remote.state == State::RolledBack {
                    Phase::RolledBack
                } else {
                    Phase::NeedsResolution
                }),
                Some(error.clone()),
            )
            .await?;
        return Err(error);
    }
    let notice = remote.error;
    receipts
        .update(id, Some(Phase::Ready), notice.clone())
        .await?;
    Ok(notice.unwrap_or_else(|| "Shell setup completed.".into()))
}
async fn helper_protocol(
    session: &ExecSession,
    command: &str,
    hash: &str,
) -> Result<serde_json::Value, String> {
    let output = session.execute(&format!("{command} deployment protocol"), None).await
        .map_err(|_| "This Needle build cannot manage shell setup safely. Update Needle explicitly, then retry.".to_owned())?;
    let value: serde_json::Value = serde_json::from_str(&output).map_err(|_| {
        "Needle returned an unsupported lifecycle protocol; update Needle explicitly".to_owned()
    })?;
    if value["version"] != 2 || value["helperOperations"] != true || value["buildSha256"] != hash {
        return Err("Needle lifecycle compatibility or executable identity changed; inspect or update Needle before retrying".into());
    }
    Ok(value)
}

pub(crate) async fn review_helpers(
    session: &ExecSession,
    receipts: &crate::lifecycle_receipts::Store,
    id: uuid::Uuid,
    keep: Option<(usize, &str)>,
) -> Result<crate::lifecycle_receipts::protocol::HelperReview, String> {
    use crate::lifecycle_receipts::{
        Phase,
        protocol::{HelperReview, Observation},
    };
    let Some(record) = receipts.get(id)? else {
        if receipts.helper_recovery(session.server(), id)?.is_some() {
            return helpers::recovery::review(session, receipts, id, keep).await;
        }
        return removal::review(session, receipts, id, keep).await;
    };
    if record.server_id != session.server().id
        || record.connection != crate::lifecycle_receipts::scope(session.server())
        || record.phase != Phase::RollingBack
    {
        return Err("Only this connection's pending restore can be reviewed".into());
    }
    if let Some((index, fingerprint)) = keep
        && (index >= 32 || !crate::agent_runtime::protocol::valid_build(fingerprint))
    {
        return Err("Invalid file review decision".into());
    }
    let hash = &record.intent.candidate;
    let command = format!("\"$HOME/.local/share/newport/needle/builds/{hash}/newport-agent\"");
    let protocol = helper_protocol(session, &command, hash).await?;
    if protocol["helperReview"] != true {
        return Err("This recorded Needle build does not support file review. Its recovery evidence has been preserved.".into());
    }
    let observed: Observation = serde_json::from_str(
        &session
            .execute(&format!("{command} deployment operation {id}"), None)
            .await?,
    )
    .map_err(|e| e.to_string())?;
    if observed.receipt.is_none_or(|r| r.intent != record.intent) {
        return Err("Remote operation does not match this restore".into());
    }
    let args = match keep {
        None => format!("review {id}"),
        Some((index, fingerprint)) => format!("keep {id} {index} {fingerprint}"),
    };
    let report: HelperReview = serde_json::from_str(
        &session
            .execute(&format!("{command} deployment {args}"), None)
            .await?,
    )
    .map_err(|e| e.to_string())?;
    if report.operation_id != id || report.files.len() > 32 {
        return Err("Invalid helper review response".into());
    }
    Ok(report)
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
    pub(crate) fn automatic_update_available(&self) -> bool {
        self.installed == Some(true)
            && self.update_available == Some(true)
            && self.build_relation == BuildRelation::Older
            && self.error.is_none()
    }
    /// The installed Needle lacks something this desktop needs. It may be
    /// replaced wherever a manual install could: an older release, or a build
    /// without release metadata. Never a newer or unordered signed release.
    pub(crate) fn required_update_available(&self) -> bool {
        self.installed == Some(true)
            && self.error.is_none()
            && match self.build_relation {
                BuildRelation::Older => true,
                BuildRelation::Different => self.installed_version.is_none(),
                _ => false,
            }
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
            Self::Required => status.required_update_available(),
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
    pub receipts: Arc<crate::lifecycle_receipts::Store>,
    pub(crate) operation: tokio::sync::Mutex<()>,
    pub progress: Arc<crate::agent_install::State>,
    pub(crate) helper_history: helpers::State,
    observation: Mutex<Observation>,
    automatic_attempt: Mutex<Option<Instant>>,
    required: Mutex<Option<RequiredUpdate>>,
}
impl State {
    pub fn new(receipts: Arc<crate::lifecycle_receipts::Store>) -> Self {
        Self {
            receipts,
            ..Self::default()
        }
    }
    pub fn snapshot(&self) -> Status {
        self.observation.lock().unwrap().status.clone()
    }
    pub(crate) fn due(&self, generation: u64, now: Instant) -> bool {
        let observation = self.observation.lock().unwrap();
        observation.probe.is_none_or(|(seen, at)| {
            seen != generation || now.saturating_duration_since(at) >= Duration::from_secs(3600)
        }) || (observation.status.installed_hash.is_some() && self.helper_history.needs_check())
    }
    pub(crate) fn record(&self, generation: u64, result: &Result<Status, String>) {
        let mut observation = self.observation.lock().unwrap();
        observation.probe = Some((generation, Instant::now()));
        match result {
            Ok(status) => {
                if observation.status.installed_hash != status.installed_hash {
                    self.helper_history.invalidate();
                }
                observation.status = status.clone();
            }
            // Preserve last verified evidence; an unreachable server is not missing.
            Err(error) => observation.status.error = Some(error.clone()),
        }
    }
    /// A reconnect or helper-history retry must not repeatedly start upgrades.
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
    pub(crate) fn automatic_update_unblocked(
        &self,
        server: &crate::model::Server,
    ) -> Result<bool, String> {
        if !server.agent_install_approved || self.tasks.active().is_some() {
            return Ok(false);
        }
        self.automatic_update_journal_clear(server)
    }
    pub(crate) fn automatic_update_journal_clear(
        &self,
        server: &crate::model::Server,
    ) -> Result<bool, String> {
        use crate::lifecycle_receipts::{HelperRecoveryPhase, RemovalPhase};
        if self.receipts.require_agent_available(server).is_err() {
            return Ok(false);
        }
        let records = self.receipts.operations_snapshot(server)?;
        Ok(records.operations.iter().all(|r| r.phase.terminal())
            && records
                .removals
                .iter()
                .all(|r| r.phase == RemovalPhase::Restored)
            && records
                .helper_recoveries
                .iter()
                .all(|r| r.phase == HelperRecoveryPhase::Restored))
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
    force: bool,
    integrations: bool,
) -> Result<String, String> {
    let state = State::default();
    let progress = state.progress.attempt();
    let result = manage(
        session,
        &progress,
        force,
        integrations,
        &state.receipts,
        uuid::Uuid::new_v4(),
        Trigger::Manual,
    )
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
old=$(sha256sum "$HOME/.local/bin/newport-agent"); old=${old%% *}
t=$(mktemp)
cat "$HOME/.local/bin/newport-agent" > "$t"
printf '\000fixture-older-build' >> "$t"
h=$(sha256sum "$t"); h=${h%% *}
d="$HOME/.local/share/newport/needle/builds/$h"
mkdir -m 700 -p "$d"
mv "$t" "$d/newport-agent"
chmod 500 "$d/newport-agent"
"$d/newport-agent" deployment activate "$h" "$old" >/dev/null
printf '%s' "$h"
"#,
            None,
        )
        .await
        .unwrap()
}
async fn deploy(
    session: &ExecSession,
    progress: &crate::agent_install::Attempt,
    force: bool,
    integrate: bool,
    receipts: &crate::lifecycle_receipts::Store,
    operation_id: uuid::Uuid,
    trigger: Trigger,
) -> Result<Deployment, String> {
    if !session.server().agent_install_approved {
        return Err(
            "Needle installation requires approval. Open Overview to set up this server.".into(),
        );
    }
    deploy_inner(
        session,
        force,
        integrate,
        progress,
        receipts,
        operation_id,
        trigger,
    )
    .await
}
async fn deploy_inner(
    session: &ExecSession,
    _force: bool,
    integrate: bool,
    progress: &crate::agent_install::Attempt,
    receipts: &crate::lifecycle_receipts::Store,
    operation_id: uuid::Uuid,
    trigger: Trigger,
) -> Result<Deployment, String> {
    use crate::lifecycle_receipts::{
        Phase, Record,
        protocol::{Intent, Observation, State as RemoteState},
    };
    if operation_id.is_nil() {
        return Err("Invalid Needle operation ID".into());
    }
    let server = session.server();
    let connection = crate::lifecycle_receipts::scope(server);
    let prior = receipts.get(operation_id)?;
    if let Some(record) = &prior {
        if record.server_id != server.id
            || record.connection != connection
            || !record.intent.kind.is_deployment()
            || record.intent.integrations != integrate
        {
            return Err("Needle operation ID belongs to a different request or connection".into());
        }
        if matches!(record.phase, Phase::RollingBack | Phase::RolledBack) {
            return Err("This Needle operation is being restored or was rolled back; finish recovery before a new installation".into());
        }
        if record.phase == Phase::Ready {
            return Ok(Deployment {
                output: "Needle operation already completed.".into(),
                build_sha256: record.intent.candidate.clone(),
                already_complete: true,
                setup_notice: record.error.clone(),
            });
        }
    }
    let platform = session.execute("uname -s; uname -m", None).await?;
    let mut lines = platform.lines();
    if lines.next() != Some("Linux") {
        return Err("The Newport agent requires Linux.".into());
    }
    let architecture = lines.next().unwrap_or_default();
    let binary = payload(architecture)
        .ok_or("The Newport agent supports x86_64 and ARM64 Linux servers.")?;
    let bundled = binary.sha256.to_owned();
    let record = if let Some(record) = prior {
        record
    } else {
        let status = check(session).await?;
        if !trigger.allows(&status) {
            return Err(
                "Needle changed since the automatic update check; leaving it unchanged.".into(),
            );
        }
        require_ordered_replacement(&status)?;
        let previous = status.installed_hash.unwrap_or_else(|| "missing".into());
        receipts
            .begin(Record {
                server_id: server.id,
                connection,
                intent: Intent {
                    kind: crate::lifecycle_receipts::protocol::Kind::Deployment,
                    operation_id,
                    candidate: bundled.clone(),
                    previous,
                    integrations: integrate,
                },
                phase: Phase::Preparing,
                error: None,
            })
            .await?
    };
    let hash = &record.intent.candidate;
    let candidate = format!("\"$HOME/.local/share/newport/needle/builds/{hash}/newport-agent\"");
    let staged = session
        .execute(
            &format!("if test -x {candidate}; then printf yes; fi"),
            None,
        )
        .await?
        == "yes";
    if !staged {
        if record.phase != Phase::Preparing && record.phase != Phase::Uploading {
            return Err(
                "Recorded Needle candidate is missing; inspect this operation before proceeding"
                    .into(),
            );
        }
        if *hash != bundled {
            return Err("The recorded Needle build is no longer bundled; resolve this operation before installing a different build".into());
        }
        receipts
            .update(operation_id, Some(Phase::Uploading), None)
            .await?;
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
    crate::agent_runtime::verify_candidate(session, hash).await?;
    if *hash == bundled && !BUNDLED_RELEASE.is_empty() {
        record_release(
            session,
            hash,
            architecture,
            BUNDLED_RELEASE,
            newport_release::needle::verify_production,
        )
        .await?;
    }
    let observed: Observation = serde_json::from_str(
        &session
            .execute(
                &format!("{candidate} deployment operation {operation_id}"),
                None,
            )
            .await?,
    )
    .map_err(|e| format!("Invalid Needle operation receipt: {e}"))?;
    let observed: Observation = if let Some(receipt) = observed.receipt.as_ref() {
        if receipt.intent != record.intent {
            return Err("Needle operation receipt does not match the approved request".into());
        }
        if matches!(
            receipt.state,
            RemoteState::Prepared | RemoteState::Activated
        ) {
            // Explicit retry reconciles the saved operation. No upload, helper
            // execution, or entry-point switch is repeated here.
            serde_json::from_str(
                &session
                    .execute(
                        &format!("{candidate} deployment reconcile {operation_id}"),
                        None,
                    )
                    .await?,
            )
            .map_err(|e| format!("Invalid reconciled Needle receipt: {e}"))?
        } else {
            observed
        }
    } else {
        if record.phase == Phase::Handoff || record.phase == Phase::NeedsResolution {
            return Err(
                "Needle activation receipt is missing; refusing to repeat an uncertain operation"
                    .into(),
            );
        }
        // A resumed operation may refer to a build from an older desktop.
        // Compare that actual candidate, not the desktop's current payload.
        let candidate_metadata = if *hash == bundled {
            BUNDLED_RELEASE.to_vec()
        } else {
            read_release(session, hash).await?.into_bytes()
        };
        require_ordered_replacement(&parse_candidate_check(
            &session.execute(CHECK, None).await?,
            &candidate_metadata,
            Some(hash),
            newport_release::needle::verify_production,
        )?)?;
        receipts
            .update(operation_id, Some(Phase::Activating), None)
            .await?;
        progress.update("activating", 0, 0, None);
        serde_json::from_str(
            &session
                .execute(
                    &format!(
                        "{candidate} deployment activate {hash} {} --operation {operation_id}{}",
                        record.intent.previous,
                        if integrate { " --integrations" } else { "" }
                    ),
                    None,
                )
                .await?,
        )
        .map_err(|e| format!("Invalid Needle activation receipt: {e}"))?
    };
    let remote = observed.receipt.ok_or("Needle activation has no receipt")?;
    if remote.intent != record.intent {
        return Err("Needle activation receipt does not match the approved request".into());
    }
    if remote.state != RemoteState::Succeeded || observed.active_build != *hash {
        let error = remote.error.unwrap_or_else(|| {
            "Needle activation requires review; the recorded build is not confirmed active".into()
        });
        receipts
            .update(
                operation_id,
                Some(if remote.state == RemoteState::RolledBack {
                    Phase::RolledBack
                } else {
                    Phase::NeedsResolution
                }),
                Some(error.clone()),
            )
            .await?;
        return Err(error);
    }
    receipts
        .update(operation_id, Some(Phase::Handoff), remote.error.clone())
        .await?;
    Ok(Deployment {
        output: "Needle installed and verified.".into(),
        build_sha256: hash.clone(),
        already_complete: false,
        setup_notice: remote.error,
    })
}

fn require_ordered_replacement(status: &Status) -> Result<(), String> {
    if status.build_relation == BuildRelation::Newer {
        return Err(
            "A newer Needle release is installed. Update Newport before replacing it.".into(),
        );
    }
    if status.build_relation == BuildRelation::Different && status.installed_version.is_some() {
        return Err("This Newport build has no verified release order. It cannot replace a signed Needle release.".into());
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
pub(crate) async fn recover(
    session: &ExecSession,
    receipts: &crate::lifecycle_receipts::Store,
    id: uuid::Uuid,
) -> Result<Option<String>, String> {
    use crate::lifecycle_receipts::{
        Phase,
        protocol::{Observation, State},
    };
    let record = receipts.get(id)?.ok_or("Needle operation not found")?;
    if record.server_id != session.server().id
        || record.connection != crate::lifecycle_receipts::scope(session.server())
    {
        return Err("Needle operation belongs to a different server connection".into());
    }
    if record.phase == Phase::Ready {
        return Err(
            "This installation is already complete; it is not an interrupted operation".into(),
        );
    }
    if record.phase == Phase::RolledBack {
        return Ok(None);
    }
    let hash = &record.intent.candidate;
    let candidate = format!("\"$HOME/.local/share/newport/needle/builds/{hash}/newport-agent\"");
    // These phases are persisted before upload, and cannot have dispatched activation.
    if matches!(record.phase, Phase::Preparing | Phase::Uploading) {
        receipts.update(id, Some(Phase::RolledBack), None).await?;
        return Ok(None);
    }
    crate::agent_runtime::verify_candidate(session, hash).await?;
    let observed: Observation = serde_json::from_str(
        &session
            .execute(&format!("{candidate} deployment operation {id}"), None)
            .await?,
    )
    .map_err(|e| format!("Invalid Needle recovery receipt: {e}"))?;
    let remote = observed
        .receipt
        .ok_or("Needle activation receipt is missing; manual review is required")?;
    if remote.intent != record.intent {
        return Err("Needle recovery receipt does not match the original operation".into());
    }
    receipts.update(id, Some(Phase::RollingBack), None).await?;
    let result: Observation = serde_json::from_str(
        &session
            .execute(&format!("{candidate} deployment recover {id}"), None)
            .await?,
    )
    .map_err(|e| format!("Invalid Needle recovery result: {e}"))?;
    let remote = result.receipt.ok_or("Needle recovery has no receipt")?;
    if remote.intent != record.intent {
        return Err("Needle recovery returned a different operation".into());
    }
    if remote.state != State::RolledBack {
        return Err(remote.error.unwrap_or_else(|| {
            "Needle recovery needs review; changed files were preserved".into()
        }));
    }
    // A repeated completed recovery must never adopt an older build over a later activation.
    if !record.intent.kind.is_deployment() || result.active_build != record.intent.previous {
        receipts.update(id, Some(Phase::RolledBack), None).await?;
        return Ok(None);
    }
    Ok(Some(record.intent.previous))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_updates_replace_only_what_a_manual_install_may() {
        let status = |relation, version: Option<&str>| Status {
            installed: Some(true),
            build_relation: relation,
            installed_version: version.map(str::to_owned),
            ..Status::default()
        };
        // An older release, or a build without release metadata (development).
        assert!(Trigger::Required.allows(&status(BuildRelation::Older, Some("1.0.0"))));
        assert!(Trigger::Required.allows(&status(BuildRelation::Different, None)));
        // Never a newer or unordered signed release, nor the bundled build.
        assert!(!Trigger::Required.allows(&status(BuildRelation::Newer, Some("9.0.0"))));
        assert!(!Trigger::Required.allows(&status(BuildRelation::Different, Some("1.0.0"))));
        assert!(!Trigger::Required.allows(&status(BuildRelation::Bundled, None)));
        assert!(!Trigger::Required.allows(&Status {
            installed: Some(false),
            ..status(BuildRelation::Missing, None)
        }));
        // Automatic updates still need a signed, ordered release.
        assert!(!Trigger::Automatic.allows(&status(BuildRelation::Different, None)));
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
    #[ignore = "Requires disposable Linux OpenSSH fixture"]
    async fn conflict_review_keeps_only_the_confirmed_file_and_scope() {
        use crate::lifecycle_receipts::{
            Phase, Record,
            protocol::{Intent, Kind},
        };
        let server: crate::model::Server = serde_json::from_value(serde_json::json!({
            "id": uuid::Uuid::new_v4(), "name": "Conflict review fixture", "sshUser": "fixture",
            "sshHost": "127.0.0.1", "sshPort": std::env::var("NEWPORT_TEST_SSH_PORT").unwrap().parse::<u16>().unwrap(),
            "identityFile": std::env::var("NEWPORT_TEST_IDENTITY").unwrap(), "agentInstallApproved": true
        })).unwrap();
        let setup = ExecSession::connect(&server).await.unwrap();
        manage_test(&setup, false, false).await.unwrap();
        let registry = crate::server_session::Registry::default();
        let owner = registry.session(&server, 0);
        let lease = owner.acquire(&server).await.unwrap();
        let (_worker, _) = lease.launch(crate::agent::Service::Monitor).await.unwrap();
        let runtime = owner.agent_runtime.snapshot().hello.unwrap().instance_id;
        let hash = check(&setup).await.unwrap().installed_hash.unwrap();
        let id = uuid::Uuid::new_v4();
        owner
            .lifecycle
            .receipts
            .begin(Record {
                server_id: server.id,
                connection: crate::lifecycle_receipts::scope(&server),
                intent: Intent {
                    kind: Kind::Helpers,
                    operation_id: id,
                    candidate: hash.clone(),
                    previous: hash.clone(),
                    integrations: true,
                },
                phase: Phase::Activating,
                error: None,
            })
            .await
            .unwrap();
        setup
            .execute(
                &format!(
                    "\"$HOME/.local/bin/newport-agent\" deployment helpers {hash} --operation {id}"
                ),
                None,
            )
            .await
            .unwrap();
        setup.execute("sed 's/ env / customized /' \"$HOME/.bashrc\" > \"$HOME/edited\"; cat \"$HOME/edited\" > \"$HOME/.bashrc\"", None).await.unwrap();
        assert!(
            lease
                .recover_installation(id)
                .await
                .unwrap_err()
                .contains("edited")
        );
        let report = lease.review_helpers(id).await.unwrap();
        let file = report
            .files
            .iter()
            .find(|f| f.path.ends_with("/.bashrc"))
            .unwrap();
        let old = file.fingerprint.clone().unwrap();
        let index = file.index;
        setup
            .execute("printf '\n# later edit\n' >> \"$HOME/.bashrc\"", None)
            .await
            .unwrap();
        assert!(
            lease
                .keep_helper(id, index, old)
                .await
                .unwrap_err()
                .contains("changed since review")
        );
        let mut other = server.clone();
        other.id = uuid::Uuid::new_v4();
        let other_owner = registry.session(&other, 0);
        let other_lease = other_owner.acquire(&other).await.unwrap();
        assert!(
            other_lease
                .review_helpers(id)
                .await
                .unwrap_err()
                .contains("connection")
        );
        let fresh = lease.review_helpers(id).await.unwrap();
        let fingerprint = fresh.files[index].fingerprint.clone().unwrap();
        let kept = lease
            .keep_helper(id, index, fingerprint.clone())
            .await
            .unwrap();
        assert_eq!(kept.files[index].status, "kept");
        lease.keep_helper(id, index, fingerprint).await.unwrap();
        lease.recover_installation(id).await.unwrap();
        assert_eq!(
            owner.lifecycle.receipts.get(id).unwrap().unwrap().phase,
            Phase::RolledBack
        );
        let content = setup.execute("cat \"$HOME/.bashrc\"", None).await.unwrap();
        assert!(content.contains("customized") && content.contains("# later edit"));
        assert_eq!(
            owner.agent_runtime.snapshot().hello.unwrap().instance_id,
            runtime
        );
        assert_eq!(check(&setup).await.unwrap().installed_hash.unwrap(), hash);
        registry.shutdown().await;
        setup.close().await;
    }
    #[tokio::test]
    #[ignore = "Requires disposable Linux OpenSSH fixture"]
    async fn owned_lifecycle_survives_callers_and_stops_on_retirement() {
        use crate::{lifecycle_receipts::Phase, lifecycle_task::Action};
        use uuid::Uuid;
        let server: crate::model::Server = serde_json::from_value(serde_json::json!({
            "id": Uuid::new_v4(), "name": "Owned lifecycle fixture", "sshUser": "fixture",
            "sshHost": "127.0.0.1", "sshPort": std::env::var("NEWPORT_TEST_SSH_PORT").unwrap().parse::<u16>().unwrap(),
            "identityFile": std::env::var("NEWPORT_TEST_IDENTITY").unwrap(), "agentInstallApproved": true
        })).unwrap();
        let setup = ExecSession::connect(&server).await.unwrap();
        let registry = crate::server_session::Registry::default();
        let owner = registry.session(&server, 0);
        let deployment = Uuid::new_v4();
        let helpers = Uuid::new_v4();
        for (id, action, terminal) in [
            (
                deployment,
                Action::Install {
                    integrations: false,
                },
                Some(Phase::Ready),
            ),
            (helpers, Action::Helpers, None),
            (helpers, Action::Recover, Some(Phase::RolledBack)),
        ] {
            if action == Action::Helpers {
                setup
                    .execute(
                        "printf unrelated > \"$HOME/.local/bin/newport-browser\"",
                        None,
                    )
                    .await
                    .unwrap();
            }
            // Hold the lifecycle gate until the request is admitted, then remove
            // its only waiter. No later caller may drive the operation to completion.
            let gate = owner.lifecycle.operation.lock().await;
            let waiter = tokio::spawn({
                let lease = owner.acquire(&server).await.unwrap();
                async move {
                    match action {
                        Action::Install { integrations } => {
                            lease.install_operation(false, integrations, id).await
                        }
                        Action::Helpers => lease.configure_integrations_operation(id).await,
                        Action::Recover => lease.recover_installation(id).await,
                        Action::AutoUpdate
                        | Action::RequiredUpdate
                        | Action::Keep { .. }
                        | Action::Remove { .. }
                        | Action::RestoreRemoval
                        | Action::RestoreHelpers { .. } => {
                            unreachable!()
                        }
                    }
                }
            });
            tokio::time::timeout(Duration::from_secs(5), async {
                while owner
                    .lifecycle
                    .tasks
                    .active()
                    .is_none_or(|r| r.id != id || r.action != action)
                {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            waiter.abort();
            assert!(waiter.await.unwrap_err().is_cancelled());
            assert_eq!(
                owner.transport_snapshot().demand,
                1,
                "only the lifecycle task retains demand"
            );
            drop(gate);
            tokio::time::timeout(Duration::from_secs(30), async {
                loop {
                    if owner.lifecycle.tasks.active().is_none() {
                        let receipt = owner
                            .lifecycle
                            .receipts
                            .get(id)
                            .unwrap()
                            .expect("owned task persisted its intent");
                        if let Some(phase) = terminal {
                            assert_eq!(receipt.phase, phase, "{:?}", receipt.error);
                        } else {
                            assert_eq!(receipt.phase, Phase::RolledBack);
                            assert!(receipt.error.unwrap().contains("unrelated"));
                        }
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
        }
        assert!(check(&setup).await.unwrap().installed.unwrap());
        // Delay the remote helper transaction at the actual activation lock,
        // after desktop intent is durable and the SSH command was dispatched.
        setup
            .execute("rm \"$HOME/.local/bin/newport-browser\"", None)
            .await
            .unwrap();
        let held = tokio::spawn({
            let setup = setup.with_server(server.clone());
            async move {
                setup.execute(r#"flock "$HOME/.local/share/newport/needle/activation.lock" sh -c 'touch "$HOME/helper-lock-held"; while ! test -e "$HOME/helper-lock-release"; do sleep 0.01; done'"#, None).await
            }
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            while setup
                .execute(
                    "if test -e \"$HOME/helper-lock-held\"; then printf yes; fi",
                    None,
                )
                .await
                .unwrap()
                != "yes"
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let dispatched = Uuid::new_v4();
        let waiter = tokio::spawn({
            let lease = owner.acquire(&server).await.unwrap();
            async move { lease.configure_integrations_operation(dispatched).await }
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            while owner
                .lifecycle
                .receipts
                .get(dispatched)
                .unwrap()
                .is_none_or(|r| r.phase != Phase::Activating)
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let processes = setup.execute("ps -eo comm=,args=", None).await.unwrap();
                if processes.lines().any(|line| {
                    line.trim_start().starts_with("newport-agent ")
                        && line.contains(&dispatched.to_string())
                }) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        assert!(
            owner
                .lifecycle
                .tasks
                .active()
                .is_some_and(|r| r.id == dispatched)
        );
        setup
            .execute("touch \"$HOME/helper-lock-release\"", None)
            .await
            .unwrap();
        held.await.unwrap().unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            while owner.lifecycle.tasks.active().is_some() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let receipt = owner.lifecycle.receipts.get(dispatched).unwrap().unwrap();
        assert_eq!(receipt.phase, Phase::Ready, "{:?}", receipt.error);
        // Retirement must cancel admitted work before it can acquire the gate.
        let gate = owner.lifecycle.operation.lock().await;
        let retired_id = Uuid::new_v4();
        let waiter = tokio::spawn({
            let lease = owner.acquire(&server).await.unwrap();
            async move { lease.configure_integrations_operation(retired_id).await }
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            while owner.lifecycle.tasks.active().is_none() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        registry.invalidate(server.id);
        let replacement = registry.session(&server, 1);
        assert!(waiter.await.unwrap().is_err());
        drop(gate);
        assert!(owner.lifecycle.receipts.get(retired_id).unwrap().is_none());
        assert!(replacement.lifecycle.progress.snapshot().is_none());
        assert!(replacement.lifecycle.tasks.active().is_none());
        registry.shutdown().await;
        setup.close().await;
    }
    #[tokio::test]
    #[ignore = "Requires disposable Linux OpenSSH fixture"]
    async fn helper_operation_resumes_and_recovers_without_replacing_runtime() {
        use crate::lifecycle_receipts::{
            Phase, Record,
            protocol::{Intent, Kind},
        };
        let server: crate::model::Server = serde_json::from_value(serde_json::json!({
            "id": uuid::Uuid::new_v4(), "name": "Helper lifecycle fixture", "sshUser": "fixture",
            "sshHost": "127.0.0.1", "sshPort": std::env::var("NEWPORT_TEST_SSH_PORT").unwrap().parse::<u16>().unwrap(),
            "identityFile": std::env::var("NEWPORT_TEST_IDENTITY").unwrap(), "agentInstallApproved": true
        })).unwrap();
        let setup = ExecSession::connect(&server).await.unwrap();
        manage_test(&setup, false, false).await.unwrap();
        let registry = crate::server_session::Registry::default();
        let owner = registry.session(&server, 0);
        let lease = owner.acquire(&server).await.unwrap();
        let (_worker, _) = lease.launch(crate::agent::Service::Monitor).await.unwrap();
        let runtime = owner.agent_runtime.snapshot().hello.unwrap().instance_id;
        let hash = check(&setup).await.unwrap().installed_hash.unwrap();
        let inode = setup
            .execute("stat -c %i \"$HOME/.local/bin/newport-agent\"", None)
            .await
            .unwrap();
        setup
            .execute(
                "printf unrelated > \"$HOME/.local/bin/newport-browser\"",
                None,
            )
            .await
            .unwrap();
        let failed = uuid::Uuid::new_v4();
        assert!(
            lease
                .configure_integrations_operation(failed)
                .await
                .unwrap_err()
                .contains("unrelated")
        );
        assert_eq!(
            owner
                .lifecycle
                .receipts
                .get(failed)
                .unwrap()
                .unwrap()
                .intent
                .kind,
            Kind::Helpers
        );
        lease.recover_installation(failed).await.unwrap();
        assert_eq!(
            owner.agent_runtime.snapshot().hello.unwrap().instance_id,
            runtime
        );
        assert_eq!(
            check(&setup).await.unwrap().installed_hash.as_deref(),
            Some(hash.as_str())
        );
        setup
            .execute("rm \"$HOME/.local/bin/newport-browser\"", None)
            .await
            .unwrap();
        let resumed = uuid::Uuid::new_v4();
        owner
            .lifecycle
            .receipts
            .begin(Record {
                server_id: server.id,
                connection: crate::lifecycle_receipts::scope(&server),
                intent: Intent {
                    kind: Kind::Helpers,
                    operation_id: resumed,
                    candidate: hash.clone(),
                    previous: hash.clone(),
                    integrations: true,
                },
                phase: Phase::Activating,
                error: None,
            })
            .await
            .unwrap();
        // The remote side completed, but its reply never reached the desktop.
        setup.execute(&format!("\"$HOME/.local/bin/newport-agent\" deployment helpers {hash} --operation {resumed}"), None).await.unwrap();
        let journal = setup
            .execute(
                "sha256sum \"$HOME/.local/share/newport/needle/helpers/journal.json\"",
                None,
            )
            .await
            .unwrap();
        setup
            .execute("printf '\n# later edit\n' >> \"$HOME/.bashrc\"", None)
            .await
            .unwrap();
        lease.resume_installation(resumed).await.unwrap();
        assert_eq!(
            owner
                .lifecycle
                .receipts
                .get(resumed)
                .unwrap()
                .unwrap()
                .phase,
            Phase::Ready
        );
        lease
            .configure_integrations_operation(resumed)
            .await
            .unwrap();
        assert_eq!(
            setup
                .execute(
                    "sha256sum \"$HOME/.local/share/newport/needle/helpers/journal.json\"",
                    None
                )
                .await
                .unwrap(),
            journal
        );
        assert!(
            setup
                .execute("tail -1 \"$HOME/.bashrc\"", None)
                .await
                .unwrap()
                .contains("later edit")
        );
        assert_eq!(
            setup
                .execute("stat -c %i \"$HOME/.local/bin/newport-agent\"", None)
                .await
                .unwrap(),
            inode
        );
        assert_eq!(
            owner.agent_runtime.snapshot().hello.unwrap().instance_id,
            runtime
        );
        assert!(
            lease
                .install_operation(false, true, resumed)
                .await
                .unwrap_err()
                .contains("different request")
        );
    }
    #[tokio::test]
    #[ignore = "Requires disposable Linux OpenSSH fixture"]
    async fn cancelled_helper_plan_is_settled_and_preserves_newer_runtime() {
        use crate::lifecycle_receipts::Phase;
        let server: crate::model::Server = serde_json::from_value(serde_json::json!({
            "id": uuid::Uuid::new_v4(), "name": "Recovery fixture", "sshUser": "fixture",
            "sshHost": "127.0.0.1", "sshPort": std::env::var("NEWPORT_TEST_SSH_PORT").unwrap().parse::<u16>().unwrap(),
            "identityFile": std::env::var("NEWPORT_TEST_IDENTITY").unwrap(), "agentInstallApproved": true
        })).unwrap();
        let setup = ExecSession::connect(&server).await.unwrap();
        manage_test(&setup, false, false).await.unwrap();
        let previous = activate_variant_test(&setup).await;
        setup
            .execute(
                "printf unrelated > \"$HOME/.local/bin/newport-browser\"",
                None,
            )
            .await
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let registry = crate::server_session::Registry::new(directory.path());
        let owner = registry.session(&server, 0);
        let lease = owner.acquire(&server).await.unwrap();
        let id = uuid::Uuid::new_v4();
        assert!(
            lease
                .install_operation(true, true, id)
                .await
                .unwrap_err()
                .contains("unrelated")
        );
        assert_eq!(
            owner.lifecycle.receipts.get(id).unwrap().unwrap().phase,
            Phase::RolledBack
        );
        assert_eq!(
            check(&setup).await.unwrap().installed_hash.as_deref(),
            Some(previous.as_str())
        );
        lease.recover_installation(id).await.unwrap();
        assert_eq!(
            owner.lifecycle.receipts.get(id).unwrap().unwrap().phase,
            Phase::RolledBack
        );
        assert_eq!(
            setup
                .execute("cat \"$HOME/.local/bin/newport-browser\"", None)
                .await
                .unwrap(),
            "unrelated"
        );
        let next = uuid::Uuid::new_v4();
        lease.install_operation(false, false, next).await.unwrap();
        let current = owner.agent_runtime.snapshot().hello.map(|h| h.instance_id);
        lease.recover_installation(id).await.unwrap();
        assert_eq!(
            owner.agent_runtime.snapshot().hello.map(|h| h.instance_id),
            current
        );
        assert!(
            lease
                .recover_installation(next)
                .await
                .unwrap_err()
                .contains("already complete")
        );
        assert_eq!(
            owner.lifecycle.receipts.get(next).unwrap().unwrap().phase,
            Phase::Ready
        );
    }
    #[tokio::test]
    #[ignore = "Requires disposable Linux OpenSSH fixture"]
    async fn durable_operation_resumes_handoff_after_restart_without_second_activation() {
        use crate::lifecycle_receipts::Phase;
        let server: crate::model::Server = serde_json::from_value(serde_json::json!({
            "id": uuid::Uuid::new_v4(), "name": "Durable lifecycle fixture", "sshUser": "fixture",
            "sshHost": "127.0.0.1", "sshPort": std::env::var("NEWPORT_TEST_SSH_PORT").unwrap().parse::<u16>().unwrap(),
            "identityFile": std::env::var("NEWPORT_TEST_IDENTITY").unwrap(), "agentInstallApproved": true
        })).unwrap();
        let setup = ExecSession::connect(&server).await.unwrap();
        manage_test(&setup, false, false).await.unwrap();
        let old_hash = activate_variant_test(&setup).await;
        let directory = tempfile::tempdir().unwrap();
        let registry = crate::server_session::Registry::new(directory.path());
        let owner = registry.session(&server, 0);
        let mut held = Vec::new();
        for _ in 0..5 {
            let lease = owner.acquire(&server).await.unwrap();
            let (channel, _) = lease.launch(crate::agent::Service::Git).await.unwrap();
            let (client, _) = crate::git::client::Client::start(channel).await.unwrap();
            // Retain each runtime owner, but release the SSH worker channel so
            // the fixture's OpenSSH MaxSessions does not mask runtime capacity.
            drop(client);
            held.push(lease);
            owner.agent_runtime.exhaust_registration_budget_for_test();
        }
        let id = uuid::Uuid::new_v4();
        let install = owner.acquire(&server).await.unwrap();
        assert_eq!(
            install
                .install_operation(true, false, id)
                .await
                .unwrap_err(),
            crate::agent_runtime::BUSY
        );
        assert_eq!(
            owner.lifecycle.receipts.get(id).unwrap().unwrap().phase,
            Phase::Handoff
        );
        assert_eq!(
            owner.agent_runtime.snapshot().hello.unwrap().build_sha256,
            old_hash
        );
        let stamp = setup
            .execute(
                "stat -c '%y' \"$HOME/.local/share/newport/needle/operations.json\"",
                None,
            )
            .await
            .unwrap();
        let remote = setup
            .execute(
                &format!("\"$HOME/.local/bin/newport-agent\" deployment operation {id}"),
                None,
            )
            .await
            .unwrap();
        let observed: crate::lifecycle_receipts::protocol::Observation =
            serde_json::from_str(&remote).unwrap();
        assert_eq!(
            observed.receipt.unwrap().state,
            crate::lifecycle_receipts::protocol::State::Succeeded
        );
        drop(held);
        drop(install);
        registry.shutdown().await;
        drop(owner);
        drop(registry);
        let restarted = crate::server_session::Registry::new(directory.path());
        let owner = restarted.session(&server, 0);
        let lease = owner.acquire(&server).await.unwrap();
        lease.install_operation(true, false, id).await.unwrap();
        assert_eq!(
            owner.lifecycle.receipts.get(id).unwrap().unwrap().phase,
            Phase::Ready
        );
        assert_eq!(
            setup
                .execute(
                    "stat -c '%y' \"$HOME/.local/share/newport/needle/operations.json\"",
                    None
                )
                .await
                .unwrap(),
            stamp
        );
        let instance = owner.agent_runtime.snapshot().hello.unwrap().instance_id;
        lease.install_operation(true, false, id).await.unwrap();
        assert_eq!(
            owner.agent_runtime.snapshot().hello.unwrap().instance_id,
            instance
        );
        assert_eq!(
            setup
                .execute(
                    "stat -c '%y' \"$HOME/.local/share/newport/needle/operations.json\"",
                    None
                )
                .await
                .unwrap(),
            stamp
        );
        restarted.shutdown().await;
        setup.close().await;
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
        lease.install(false, false).await.unwrap();
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
        lease.install(true, false).await.unwrap();
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
        // A shell-helper collision causes activation to roll back while the same
        // remote lock is held; it cannot undo another desktop's later activation.
        lease.execute("printf 'unrelated' > \"$HOME/.local/bin/newport-browser\"; printf '#!/bin/sh\\necho newport-agent/4\\n' > \"$HOME/.local/bin/.legacy\"; chmod 700 \"$HOME/.local/bin/.legacy\"; mv -f \"$HOME/.local/bin/.legacy\" \"$HOME/.local/bin/newport-agent\"", None).await.unwrap();
        let legacy = check(&connection).await.unwrap().installed_hash.unwrap();
        assert_ne!(legacy, hash);
        let error = lease.install(true, true).await.unwrap_err();
        assert!(error.contains("no helper changes remain"), "{error}");
        assert_eq!(
            check(&connection).await.unwrap().installed_hash.as_deref(),
            Some(legacy.as_str())
        );
        assert_eq!(
            lease
                .execute("cat \"$HOME/.local/bin/newport-browser\"", None)
                .await
                .unwrap(),
            "unrelated"
        );
        assert!(!worker.is_closed());
        // The entry point is now the restored legacy stub. New work on the
        // surviving runtime must launch its retained build, not that entry point.
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
    fn helper_discovery_refreshes_after_build_changes_without_retrying_errors_in_a_loop() {
        let state = State::default();
        let now = Instant::now();
        state.record(1, &parse_check("Linux\nx86_64\nmissing\n"));
        assert!(!state.due(1, now));
        let installed = parse_check(&format!("Linux\nx86_64\n{}  agent", "a".repeat(64)));
        state.record(1, &installed);
        assert!(
            state.due(1, now),
            "new installation needs its first helper observation"
        );
        state
            .helper_history
            .record(Err("temporary history failure".into()));
        assert!(
            !state.due(1, now),
            "history failure follows the periodic retry schedule"
        );
        state.record(1, &installed);
        assert!(
            !state.due(1, now),
            "unchanged build does not invalidate history"
        );
        state.record(
            1,
            &parse_check(&format!("Linux\nx86_64\n{}  agent", "b".repeat(64))),
        );
        assert!(
            state.due(1, now),
            "a different build needs fresh helper evidence"
        );
        state.helper_history.record(Ok(None));
        assert!(
            !state.due(1, now),
            "unsupported history does not cause a retry loop"
        );
        assert!(state.due(2, now));
        assert!(state.due(1, now + Duration::from_secs(3601)));
    }
}
