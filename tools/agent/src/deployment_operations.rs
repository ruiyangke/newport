//! Durable activation receipts under the installation's exclusive lock.
use super::Installation;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Read},
    path::Path,
};
use uuid::Uuid;

const MAX_BYTES: u64 = 1024 * 1024;
const MAX_OPERATIONS: usize = 1000;

use crate::deployment_protocol::{Intent, Kind, Observation, Receipt, State};
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    version: u32,
    operations: Vec<Receipt>,
}
impl Journal {
    fn load(root: &Path) -> io::Result<Self> {
        let path = root.join("operations.json");
        let file = match super::private_file(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Ok(Self {
                    version: 1,
                    operations: Vec::new(),
                });
            }
            Err(e) => return Err(e),
        };
        let mut bytes = Vec::new();
        file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(io::Error::other("Needle operation journal exceeds limit"));
        }
        let journal: Self = serde_json::from_slice(&bytes)?;
        let mut ids = std::collections::HashSet::new();
        if journal.version != 1 || journal.operations.len() > MAX_OPERATIONS {
            return Err(io::Error::other("Unsupported Needle operation journal"));
        }
        for r in &journal.operations {
            super::valid_hash(&r.intent.candidate)?;
            if r.intent.previous != "missing" {
                super::valid_hash(&r.intent.previous)?;
            }
            if (r.intent.kind == Kind::Helpers
                && (!r.intent.integrations || r.intent.candidate != r.intent.previous))
                || r.intent.operation_id.is_nil()
                || !ids.insert(r.intent.operation_id)
                || r.error.as_ref().is_some_and(|s| s.len() > 4096)
            {
                return Err(io::Error::other("Invalid Needle operation receipt"));
            }
        }
        Ok(journal)
    }
    fn save(&self, root: &Path) -> io::Result<()> {
        let bytes = serde_json::to_vec(self)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(io::Error::other("Needle operation journal exceeds limit"));
        }
        let mut temporary = tempfile::NamedTempFile::new_in(root)?;
        use io::Write;
        temporary.write_all(&bytes)?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(root.join("operations.json"))
            .map_err(io::Error::other)?;
        fs::File::open(root)?.sync_all()
    }
    fn update(
        &mut self,
        root: &Path,
        index: usize,
        state: State,
        error: Option<String>,
    ) -> io::Result<()> {
        self.operations[index].state = state;
        self.operations[index].error = error.map(|s| s.chars().take(1000).collect());
        self.save(root)
    }
    fn available(&self) -> io::Result<()> {
        if self.operations.iter().any(|r| r.state.unsettled()) {
            return Err(io::Error::other(
                "A Needle activation needs reconciliation before another deployment",
            ));
        }
        Ok(())
    }
}
pub(super) fn require_settled(installation: &Installation) -> io::Result<()> {
    Journal::load(&installation.root)?.available()
}
pub(super) fn helper_owners(
    installation: &Installation,
) -> io::Result<std::collections::HashSet<Uuid>> {
    helper_owners_at(&installation.root)
}
pub(super) fn helper_owners_at(root: &Path) -> io::Result<std::collections::HashSet<Uuid>> {
    Ok(Journal::load(root)?
        .operations
        .into_iter()
        .map(|record| record.intent.operation_id)
        .collect())
}
pub(super) fn observe(installation: &Installation, id: Uuid) -> io::Result<Observation> {
    let receipt = Journal::load(&installation.root)?
        .operations
        .into_iter()
        .find(|r| r.intent.operation_id == id);
    let active_build = installation.current()?;
    let activation_observed = receipt
        .as_ref()
        .is_some_and(|r| active_build == r.intent.candidate);
    Ok(Observation {
        receipt,
        active_build,
        activation_observed,
    })
}
/// Reconcile metadata only. Never re-run activation or partially applied helpers.
pub(super) fn reconcile(installation: &Installation, id: Uuid) -> io::Result<Observation> {
    let mut journal = Journal::load(&installation.root)?;
    let index = journal
        .operations
        .iter()
        .position(|r| r.intent.operation_id == id)
        .ok_or_else(|| io::Error::other("Needle operation not found"))?;
    let receipt = &journal.operations[index];
    if matches!(receipt.state, State::Prepared | State::Activated) {
        let active = installation.current()?;
        let helpers_applied = if receipt.intent.integrations {
            let home = installation
                .active
                .parent()
                .and_then(Path::parent)
                .and_then(Path::parent)
                .ok_or_else(|| io::Error::other("Invalid installation path"))?;
            crate::helper_transaction::outcome(home, id)?
                == Some(crate::helper_transaction::State::Applied)
        } else {
            true
        };
        let proven = helpers_applied && active == receipt.intent.candidate;
        journal.update(&installation.root, index,
            if proven {State::Succeeded} else {State::NeedsResolution},
            (!proven).then(|| "Activation was interrupted; inspect the installation and helper changes before another deployment".into()))?;
    }
    observe(installation, id)
}
/// Restore an explicitly selected operation, with a durable recovery phase.
pub(super) fn review_helpers(
    installation: &Installation,
    id: Uuid,
    keep: Option<(usize, &str)>,
) -> io::Result<crate::deployment_protocol::HelperReview> {
    let journal = Journal::load(&installation.root)?;
    let index = journal
        .operations
        .iter()
        .position(|r| r.intent.operation_id == id)
        .ok_or_else(|| io::Error::other("Needle operation not found"))?;
    let receipt = &journal.operations[index];
    if receipt.state != State::RollingBack || receipt.rollback_helpers != Some(true) {
        return Err(io::Error::other(
            "Start restoration before reviewing helper conflicts",
        ));
    }
    if journal.operations[index + 1..]
        .iter()
        .any(|r| r.state != State::RolledBack)
    {
        return Err(io::Error::other(
            "A later Needle deployment exists; review cannot change its ownership",
        ));
    }
    let active = installation.current()?;
    if active != receipt.intent.candidate && active != receipt.intent.previous {
        return Err(io::Error::other("Needle changed after this operation"));
    }
    let home = installation
        .active
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .ok_or_else(|| io::Error::other("Invalid installation path"))?;
    crate::helper_transaction::Transaction::review(home, id, keep)
}

