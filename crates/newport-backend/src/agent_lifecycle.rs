//! Needle on a server: install (also update) and uninstall. Installing again
//! converges and uninstalling is final; nothing is journaled in between.
use crate::agent_payload::{TransferEncoding, payload};
use crate::ssh::ExecSession;
use newport_release::agent_product::Variant;
use serde::Serialize;
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
static BUNDLED_RELEASE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/needle-release.json"));

#[cfg(test)]
#[path = "agent_release_tests.rs"]
mod release_tests;

pub(crate) fn transfer_size(arch: &str, encoding: TransferEncoding) -> Option<usize> {
    payload(arch).map(|payload| payload.transfer_size(encoding))
}
/// Validate packaged resources without contacting a server or accessing profiles.
pub fn check_resources() -> Result<(), String> {
    crate::agent_payload::check_resources()
}
/// The embedded payload and release identity as JSON (`--agent-identity`),
/// without reading resources, contacting a server or accessing profiles.
pub fn agent_identity() -> Result<String, String> {
    let report = agent_report(BUNDLED_RELEASE, newport_release::needle::verify_production)?;
    serde_json::to_string(&report).map_err(|error| error.to_string())
}
fn agent_report(
    envelope: &[u8],
    verify: impl Fn(&[u8]) -> Result<newport_release::needle::VerifiedRelease, String>,
) -> Result<newport_release::desktop::AgentReport, String> {
    use newport_release::desktop::{AgentReport, Envelope, Payload, Payloads, REPORT_SCHEMA};
    use sha2::{Digest, Sha256};
    let (x86_64, aarch64) = (payload("x86_64").unwrap(), payload("aarch64").unwrap());
    let envelope = if envelope.is_empty() {
        None
    } else {
        let release = verify(envelope)?;
        let version = release
            .version_for("x86_64", x86_64.sha256)
            .filter(|version| release.version_for("aarch64", aarch64.sha256) == Some(*version))
            .ok_or("Bundled release metadata does not match the bundled payloads")?;
        Some(Envelope {
            sha256: Sha256::digest(envelope)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect(),
            size: envelope.len() as u64,
            version: version.to_string(),
            product: release.product_for("x86_64", x86_64.sha256).cloned(),
        })
    };
    let described = |payload: crate::agent_payload::Payload| Payload {
        sha256: payload.sha256.into(),
        size: payload.size as u64,
    };
    Ok(AgentReport {
        schema: REPORT_SCHEMA,
        variant: crate::agent_payload::variant(),
        payloads: Payloads {
            x86_64: described(x86_64),
            aarch64: described(aarch64),
        },
        envelope,
    })
}
/// The outcome of installing Needle.
pub struct Deployment {
    pub output: String,
    pub build_sha256: String,
    /// Shell setup the person still has to do by hand.
    pub setup_notice: Option<String>,
}
/// Installs or updates Needle on the server: this Newport's build, verified,
/// then made active as the trigger's `Activation` says; with `integrations`,
/// also the command links and shell setup.
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
    let platform = session
        .execute(
            &format!("uname -s; uname -m\n{}", TransferEncoding::PROBE),
            None,
        )
        .await?;
    let mut lines = platform.lines();
    if lines.next() != Some("Linux") {
        return Err("The Newport agent requires Linux.".into());
    }
    let architecture = lines.next().unwrap_or_default();
    let encoding = TransferEncoding::parse(lines.next().unwrap_or_default())?;
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
    require_product_replacement(&status, trigger)?;
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
            "sh -c '{}' newport-install {hash} {}",
            include_str!("agent-install.sh").replace('\'', "'\"'\"'"),
            encoding.as_str()
        );
        let total = binary.transfer_size(encoding);
        progress.update("uploading", 0, total, None);
        session
            .upload_resource(&command, binary.stream(encoding)?, &|sent| {
                progress.update(
                    if sent == total {
                        "installing"
                    } else {
                        "uploading"
                    },
                    sent,
                    total,
                    None,
                )
            })
            .await?;
    }
    progress.update("verifying", 0, 0, None);
    let product = crate::agent_runtime::verify_candidate(session, &hash).await?;
    if product.variant != status.bundled_variant {
        return Err("Candidate product does not match the bundled agent variant".into());
    }
    if product.variant == Variant::Full
        && product
            .services
            .get("remote-apps")
            .is_none_or(|versions| !versions.contains(&1))
    {
        return Err("Candidate Remote Apps protocol is incompatible".into());
    }
    if !BUNDLED_RELEASE.is_empty() {
        let release = newport_release::needle::verify_production(BUNDLED_RELEASE)?;
        if release.variant_for(architecture, &hash) != Some(product.variant)
            || release
                .product_for(architecture, &hash)
                .is_some_and(|signed| signed != &product)
        {
            return Err("Candidate protocols do not match the signed agent product".into());
        }
        record_release(
            session,
            &hash,
            architecture,
            BUNDLED_RELEASE,
            newport_release::needle::verify_production,
        )
        .await?;
    }
    let activation = trigger.activation(&status);
    // Either way this Newport's sessions run its build; `active` says
    // whether the entry point is it now.
    let kept = |active| Deployment {
        output: if active { INSTALLED } else { BESIDE }.into(),
        build_sha256: hash.clone(),
        setup_notice: None,
    };
    if activation == Activation::Keep {
        let active = status.installed_hash.as_deref() == Some(hash.as_str());
        return Ok(kept(active));
    }
    progress.update("activating", 0, 0, None);
    #[derive(serde::Deserialize)]
    struct Installed {
        build: String,
        active: bool,
        notice: Option<String>,
    }
    let installed: Installed = serde_json::from_str(
        &session
            .execute(
                &format!(
                    "{candidate} deployment install {hash}{}{} --expect-current {}{}",
                    if integrations { " --integrations" } else { "" },
                    if activation == Activation::UnlessRunning {
                        " --unless-running"
                    } else {
                        ""
                    },
                    status.installed_hash.as_deref().unwrap_or("missing"),
                    match status.installed_variant {
                        Some(Variant::Core) => " --expect-variant core",
                        Some(Variant::Full) => " --expect-variant full",
                        None => "",
                    }
                ),
                None,
            )
            .await?,
    )
    .map_err(|e| format!("Invalid Needle install result: {e}"))?;
    if installed.build != hash {
        return Err("Needle didn't confirm the installed build".into());
    }
    if !installed.active {
        return Ok(kept(false));
    }
    Ok(Deployment {
        setup_notice: installed.notice,
        ..kept(true)
    })
}