pub(super) fn recover(installation: &Installation, id: Uuid) -> io::Result<Observation> {
    let mut journal = Journal::load(&installation.root)?;
    let index = journal
        .operations
        .iter()
        .position(|r| r.intent.operation_id == id)
        .ok_or_else(|| io::Error::other("Needle operation not found"))?;
    if journal.operations[index].state == State::RolledBack {
        return observe(installation, id);
    }
    // Never undo a newer deployment, even if it selected the same executable.
    if journal.operations[index + 1..]
        .iter()
        .any(|r| r.state != State::RolledBack)
    {
        return Err(io::Error::other(
            "A later Needle deployment exists; refusing to undo it",
        ));
    }
    let receipt = journal.operations[index].clone();
    let active = installation.current()?;
    if active != receipt.intent.candidate && active != receipt.intent.previous {
        return Err(io::Error::other(
            "Needle changed after this operation; refusing to replace another installation",
        ));
    }
    if receipt.intent.previous != "missing" {
        installation.verify(&receipt.intent.previous)?;
    }
    let helpers = receipt
        .rollback_helpers
        .unwrap_or(receipt.intent.integrations && receipt.state != State::Prepared);
    journal.operations[index].rollback_helpers = Some(helpers);
    journal.update(&installation.root, index, State::RollingBack, None)?;
    let result = (|| {
        if helpers {
            let home = installation
                .active
                .parent()
                .and_then(Path::parent)
                .and_then(Path::parent)
                .ok_or_else(|| io::Error::other("Invalid installation path"))?;
            crate::helper_transaction::Transaction::recover(home, id)?;
        }
        if receipt.intent.kind.is_deployment() && active != receipt.intent.previous {
            installation.replace(&receipt.intent.previous)?;
        }
        Ok::<_, io::Error>(())
    })();
    match result {
        Ok(()) => journal.update(&installation.root, index, State::RolledBack, None)?,
        Err(error) => journal.update(
            &installation.root,
            index,
            State::RollingBack,
            Some(error.to_string()),
        )?,
    }
    observe(installation, id)
}

pub(super) fn activate(
    installation: &Installation,
    intent: Intent,
    helpers: impl FnOnce() -> io::Result<Option<String>>,
) -> io::Result<Observation> {
    super::valid_hash(&intent.candidate)?;
    if intent.previous != "missing" {
        super::valid_hash(&intent.previous)?;
    }
    if intent.operation_id.is_nil() {
        return Err(io::Error::other("Invalid Needle operation ID"));
    }
    let mut journal = Journal::load(&installation.root)?;
    if let Some(existing) = journal
        .operations
        .iter()
        .find(|r| r.intent.operation_id == intent.operation_id)
    {
        if existing.intent != intent {
            return Err(io::Error::other(
                "Needle operation ID was already used for different parameters",
            ));
        }
        // A retry observes a durable result. It never repeats activation or helper writes.
        return observe(installation, intent.operation_id);
    }
    journal.available()?;
    if journal.operations.len() == MAX_OPERATIONS {
        return Err(io::Error::other(
            "Needle operation journal is full; retain receipts before clearing history",
        ));
    }
    if intent.kind == Kind::Helpers && (!intent.integrations || intent.candidate != intent.previous)
    {
        return Err(io::Error::other("Invalid helper-only operation"));
    }
    let current = installation.current()?;
    installation.verify(&intent.candidate)?;
    if current != intent.previous {
        return Err(io::Error::other(
            "Needle changed since inspection; inspect again before activating",
        ));
    }
    if !intent.integrations && current == "missing" && journal.operations.is_empty() {
        let home = installation
            .active
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .ok_or_else(|| io::Error::other("Invalid installation path"))?;
        crate::helper_transaction::initialize_ownership(home)?;
    }
    let index = journal.operations.len();
    journal.operations.push(Receipt {
        intent: intent.clone(),
        state: State::Prepared,
        error: None,
        rollback_helpers: None,
    });
    journal.save(&installation.root)?;
    // Any error after this point may follow a successful entry-point rename.
    // Leave the last durable phase intact so callers can inspect the outcome.
    if intent.kind.is_deployment() {
        installation.switch(&intent.candidate, &intent.previous)?;
    }
    journal.update(&installation.root, index, State::Activated, None)?;
    let mut notice = None;
    if intent.integrations {
        let result = helpers();
        if let Ok(value) = &result {
            notice = value.clone();
        }
        if let Err(error) = result {
            let rollback = if intent.kind.is_deployment() {
                installation.replace(&intent.previous)
            } else {
                Ok(())
            };
            // A cancelled plan or confirmed rollback has no outstanding helper
            // changes. Do not strand a settled installation behind a review.
            let home = installation
                .active
                .parent()
                .and_then(Path::parent)
                .and_then(Path::parent);
            let helpers_restored = home.is_some_and(|home| {
                matches!(
                    crate::helper_transaction::outcome(home, intent.operation_id),
                    Ok(Some(crate::helper_transaction::State::RolledBack))
                )
            });
            if helpers_restored && rollback.is_ok() {
                journal.update(
                    &installation.root,
                    index,
                    State::RolledBack,
                    Some(format!(
                        "Shell setup failed; no helper changes remain: {error}"
                    )),
                )?;
                return observe(installation, intent.operation_id);
            }
            let message = match rollback {
                Ok(()) if intent.kind == Kind::Helpers => format!(
                    "Shell setup failed; Needle was not replaced: {error}. Helper changes require review."
                ),
                Ok(()) => format!(
                    "Integration setup failed; previous Needle build restored: {error}. Helper changes require review."
                ),
                Err(rollback) => format!(
                    "Integration setup failed: {error}; rollback could not be confirmed: {rollback}"
                ),
            };
            // Binary rollback does not prove rollback of partial shell/helper edits.
            journal.update(
                &installation.root,
                index,
                State::NeedsResolution,
                Some(message),
            )?;
            return observe(installation, intent.operation_id);
        }
    }
    journal.update(&installation.root, index, State::Succeeded, notice)?;
    observe(installation, intent.operation_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn candidate(install: &Installation, bytes: &[u8]) -> String {
        let file = tempfile::NamedTempFile::new().unwrap();
        fs::write(file.path(), bytes).unwrap();
        let hash = super::super::checksum(file.path()).unwrap();
        install.retain(file.path(), &hash).unwrap();
        hash
    }
    fn intent(candidate: String, integrations: bool) -> Intent {
        Intent {
            kind: Kind::Deployment,
            operation_id: Uuid::new_v4(),
            candidate,
            previous: "missing".into(),
            integrations,
        }
    }
    #[test]
    fn retry_preserves_receipt_and_never_reapplies_helpers_or_an_old_build() {
        let home = tempfile::tempdir().unwrap();
        let install = Installation::open(home.path()).unwrap();
        let intent = intent(candidate(&install, b"first"), true);
        let result = activate(&install, intent.clone(), || Ok(None)).unwrap();
        assert_eq!(result.receipt.unwrap().state, State::Succeeded);
        let other = candidate(&install, b"other");
        install.activate(&other, &intent.candidate).unwrap();
        let result = activate(&install, intent.clone(), || {
            panic!("duplicate helper execution")
        })
        .unwrap();
        assert_eq!(result.active_build, other);
        assert!(!result.activation_observed);
        assert_eq!(result.receipt.unwrap().state, State::Succeeded);
        let mut changed = intent;
        changed.integrations = false;
        assert!(activate(&install, changed, || Ok(None)).is_err());
    }
    #[test]
    fn reconciliation_requires_evidence_and_does_not_replay_side_effects() {
        let home = tempfile::tempdir().unwrap();
        let install = Installation::open(home.path()).unwrap();
        let intent = intent(candidate(&install, b"candidate"), false);
        let mut journal = Journal {
            version: 1,
            operations: vec![Receipt {
                intent: intent.clone(),
                state: State::Prepared,
                error: None,
                rollback_helpers: None,
            }],
        };
        journal.save(&install.root).unwrap();
        assert!(install.activate(&intent.candidate, "missing").is_err());
        // Crash after entry-point rename but before the next journal fsync.
        install.replace(&intent.candidate).unwrap();
        let result = reconcile(&install, intent.operation_id).unwrap();
        assert_eq!(result.receipt.unwrap().state, State::Succeeded);
        journal.operations[0].intent.integrations = true;
        journal.operations[0].state = State::Activated;
        journal.save(&install.root).unwrap();
        assert_eq!(
            reconcile(&install, intent.operation_id)
                .unwrap()
                .receipt
                .unwrap()
                .state,
            State::NeedsResolution
        );
        assert!(require_settled(&install).is_err());
    }
    #[test]
    fn completed_helper_receipt_proves_interrupted_activation_without_replay() {
        use crate::helper_transaction::{Snapshot, Transaction};
        let home = tempfile::tempdir().unwrap();
        let install = Installation::open(home.path()).unwrap();
        let intent = intent(candidate(&install, b"candidate"), true);
        let journal = Journal {
            version: 1,
            operations: vec![Receipt {
                intent: intent.clone(),
                state: State::Activated,
                error: None,
                rollback_helpers: None,
            }],
        };
        journal.save(&install.root).unwrap();
        install.replace(&intent.candidate).unwrap();
        let path = home.path().join("helper");
        let mut helpers = Transaction::open(home.path(), intent.operation_id).unwrap();
        helpers
            .plan(
                &path,
                Snapshot::Missing,
                Snapshot::Link("newport-agent".into()),
            )
            .unwrap();
        helpers.apply().unwrap();
        // Simulate a lost final activation receipt. A later user edit must not be
        // overwritten while recovering the historical helper outcome.
        fs::remove_file(&path).unwrap();
        fs::write(&path, b"later user edit").unwrap();
        let result = reconcile(&install, intent.operation_id).unwrap();
        assert_eq!(result.receipt.unwrap().state, State::Succeeded);
        assert_eq!(fs::read(path).unwrap(), b"later user edit");
        assert!(require_settled(&install).is_ok());
    }
    #[test]
    fn explicit_recovery_preserves_conflicts_and_retries_without_replaying_installation() {
        use crate::helper_transaction::{Snapshot, Transaction};
        let home = tempfile::tempdir().unwrap();
        let install = Installation::open(home.path()).unwrap();
        let before = candidate(&install, b"previous");
        install.activate(&before, "missing").unwrap();
        let mut intent = intent(candidate(&install, b"new"), true);
        intent.previous = before.clone();
        let path = home.path().join("helper");
        activate(&install, intent.clone(), || {
            let mut tx = Transaction::open(home.path(), intent.operation_id)?;
            tx.plan(
                &path,
                Snapshot::Missing,
                Snapshot::Link("newport-agent".into()),
            )?;
            tx.apply().map(|_| None)
        })
        .unwrap();
        fs::remove_file(&path).unwrap();
        fs::write(&path, b"user edit").unwrap();
        let result = recover(&install, intent.operation_id).unwrap();
        assert_eq!(result.receipt.unwrap().state, State::RollingBack);
        assert_eq!(result.active_build, intent.candidate);
        assert_eq!(fs::read(&path).unwrap(), b"user edit");
        assert!(require_settled(&install).is_err());
        // The user resolves the conflicting file; retry only finishes recovery.
        fs::remove_file(&path).unwrap();
        let result = recover(&install, intent.operation_id).unwrap();
        assert_eq!(result.receipt.unwrap().state, State::RolledBack);
        assert_eq!(result.active_build, before);
        assert!(require_settled(&install).is_ok());
        let next = candidate(&install, b"subsequent deployment");
        install.activate(&next, &before).unwrap();
        let result = recover(&install, intent.operation_id).unwrap();
        assert_eq!(result.active_build, next);
        assert_eq!(result.receipt.unwrap().state, State::RolledBack);
    }
    #[test]
    fn recovery_of_prepared_intent_needs_no_helper_record_and_retains_its_decision() {
        let home = tempfile::tempdir().unwrap();
        let install = Installation::open(home.path()).unwrap();
        let intent = intent(candidate(&install, b"candidate"), true);
        let mut journal = Journal {
            version: 1,
            operations: vec![Receipt {
                intent: intent.clone(),
                state: State::RollingBack,
                error: None,
                rollback_helpers: Some(false),
            }],
        };
        // Models interruption after persisting the recovery of Prepared, before
        // undoing the entry switch. No helper process could have been started.
        journal.save(&install.root).unwrap();
        install.replace(&intent.candidate).unwrap();
        let result = recover(&install, intent.operation_id).unwrap();
        assert_eq!(result.active_build, "missing");
        assert_eq!(result.receipt.unwrap().state, State::RolledBack);
        journal.operations[0].state = State::Prepared;
        journal.operations[0].rollback_helpers = None;
        journal.save(&install.root).unwrap();
        assert_eq!(
            recover(&install, intent.operation_id)
                .unwrap()
                .receipt
                .unwrap()
                .state,
            State::RolledBack
        );
    }
    #[test]
    fn recovery_refuses_newer_recorded_deployment_even_when_hash_matches() {
        let home = tempfile::tempdir().unwrap();
        let install = Installation::open(home.path()).unwrap();
        let intent = intent(candidate(&install, b"candidate"), false);
        activate(&install, intent.clone(), || Ok(None)).unwrap();
        activate(
            &install,
            Intent {
                operation_id: Uuid::new_v4(),
                previous: intent.candidate.clone(),
                ..intent.clone()
            },
            || Ok(None),
        )
        .unwrap();
        assert!(recover(&install, intent.operation_id).is_err());
        assert_eq!(install.current().unwrap(), intent.candidate);
    }
    #[test]
    fn partial_helper_failure_preserves_binary_rollback_and_blocks_conflicting_deployment() {
        let home = tempfile::tempdir().unwrap();
        let install = Installation::open(home.path()).unwrap();
        let before = candidate(&install, b"before");
        install.activate(&before, "missing").unwrap();
        let mut intent = intent(candidate(&install, b"after"), true);
        intent.previous = before.clone();
        let result = activate(&install, intent.clone(), || {
            Err(io::Error::other("partial helper failure"))
        })
        .unwrap();
        assert_eq!(result.active_build, before);
        assert_eq!(result.receipt.unwrap().state, State::NeedsResolution);
        assert!(install.activate(&intent.candidate, &before).is_err());
        assert!(activate(
            &install,
            Intent {
                operation_id: Uuid::new_v4(),
                ..intent
            },
            || Ok(None)
        )
        .is_err());
    }
    #[test]
    fn unknown_corrupt_and_symlinked_journals_are_preserved_and_block_writes() {
        use std::os::unix::fs::symlink;
        let home = tempfile::tempdir().unwrap();
        let install = Installation::open(home.path()).unwrap();
        let hash = candidate(&install, b"candidate");
        let path = install.root.join("operations.json");
        let other = home.path().join("unrelated");
        fs::write(&other, b"unrelated").unwrap();
        symlink(&other, &path).unwrap();
        assert!(install.activate(&hash, "missing").is_err());
        assert_eq!(fs::read(&other).unwrap(), b"unrelated");
        fs::remove_file(&path).unwrap();
        let mut file = tempfile::NamedTempFile::new_in(&install.root).unwrap();
        use io::Write;
        file.write_all(br#"{"version":2,"operations":[]}"#).unwrap();
        file.persist(&path).unwrap();
        assert!(install.activate(&hash, "missing").is_err());
        assert!(fs::read_to_string(&path).unwrap().contains("\"version\":2"));
        fs::write(&path, "corrupt").unwrap();
        assert!(install.activate(&hash, "missing").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"corrupt");
        assert_eq!(install.current().unwrap(), "missing");
    }
    #[test]
    #[ignore = "Child process of forced_exit_preserves_activation_and_never_replays_helpers"]
    fn crash_child() {
        let home = std::env::var_os("NEEDLE_JOURNAL_TEST_HOME").expect("child fixture HOME");
        let install = Installation::open(Path::new(&home)).unwrap();
        let intent: Intent =
            serde_json::from_slice(&fs::read(Path::new(&home).join("intent.json")).unwrap())
                .unwrap();
        activate(&install, intent, || {
            fs::write(Path::new(&home).join("ready"), b"activated").unwrap();
            std::thread::sleep(std::time::Duration::from_secs(30));
            Err(io::Error::other("child was not killed"))
        })
        .unwrap();
    }
    #[test]
    fn forced_exit_preserves_activation_and_never_replays_helpers() {
        use std::{
            process::{Command, Stdio},
            time::{Duration, Instant},
        };
        let home = tempfile::tempdir().unwrap();
        let install = Installation::open(home.path()).unwrap();
        let intent = intent(candidate(&install, b"candidate"), true);
        fs::write(
            home.path().join("intent.json"),
            serde_json::to_vec(&intent).unwrap(),
        )
        .unwrap();
        drop(install);
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "deployment::operations::tests::crash_child",
                "--ignored",
                "--nocapture",
            ])
            .env("NEEDLE_JOURNAL_TEST_HOME", home.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !home.path().join("ready").exists() && Instant::now() < deadline {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(home.path().join("ready").exists());
        let install = Installation::inspect(home.path()).unwrap();
        let result = observe(&install, intent.operation_id).unwrap();
        assert!(result.activation_observed);
        assert_eq!(result.receipt.unwrap().state, State::Activated);
        let result = activate(&install, intent.clone(), || {
            panic!("must not replay after crash")
        })
        .unwrap();
        assert_eq!(result.receipt.unwrap().state, State::Activated);
        assert_eq!(
            reconcile(&install, intent.operation_id)
                .unwrap()
                .receipt
                .unwrap()
                .state,
            State::NeedsResolution
        );
    }
}