const INSTALLED: &str = "Needle installed and verified.";
const BESIDE: &str = "Needle is ready for this Newport. The server keeps its default Needle build, which another connection may be using.";

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
    /// This Newport's build of the agent for the server's architecture.
    pub bundled_hash: Option<String>,
    pub installed_version: Option<String>,
    pub bundled_version: Option<String>,
    pub installed_variant: Option<Variant>,
    pub bundled_variant: Variant,
    /// Whether a process on the server runs the installed build, such as
    /// another desktop's runtime. None where the server can't tell.
    pub installed_running: Option<bool>,
    /// Whether this Newport's build is on the server, ready to run.
    pub bundled_staged: bool,
    pub checked_at: u64,
    pub error: Option<String>,
    /// The last automatic replacement of the installed agent, if any.
    pub automatic_update: Option<UpdateOutcome>,
}
/// How an automatic replacement of the server's agent ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateOutcome {
    /// Milliseconds since the Unix epoch.
    pub at: u64,
    pub from: Option<String>,
    pub to: Option<String>,
    pub error: Option<String>,
}
const AUTOMATIC_CADENCE: Duration = Duration::from_secs(3600);
const AUTOMATIC_RETRY: Duration = Duration::from_secs(600);
/// How soon an installed build kept only because a process still runs it is
/// checked again: a desktop's previous runtime usually exits within minutes.
const IDLE_RECHECK: Duration = Duration::from_secs(120);

/// What an update does with the server's entry point, the build every
/// desktop's terminals and commands run and a desktop without its own build
/// starts. Each desktop runs its own build beside it either way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activation {
    /// Point it at this Newport's build.
    Replace,
    /// Point it at this Newport's build unless, when the install runs, a
    /// process runs the installed one.
    UnlessRunning,
    /// Leave it.
    Keep,
}

impl Status {
    /// Automatic replacement requires a known product it may replace.
    /// Entitlement is not product identity.
    fn replaceable_product(&self) -> bool {
        self.installed == Some(true) && self.error.is_none() && self.product_replaceable(true)
    }
    /// Whether this Newport's product may replace the installed one: the same
    /// product, or core by the full agent of the same or a newer release. The
    /// person's own install may also move core to full without release order;
    /// removing the full agent's features is never this desktop's choice.
    fn product_replaceable(&self, automatic: bool) -> bool {
        self.installed_variant.is_some_and(|installed| {
            self.bundled_variant
                .may_replace(installed, automatic, false)
                || (installed == Variant::Core
                    && self.bundled_variant == Variant::Full
                    && matches!(
                        self.build_relation,
                        BuildRelation::Older | BuildRelation::SameRelease
                    ))
        })
    }
    /// The entry point follows signed release order: an older release is
    /// replaced, a newer one never. Builds without an order (development
    /// builds, other builds of one release) take turns only when idle: one
    /// that a process runs, perhaps another desktop's runtime, stays, so two
    /// desktops never replace each other's build back and forth.
    pub(crate) fn activation(&self) -> Activation {
        match self.build_relation {
            BuildRelation::Older => Activation::Replace,
            BuildRelation::Different | BuildRelation::SameRelease
                if self.installed_running == Some(false) =>
            {
                Activation::UnlessRunning
            }
            _ => Activation::Keep,
        }
    }
    /// A build of a product this Newport may not replace isn't an update.
    fn withhold_product_change(&mut self) {
        if self.installed_variant.is_some() && !self.product_replaceable(true) {
            self.update_available = Some(false);
        }
    }
    /// The entry point would move to this Newport's build, but a process still
    /// runs the installed one. This desktop runs its own build meanwhile, and
    /// the move happens once the installed build is idle.
    pub(crate) fn waiting_for_idle(&self) -> bool {
        self.replaceable_product()
            && matches!(
                self.build_relation,
                BuildRelation::Different | BuildRelation::SameRelease
            )
            && self.installed_running != Some(false)
    }
    /// Whether an automatic update has work: replacing the entry point, or
    /// staging this Newport's build to run beside another desktop's.
    pub(crate) fn automatic_update_available(&self) -> bool {
        self.replaceable_product()
            && match self.build_relation {
                BuildRelation::Older => true,
                BuildRelation::Different | BuildRelation::SameRelease => {
                    self.activation() != Activation::Keep || !self.bundled_staged
                }
                _ => false,
            }
    }
    /// Whether a runtime on another build can move to this Newport's own,
    /// staged first if needed. A newer installed release serves it instead.
    pub(crate) fn own_runtime_available(&self) -> bool {
        self.replaceable_product()
            && matches!(
                self.build_relation,
                BuildRelation::Older
                    | BuildRelation::Different
                    | BuildRelation::SameRelease
                    | BuildRelation::Bundled
            )
    }
}

/// Who started a Needle deployment, which decides what it may replace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    /// The person chose to install, reinstall or update.
    Manual,
    /// The connection or hourly check found work (see `automatic_update_available`).
    Automatic,
    /// A feature found the runtime on another build than this Newport's.
    Required,
}
impl Trigger {
    pub fn allows(self, status: &Status) -> bool {
        match self {
            Self::Manual => true,
            Self::Automatic => status.automatic_update_available(),
            Self::Required => status.own_runtime_available(),
        }
    }
    /// What the deployment does with the entry point; the person's own
    /// install always points it at this Newport's build.
    pub(crate) fn activation(self, status: &Status) -> Activation {
        match self {
            Self::Manual => Activation::Replace,
            _ if status.build_relation == BuildRelation::Bundled => Activation::Keep,
            _ => status.activation(),
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
    /// When the last attempt was claimed or failed, and how long until the next.
    automatic_attempt: Mutex<Option<(Instant, Duration)>>,
    automatic_outcome: Mutex<Option<UpdateOutcome>>,
    /// Set when the automatic update declined to replace anything.
    automatic_skipped: std::sync::atomic::AtomicBool,
    required: Mutex<Option<RequiredUpdate>>,
}
impl State {
    pub fn snapshot(&self) -> Status {
        let mut status = self.observation.lock().unwrap().status.clone();
        status.automatic_update = self.automatic_outcome.lock().unwrap().clone();
        status
    }
    /// A check's result with the last update outcome on it, so every reader of
    /// it can show a failed update. Records the result as the latest evidence.
    pub(crate) fn checked(
        &self,
        generation: u64,
        result: Result<Status, String>,
    ) -> Result<Status, String> {
        self.record(generation, &result);
        result.map(|mut status| {
            status.automatic_update = self.automatic_outcome.lock().unwrap().clone();
            status
        })
    }
    pub(crate) fn due(&self, generation: u64, now: Instant) -> bool {
        let observation = self.observation.lock().unwrap();
        let interval = if observation.status.waiting_for_idle() {
            IDLE_RECHECK
        } else {
            AUTOMATIC_CADENCE
        };
        observation.probe.is_none_or(|(seen, at)| {
            seen != generation || now.saturating_duration_since(at) >= interval
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
        if previous.is_some_and(|(at, wait)| now.saturating_duration_since(at) < wait) {
            return false;
        }
        *previous = Some((now, AUTOMATIC_CADENCE));
        self.automatic_skipped
            .store(false, std::sync::atomic::Ordering::SeqCst);
        true
    }
    /// The automatic update found nothing it may replace.
    pub(crate) fn skipped_automatic_update(&self) {
        self.automatic_skipped
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
    /// Records how an automatic replacement ended. A failure gives the claim
    /// back, so the next attempt waits ten minutes instead of an hour.
    pub(crate) fn finish_automatic_update(
        &self,
        now: Instant,
        status: &Status,
        result: &Result<(), String>,
    ) {
        // Nothing was replaced, so there is no outcome to remember: an earlier
        // failure stays what the status shows.
        if result.is_ok()
            && self
                .automatic_skipped
                .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            // Its recheck found the installed build in use, perhaps only by a
            // shell starting; try again once it may be idle, not in an hour.
            if self.observation.lock().unwrap().status.waiting_for_idle() {
                *self.automatic_attempt.lock().unwrap() = Some((now, IDLE_RECHECK));
            }
            return;
        }
        if result.is_err() {
            *self.automatic_attempt.lock().unwrap() = Some((now, AUTOMATIC_RETRY));
        } else if status.waiting_for_idle() {
            // Only this Newport's build was staged; the installed one stays
            // until it is idle, so nothing was replaced to report.
            *self.automatic_attempt.lock().unwrap() = Some((now, IDLE_RECHECK));
            return;
        }
        self.note_update(status, result);
    }
    /// Remembers the outcome of a replacement the status can show.
    pub(crate) fn note_update(&self, status: &Status, result: &Result<(), String>) {
        *self.automatic_outcome.lock().unwrap() = Some(UpdateOutcome {
            at: crate::metrics_db::now_ms() as u64,
            from: status.installed_hash.clone(),
            to: status.bundled_hash.clone(),
            error: result.as_ref().err().cloned(),
        });
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
    /// Holds a claimed required update; dropped unfinished (the waiting
    /// feature was cancelled), it gives the claim back instead of staying
    /// Running for good.
    pub(crate) fn required_guard(&self) -> RequiredGuard<'_> {
        RequiredGuard {
            state: self,
            finished: false,
        }
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
    /// A launch found the runtime on another build than this Newport bundles:
    /// whether it may move to this Newport's own build first.
    pub(crate) fn runtime_update_allowed(
        &self,
        status: &Status,
        runtime_build: &str,
        server: &crate::model::Server,
    ) -> bool {
        status
            .bundled_hash
            .as_deref()
            .is_some_and(|bundled| !bundled.eq_ignore_ascii_case(runtime_build))
            && status.own_runtime_available()
            && self.automatic_update_unblocked(server)
    }
    pub(crate) fn automatic_update_unblocked(&self, server: &crate::model::Server) -> bool {
        server.agent_install_approved && self.tasks.active().is_none()
    }
}
pub(crate) struct RequiredGuard<'a> {
    state: &'a State,
    finished: bool,
}
impl RequiredGuard<'_> {
    pub(crate) fn finish(mut self, result: &Result<String, String>) {
        self.finished = true;
        self.state.finish_required_update(result);
    }
}
impl Drop for RequiredGuard<'_> {
    fn drop(&mut self) {
        if !self.finished {
            let mut required = self.state.required.lock().unwrap();
            if matches!(*required, Some(RequiredUpdate::Running)) {
                *required = None;
            }
        }
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
    fn line<'a>(rest: &mut &'a str) -> &'a str {
        let (line, remainder) = rest.split_once('\n').unwrap_or((rest, ""));
        *rest = remainder;
        line
    }
    let mut rest = output;
    if line(&mut rest) != "Linux" {
        return Err("Needle requires Linux.".into());
    }
    let architecture = line(&mut rest);
    let binary = payload(architecture).ok_or("Unsupported server architecture.")?;
    if rest.is_empty() {
        return Err("Incomplete Needle inspection response.".into());
    }
    let installed_hash = line(&mut rest)
        .split_whitespace()
        .next()
        .unwrap_or_default();
    // Then the builds processes run (where they can be listed) and the staged
    // builds, each a line of hashes after its name; the rest is metadata.
    let mut builds = |name: &str| {
        let listed = rest
            .split_once('\n')
            .map_or(rest, |(line, _)| line)
            .strip_prefix(name)
            .filter(|hashes| hashes.is_empty() || hashes.starts_with(' '))?;
        line(&mut rest);
        Some(
            listed
                .split_whitespace()
                .filter(|hash| crate::agent_runtime::protocol::valid_build(hash))
                .map(str::to_owned)
                .collect::<std::collections::HashSet<_>>(),
        )
    };
    let running = builds("running");
    let staged = builds("staged").unwrap_or_default();
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
    let metadata = rest;
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
    let bundled_variant = bundled
        .as_ref()
        .and_then(|release| release.variant_for(architecture, &expected))
        .unwrap_or_else(crate::agent_payload::variant);
    let installed_variant = installed
        .as_ref()
        .zip(installed_hash.as_deref())
        .and_then(|(release, hash)| release.variant_for(architecture, hash))
        .or_else(|| (relation == BuildRelation::Bundled).then_some(bundled_variant));
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
        installed_running: installed_hash
            .as_ref()
            .and_then(|hash| Some(running.as_ref()?.contains(hash))),
        bundled_staged: staged.contains(&expected),
        installed_hash,
        bundled_hash: Some(expected),
        installed_version,
        bundled_version,
        installed_variant,
        bundled_variant,
        checked_at: crate::metrics_db::now_ms() as u64,
        error: None,
        automatic_update: None,
    })
}
// A failed hash or an unexpected/truncated response must never be read as absence.
pub(crate) const CHECK: &str = include_str!("agent-release-check.sh");
pub(crate) async fn check(session: &ExecSession) -> Result<Status, String> {
    let mut status = parse_check(&session.execute(CHECK, None).await?)?;
    if status.installed_variant.is_none()
        && let Some(hash) = &status.installed_hash
    {
        match crate::agent_runtime::inspect_product(session, hash).await {
            Ok(product) => status.installed_variant = Some(product.variant),
            Err(error) => {
                status.error = Some(format!(
                    "Cannot identify the installed agent product: {error}"
                ))
            }
        }
    }
    status.withhold_product_change();
    Ok(status)
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

fn require_product_replacement(status: &Status, trigger: Trigger) -> Result<(), String> {
    if let Some(error) = &status.error {
        return Err(error.clone());
    }
    if status.installed == Some(false) {
        return Ok(());
    }
    if status.installed_variant.is_none() {
        return Err("Cannot identify the installed agent product; leaving it unchanged".into());
    }
    if !status.product_replaceable(trigger != Trigger::Manual) {
        return Err("This installation uses a different agent product. Keep the full agent, or explicitly request a product migration.".into());
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

    fn installed(relation: BuildRelation, running: Option<bool>, staged: bool) -> Status {
        Status {
            installed: Some(true),
            installed_variant: Some(Variant::Core),
            build_relation: relation,
            installed_running: running,
            bundled_staged: staged,
            ..Status::default()
        }
    }

    #[test]
    fn a_build_kept_only_because_it_runs_is_checked_again_soon() {
        let busy = installed(BuildRelation::Different, Some(true), false);
        assert!(busy.waiting_for_idle());
        assert!(!installed(BuildRelation::Different, Some(false), true).waiting_for_idle());
        assert!(!installed(BuildRelation::Older, Some(true), true).waiting_for_idle());

        let state = State::default();
        let start = Instant::now();
        state.record(1, &Ok(busy.clone()));
        assert!(!state.due(1, start + IDLE_RECHECK - Duration::from_secs(1)));
        assert!(state.due(1, start + IDLE_RECHECK + Duration::from_secs(1)));

        assert!(state.claim_automatic_update(start));
        state.finish_automatic_update(start, &busy, &Ok(()));
        assert!(
            state.snapshot().automatic_update.is_none(),
            "staging beside a running build replaces nothing"
        );
        assert!(!state.claim_automatic_update(start + IDLE_RECHECK - Duration::from_secs(1)));
        assert!(state.claim_automatic_update(start + IDLE_RECHECK));
    }

    #[test]
    fn the_entry_point_follows_release_order_and_otherwise_moves_only_when_idle() {
        use Activation::{Keep, Replace, UnlessRunning};
        for running in [None, Some(false), Some(true)] {
            for staged in [false, true] {
                // An older release is replaced, also while it runs.
                let older = installed(BuildRelation::Older, running, staged);
                assert_eq!(Trigger::Automatic.activation(&older), Replace);
                assert!(Trigger::Automatic.allows(&older));
                assert!(Trigger::Required.allows(&older));
                // A newer release never is, and serves this Newport's sessions.
                let newer = installed(BuildRelation::Newer, running, staged);
                assert_eq!(newer.activation(), Keep);
                assert!(!Trigger::Automatic.allows(&newer));
                assert!(!Trigger::Required.allows(&newer));
                assert!(require_ordered_replacement(&newer).is_err());
                // The person's own install points it at this build.
                assert_eq!(Trigger::Manual.activation(&newer), Replace);
            }
        }
        for relation in [BuildRelation::Different, BuildRelation::SameRelease] {
            // Unordered and idle: replaced, checked again under the lock.
            let idle = installed(relation, Some(false), true);
            assert_eq!(Trigger::Automatic.activation(&idle), UnlessRunning);
            assert!(Trigger::Automatic.allows(&idle));
            for running in [Some(true), None] {
                // Another desktop may run it: it stays, and this Newport
                // stages its own build once to run beside it.
                let busy = installed(relation, running, false);
                assert_eq!(Trigger::Automatic.activation(&busy), Keep);
                assert!(Trigger::Automatic.allows(&busy));
                let staged = installed(relation, running, true);
                assert!(!Trigger::Automatic.allows(&staged), "nothing left to do");
                assert!(Trigger::Required.allows(&staged));
                assert_eq!(Trigger::Required.activation(&staged), Keep);
            }
        }
        // The bundled build installed: a runtime on another build still moves.
        let bundled = installed(BuildRelation::Bundled, Some(true), true);
        assert!(!Trigger::Automatic.allows(&bundled));
        assert!(Trigger::Required.allows(&bundled));
        assert_eq!(Trigger::Required.activation(&bundled), Keep);
        // Nothing installed, an unreadable installation or another product.
        let missing = Status {
            installed: Some(false),
            ..installed(BuildRelation::Missing, None, false)
        };
        let failed = Status {
            error: Some("unreadable".into()),
            ..installed(BuildRelation::Older, Some(false), false)
        };
        let other = Status {
            installed_variant: Some(Variant::Full),
            ..installed(BuildRelation::Older, Some(false), false)
        };
        for status in [missing, failed, other] {
            assert!(!Trigger::Automatic.allows(&status));
            assert!(!Trigger::Required.allows(&status));
        }
    }

    /// Two desktops with unordered builds on one server, each checking again
    /// and again: the entry point moves only once the other one leaves, and
    /// each keeps running its own build throughout.
    #[test]
    fn two_desktops_with_unordered_builds_never_take_turns() {
        use std::collections::BTreeSet;
        struct Server {
            entry: String,
            running: BTreeSet<String>,
            staged: BTreeSet<String>,
            moves: usize,
        }
        let list = |builds: &BTreeSet<String>| {
            builds
                .iter()
                .map(|hash| format!(" {hash}"))
                .collect::<String>()
        };
        // One desktop's automatic update, as `manage` runs it.
        let check = |server: &mut Server, own: &str| {
            let output = format!(
                "Linux\nx86_64\n{}\nrunning{}\nstaged{}\n",
                server.entry,
                list(&server.running),
                list(&server.staged)
            );
            let mut status =
                parse_candidate_check(&output, &[], Some(own), |_| unreachable!()).unwrap();
            status.installed_variant = Some(status.bundled_variant);
            if !Trigger::Automatic.allows(&status) {
                return;
            }
            server.staged.insert(own.into());
            let replace = match Trigger::Automatic.activation(&status) {
                Activation::Replace => true,
                Activation::UnlessRunning => !server.running.contains(&server.entry),
                Activation::Keep => false,
            };
            if replace && server.entry != own {
                server.entry = own.into();
                server.moves += 1;
            }
        };
        // A runtime starts its own build when staged, else the entry point's;
        // after an update it adopts its own.
        let start = |server: &mut Server, own: &str| {
            let build = if server.staged.contains(own) {
                own.to_owned()
            } else {
                server.entry.clone()
            };
            server.running.insert(build);
        };
        let (a, b) = ("a".repeat(64), "b".repeat(64));
        let mut server = Server {
            entry: a.clone(),
            running: BTreeSet::new(),
            staged: BTreeSet::from([a.clone()]),
            moves: 0,
        };
        start(&mut server, &a);
        start(&mut server, &b);
        assert!(server.running.contains(&a), "B starts on A's build");
        for _ in 0..24 {
            check(&mut server, &a);
            check(&mut server, &b);
            // B adopted its own build; A's still runs for A.
            server.running = BTreeSet::from([a.clone(), b.clone()]);
        }
        assert_eq!((server.entry.as_str(), server.moves), (a.as_str(), 0));
        assert!(server.staged.contains(&b));
        // A leaves: B takes the entry point once.
        server.running.remove(&a);
        for _ in 0..24 {
            check(&mut server, &b);
        }
        assert_eq!((server.entry.as_str(), server.moves), (b.as_str(), 1));
        // A comes back on its own build and leaves the entry point alone.
        start(&mut server, &a);
        assert_eq!(server.running, BTreeSet::from([a.clone(), b.clone()]));
        for _ in 0..24 {
            check(&mut server, &a);
            check(&mut server, &b);
        }
        assert_eq!((server.entry.as_str(), server.moves), (b.as_str(), 1));
    }

    #[test]
    fn the_check_reads_running_and_staged_builds() {
        let (installed, own, other) = ("a".repeat(64), "b".repeat(64), "c".repeat(64));
        let parse = |output: &str| {
            parse_candidate_check(output, &[], Some(&own), |_| unreachable!()).unwrap()
        };
        let status = parse(&format!(
            "Linux\nx86_64\n{installed}\nrunning {other} {installed}\nstaged {installed} {own}\n"
        ));
        assert_eq!(status.installed_running, Some(true));
        assert!(status.bundled_staged);
        let status = parse(&format!(
            "Linux\nx86_64\n{installed}\nrunning\nstaged {installed} not-a-build\n"
        ));
        assert_eq!(status.installed_running, Some(false));
        assert!(!status.bundled_staged);
        // Where processes can't be listed, the server can't tell.
        let status = parse(&format!("Linux\nx86_64\n{installed}\nstaged {own}\n"));
        assert_eq!(status.installed_running, None);
        assert!(status.bundled_staged);
        let missing = parse("Linux\nx86_64\nmissing\nrunning\nstaged\n");
        assert_eq!(missing.installed, Some(false));
        assert_eq!(missing.installed_running, None);
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
    #[test]
    fn a_failed_automatic_update_gives_back_the_claim_and_retries_after_ten_minutes() {
        let state = State::default();
        let status = Status {
            installed: Some(true),
            installed_hash: Some("a".repeat(64)),
            bundled_hash: Some("b".repeat(64)),
            ..Status::default()
        };
        let start = Instant::now();
        assert!(state.claim_automatic_update(start));
        assert!(!state.claim_automatic_update(start));
        let failed_at = start + Duration::from_secs(30);
        state.finish_automatic_update(failed_at, &status, &Err("upload failed".into()));
        assert!(!state.claim_automatic_update(failed_at + Duration::from_secs(599)));
        assert!(state.claim_automatic_update(failed_at + Duration::from_secs(600)));
        // A claimed attempt that succeeds keeps the hourly cadence.
        let later = failed_at + Duration::from_secs(600);
        state.finish_automatic_update(later, &status, &Ok(()));
        assert!(!state.claim_automatic_update(later + Duration::from_secs(3599)));
        assert!(state.claim_automatic_update(later + Duration::from_secs(3600)));
    }

    #[test]
    fn an_old_runtime_is_updated_once_before_a_launch_unless_blocked() {
        let state = State::default();
        let server: crate::model::Server = serde_json::from_value(serde_json::json!({
            "id": uuid::Uuid::new_v4(), "name": "fixture", "sshHost": "127.0.0.1",
            "sshPort": 22, "sshUser": "fixture", "agentInstallApproved": true
        }))
        .unwrap();
        let bundled = "b".repeat(64);
        let status = |relation| Status {
            installed: Some(true),
            installed_variant: Some(Variant::Full),
            bundled_variant: Variant::Full,
            build_relation: relation,
            bundled_hash: Some(bundled.clone()),
            ..Status::default()
        };
        let old = "a".repeat(64);
        assert!(state.runtime_update_allowed(&status(BuildRelation::Older), &old, &server));
        assert!(state.runtime_update_allowed(&status(BuildRelation::Different), &old, &server));
        // Also beside a build another desktop runs, or once this one is installed.
        let busy = Status {
            installed_running: Some(true),
            bundled_staged: true,
            ..status(BuildRelation::Different)
        };
        assert!(state.runtime_update_allowed(&busy, &old, &server));
        assert!(state.runtime_update_allowed(&status(BuildRelation::Bundled), &old, &server));
        // The runtime already runs this build, or a newer release is installed.
        assert!(!state.runtime_update_allowed(&status(BuildRelation::Older), &bundled, &server));
        assert!(!state.runtime_update_allowed(&status(BuildRelation::Newer), &old, &server));
        let mut unapproved = server.clone();
        unapproved.agent_install_approved = false;
        assert!(!state.runtime_update_allowed(&status(BuildRelation::Older), &old, &unapproved));
        // The feature path's own claim allows one request, then its backoff.
        let now = Instant::now();
        assert_eq!(state.claim_required_update(now), Ok(()));
        state.finish_required_update(&Err("upload failed".into()));
        assert!(state.claim_required_update(now).is_err());
    }

    #[test]
    fn the_last_automatic_update_outcome_is_part_of_the_snapshot() {
        let state = State::default();
        assert_eq!(state.snapshot().automatic_update, None);
        let status = Status {
            installed_hash: Some("a".repeat(64)),
            bundled_hash: Some("b".repeat(64)),
            ..Status::default()
        };
        state.finish_automatic_update(Instant::now(), &status, &Err("upload failed".into()));
        let outcome = state.snapshot().automatic_update.unwrap();
        assert_eq!(outcome.error.as_deref(), Some("upload failed"));
        assert_eq!(outcome.from, status.installed_hash);
        assert_eq!(outcome.to, status.bundled_hash);
        assert!(outcome.at > 0);
        // A later check replaces the observation but not the outcome.
        state.record(1, &Ok(Status::default()));
        assert!(state.snapshot().automatic_update.is_some());
        let json = serde_json::to_value(state.snapshot()).unwrap();
        assert_eq!(json["automaticUpdate"]["error"], "upload failed");
        state.finish_automatic_update(Instant::now(), &status, &Ok(()));
        assert_eq!(state.snapshot().automatic_update.unwrap().error, None);
    }

    #[test]
    fn a_check_result_carries_the_stored_update_failure() {
        let state = State::default();
        let status = Status {
            installed_hash: Some("a".repeat(64)),
            bundled_hash: Some("b".repeat(64)),
            ..Status::default()
        };
        state.finish_automatic_update(Instant::now(), &status, &Err("upload failed".into()));
        let checked = state.checked(1, Ok(Status::default())).unwrap();
        assert_eq!(
            checked.automatic_update.unwrap().error.as_deref(),
            Some("upload failed")
        );
        assert_eq!(state.snapshot().checked_at, checked.checked_at);
        assert!(state.checked(1, Err("offline".into())).is_err());
    }

    #[test]
    fn an_update_skipped_for_a_build_in_use_tries_again_once_it_may_be_idle() {
        let state = State::default();
        let idle = Status {
            installed: Some(true),
            installed_variant: Some(Variant::Full),
            bundled_variant: Variant::Full,
            build_relation: BuildRelation::Different,
            installed_running: Some(false),
            ..Status::default()
        };
        let start = Instant::now();
        state.record(1, &Ok(idle.clone()));
        assert!(idle.automatic_update_available());
        assert!(state.claim_automatic_update(start));
        // The update's own recheck catches the installed build in use.
        state.record(
            1,
            &Ok(Status {
                installed_running: Some(true),
                ..idle.clone()
            }),
        );
        state.skipped_automatic_update();
        state.finish_automatic_update(start, &idle, &Ok(()));
        assert!(!state.claim_automatic_update(start + IDLE_RECHECK - Duration::from_secs(1)));
        assert!(state.claim_automatic_update(start + IDLE_RECHECK));
        // A skip for any other reason keeps the usual cadence.
        let other = State::default();
        other.record(1, &Ok(Status::default()));
        assert!(other.claim_automatic_update(start));
        other.skipped_automatic_update();
        other.finish_automatic_update(start, &Status::default(), &Ok(()));
        assert!(!other.claim_automatic_update(start + IDLE_RECHECK));
    }

    #[test]
    fn a_skipped_automatic_update_leaves_an_earlier_failure_in_place() {
        let state = State::default();
        let status = Status::default();
        let start = Instant::now();
        assert!(state.claim_automatic_update(start));
        state.finish_automatic_update(start, &status, &Err("upload failed".into()));
        let retry = start + AUTOMATIC_RETRY;
        assert!(state.claim_automatic_update(retry));
        state.skipped_automatic_update();
        state.finish_automatic_update(retry, &status, &Ok(()));
        let outcome = state.snapshot().automatic_update.unwrap();
        assert_eq!(outcome.error.as_deref(), Some("upload failed"));
        // A later real success still replaces it.
        let later = retry + AUTOMATIC_CADENCE;
        assert!(state.claim_automatic_update(later));
        state.finish_automatic_update(later, &status, &Ok(()));
        assert_eq!(state.snapshot().automatic_update.unwrap().error, None);
    }

    #[test]
    fn a_required_update_dropped_before_it_finishes_gives_the_claim_back() {
        let state = State::default();
        let now = Instant::now();
        assert_eq!(state.claim_required_update(now), Ok(()));
        let guard = state.required_guard();
        drop(guard);
        assert_eq!(state.required_update(), None);
        assert_eq!(state.claim_required_update(now), Ok(()));
        state.required_guard().finish(&Err("upload failed".into()));
        assert!(matches!(
            state.required_update(),
            Some(RequiredUpdate::Failed { .. })
        ));
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
        // Product inspection must finish its own runtime before the next idle
        // observation, rather than making this otherwise unused build busy.
        assert_eq!(
            lease.check_installation().await.unwrap().installed_running,
            Some(false)
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
        lease
            .execute(
                "printf 'unrelated' > \"$HOME/.local/bin/newport-browser\"",
                None,
            )
            .await
            .unwrap();
        let other = activate_variant_test(&connection).await;
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
