//! Explicit removal with durable service admission and reversible helper changes.
use super::{private_file, valid_hash, Installation};
use crate::deployment_protocol::{RemovalReceipt as Record, RemovalState as Phase};
use crate::{helper_transaction, installation_use::MaintenanceLease};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};
use uuid::Uuid;

const MAX_BYTES: u64 = 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    version: u32,
    records: Vec<Record>,
}

impl Journal {
    fn load(root: &Path) -> io::Result<Self> {
        let file = match private_file(&root.join("removals.json")) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(Self {
                    version: 1,
                    records: Vec::new(),
                });
            }
            Err(error) => return Err(error),
        };
        let mut bytes = Vec::new();
        file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(io::Error::other("Needle removal journal exceeds limit"));
        }
        let journal: Self = serde_json::from_slice(&bytes)?;
        if journal.version != 1 || journal.records.len() > 1000 {
            return Err(io::Error::other("Unsupported Needle removal journal"));
        }
        let mut ids = std::collections::HashSet::new();
        for (index, record) in journal.records.iter().enumerate() {
            valid_hash(&record.build)?;
            if record.operation_id.is_nil()
                || !ids.insert(record.operation_id)
                || (index + 1 < journal.records.len() && record.state != Phase::Restored)
            {
                return Err(io::Error::other("Invalid Needle removal journal"));
            }
        }
        Ok(journal)
    }
    fn save(&self, root: &Path) -> io::Result<()> {
        let bytes = serde_json::to_vec(self)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(io::Error::other("Needle removal journal exceeds limit"));
        }
        let mut file = tempfile::NamedTempFile::new_in(root)?;
        file.write_all(&bytes)?;
        file.as_file().sync_all()?;
        file.persist(root.join("removals.json"))
            .map_err(io::Error::other)?;
        fs::File::open(root)?.sync_all()
    }
}

fn root(home: &Path) -> PathBuf {
    home.join(".local/share/newport/needle")
}

pub(super) fn require_ready(home: &Path) -> io::Result<()> {
    let journal = Journal::load(&root(home))?;
    if journal
        .records
        .last()
        .is_some_and(|r| r.state != Phase::Restored)
    {
        return Err(io::Error::other(
            "Needle removal is pending or completed. Explicitly restore the recorded removal before starting services or installing again.",
        ));
    }
    Ok(())
}

#[derive(PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Compatibility {
    version: u32,
    build: String,
    removal_admission: u32,
}

/// Only the running, checksum-verified candidate can record its own support.
pub(super) fn record_compatibility(installation: &Installation, hash: &str) -> io::Result<()> {
    let path = installation.verify(hash)?;
    let directory = path.parent().unwrap();
    let value = Compatibility {
        version: 1,
        build: hash.into(),
        // v2 admission uses a persistent home lock. Older removers reject this
        // build instead of assuming their /tmp lock excludes its services.
        removal_admission: 2,
    };
    let destination = directory.join("lifecycle.json");
    match private_file(&destination) {
        Ok(file) => {
            let mut bytes = Vec::new();
            file.take(513).read_to_end(&mut bytes)?;
            if bytes.len() <= 512
                && serde_json::from_slice::<Compatibility>(&bytes)
                    .is_ok_and(|existing| existing == value)
            {
                return Ok(());
            }
            return Err(io::Error::other(
                "Existing Needle lifecycle metadata is unrecognized; preserving it",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => (),
        Err(error) => return Err(error),
    }
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    serde_json::to_writer(file.as_file_mut(), &value)?;
    file.as_file().sync_all()?;
    file.persist_noclobber(destination)
        .map_err(io::Error::other)?;
    fs::File::open(directory)?.sync_all()
}

fn compatible(installation: &Installation) -> io::Result<()> {
    // Retained builds may be launched by another desktop. A build that lacks
    // durable admission cannot be treated as idle merely because the lease is free.
    let mut count = 0;
    super::check_directory(&installation.root.join("builds"))?;
    for entry in fs::read_dir(installation.root.join("builds"))? {
        count += 1;
        if count > 128 {
            return Err(io::Error::other(
                "Too many retained builds to verify removal compatibility",
            ));
        }
        let entry = entry?;
        super::check_directory(&entry.path())?;
        let name = entry.file_name();
        let hash = name
            .to_str()
            .ok_or_else(|| io::Error::other("Unknown retained Needle build"))?;
        installation.verify(hash)?;
        let file = private_file(&entry.path().join("lifecycle.json")).map_err(|_| io::Error::other("A retained Needle build cannot coordinate removal. Preserve the installation until older-client compatibility is resolved."))?;
        let mut bytes = Vec::new();
        file.take(513).read_to_end(&mut bytes)?;
        if bytes.len() > 512 {
            return Err(io::Error::other("Invalid Needle lifecycle metadata"));
        }
        let value: Compatibility = serde_json::from_slice(&bytes)?;
        // A retained v1 executable could create its old /tmp lock after our
        // legacy-path probe. Refuse removal rather than race that service.
        if value.version != 1 || value.build != hash || value.removal_admission != 2 {
            return Err(io::Error::other("A retained Needle build uses older installation locks. Preserve it until older-client compatibility is resolved."));
        }
    }
    Ok(())
}

pub(super) fn observe(installation: &Installation, id: Uuid) -> io::Result<Option<Record>> {
    Ok(Journal::load(&installation.root)?
        .records
        .into_iter()
        .find(|r| r.operation_id == id))
}

pub(super) fn remove(
    installation: &Installation,
    home: &Path,
    build: &str,
    id: Uuid,
) -> io::Result<Record> {
    remove_with(installation, home, build, id, |_| Ok(()))
}

fn remove_with(
    installation: &Installation,
    home: &Path,
    build: &str,
    id: Uuid,
    mut checkpoint: impl FnMut(&str) -> io::Result<()>,
) -> io::Result<Record> {
    valid_hash(build)?;
    if id.is_nil() || crate::runtime::build_identity()? != build {
        return Err(io::Error::other("Invalid Needle removal identity"));
    }
    let mut journal = Journal::load(&installation.root)?;
    let existing = journal.records.iter().position(|r| r.operation_id == id);
    if let Some(index) = existing {
        let record = &journal.records[index];
        if record.build != build {
            return Err(io::Error::other("Removal operation parameters changed"));
        }
        if matches!(record.state, Phase::Removed | Phase::Restored) {
            return Ok(record.clone());
        }
        if record.state != Phase::Removing {
            return Err(io::Error::other("Continue restoring this removal"));
        }
    } else {
        require_ready(home)?;
        if journal.records.len() == 1000 {
            return Err(io::Error::other(
                "Needle removal journal is full; preserve its history",
            ));
        }
    }
    super::operations::require_settled(installation)?;
    let lease = MaintenanceLease::acquire(home)?;
    compatible(installation)?;
    installation.verify(build)?;
    let active = installation.inspect_current(false)?;
    if active != build && !(existing.is_some() && active == "missing") {
        return Err(io::Error::other(
            "Needle changed since removal was requested",
        ));
    }
    let outcome = helper_transaction::removal_outcome(home, id)?;
    if existing.is_none() && outcome.is_some() {
        return Err(io::Error::other(
            "Removal ID belongs to another helper operation",
        ));
    }
    let planned = if outcome.is_none() {
        Some(helper_transaction::prepare_removal(home, id)?)
    } else {
        None
    };
    let index = match existing {
        Some(index) => index,
        None => {
            journal.records.push(Record {
                operation_id: id,
                build: build.into(),
                state: Phase::Removing,
            });
            journal.save(&installation.root)?;
            checkpoint("intent")?;
            journal.records.len() - 1
        }
    };
    if let Some(planned) = planned {
        planned.apply_removal(&lease)?;
    } else if outcome != Some(helper_transaction::State::Applied) {
        return Err(io::Error::other(
            "Helper removal was interrupted; restore this removal before retrying",
        ));
    }
    checkpoint("helpers")?;
    lease.validate()?;
    let active = installation.inspect_current(false)?;
    if active == build {
        installation.replace("missing")?;
    } else if active != "missing" {
        return Err(io::Error::other("Needle changed during removal"));
    }
    checkpoint("entry")?;
    journal.records[index].state = Phase::Removed;
    journal.save(&installation.root)?;
    Ok(journal.records[index].clone())
}

pub(super) fn restore(installation: &Installation, home: &Path, id: Uuid) -> io::Result<Record> {
    let mut journal = Journal::load(&installation.root)?;
    let index = journal
        .records
        .iter()
        .position(|r| r.operation_id == id)
        .ok_or_else(|| io::Error::other("Needle removal operation not found"))?;
    if journal.records[index].state == Phase::Restored {
        return Ok(journal.records[index].clone());
    }
    if index + 1 != journal.records.len() {
        return Err(io::Error::other("A later Needle removal exists"));
    }
    super::operations::require_settled(installation)?;
    let lease = MaintenanceLease::acquire(home)?;
    compatible(installation)?;
    let build = journal.records[index].build.clone();
    installation.verify(&build)?;
    let active = installation.inspect_current(false)?;
    if active != "missing" && active != build {
        return Err(io::Error::other("Needle changed after removal"));
    }
    journal.records[index].state = Phase::Restoring;
    journal.save(&installation.root)?;
    lease.validate()?;
    if helper_transaction::removal_outcome(home, id)?.is_some() {
        helper_transaction::Transaction::recover(home, id)?;
    }
    lease.validate()?;
    if installation.inspect_current(false)? == "missing" {
        installation.replace(&build)?;
    } else if installation.inspect_current(false)? != build {
        return Err(io::Error::other("Needle changed during restoration"));
    }
    journal.records[index].state = Phase::Restored;
    journal.save(&installation.root)?;
    Ok(journal.records[index].clone())
}

/// Review only an already-started restoration of this exact retained build.
/// The caller holds the activation lock, including while recording keep choices.
pub(super) fn review(
    installation: &Installation,
    home: &Path,
    id: Uuid,
    build: &str,
    keep: Option<(usize, &str)>,
) -> io::Result<crate::deployment_protocol::HelperReview> {
    valid_hash(build)?;
    if id.is_nil() || crate::runtime::build_identity()? != build {
        return Err(io::Error::other("Invalid Needle removal identity"));
    }
    let journal = Journal::load(&installation.root)?;
    let record = journal
        .records
        .last()
        .ok_or_else(|| io::Error::other("Needle removal not found"))?;
    if record.operation_id != id || record.build != build || record.state != Phase::Restoring {
        return Err(io::Error::other(
            "Only the current Needle removal restoration can be reviewed",
        ));
    }
    super::operations::require_settled(installation)?;
    installation.verify(build)?;
    let active = installation.inspect_current(false)?;
    if active != "missing" && active != build {
        return Err(io::Error::other("Needle changed after removal"));
    }
    if helper_transaction::removal_outcome(home, id)?.is_none() {
        return Err(io::Error::other("Removal has no helper changes to review"));
    }
    helper_transaction::Transaction::review(home, id, keep)
}

/// The activation lock serializes this decision with a possibly delayed remove.
/// If nothing was dispatched, a durable tombstone prevents that ID from removing
/// files later. Cancellation changes only the journal, so live clients may stay.
pub(super) fn restore_or_cancel(
    installation: &Installation,
    home: &Path,
    id: Uuid,
    build: &str,
) -> io::Result<Record> {
    valid_hash(build)?;
    if id.is_nil() || crate::runtime::build_identity()? != build {
        return Err(io::Error::other("Invalid Needle removal identity"));
    }
    let mut journal = Journal::load(&installation.root)?;
    if let Some(record) = journal.records.iter().find(|r| r.operation_id == id) {
        if record.build != build {
            return Err(io::Error::other("Removal operation parameters changed"));
        }
        return restore(installation, home, id);
    }
    require_ready(home)?;
    if journal.records.len() >= 1000 {
        return Err(io::Error::other(
            "Needle removal journal is full; preserve its history",
        ));
    }
    installation.verify(build)?;
    let record = Record {
        operation_id: id,
        build: build.into(),
        state: Phase::Restored,
    };
    journal.records.push(record.clone());
    journal.save(&installation.root)?;
    Ok(record)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        process::{Child, Command, Stdio},
        time::{Duration, Instant},
    };

    #[test]
    fn removal_refuses_retained_builds_with_the_old_lock_scheme() {
        let home = tempfile::tempdir().unwrap();
        let install = Installation::open(home.path()).unwrap();
        let binary = std::env::current_exe().unwrap();
        let hash = super::super::checksum(&binary).unwrap();
        install.retain(&binary, &hash).unwrap();
        record_compatibility(&install, &hash).unwrap();
        compatible(&install).unwrap();
        let metadata = install.binary(&hash).with_file_name("lifecycle.json");
        let mut old: Compatibility = serde_json::from_slice(&fs::read(&metadata).unwrap()).unwrap();
        old.removal_admission = 1;
        fs::write(&metadata, serde_json::to_vec(&old).unwrap()).unwrap();
        assert!(compatible(&install)
            .unwrap_err()
            .to_string()
            .contains("older installation locks"));
        assert!(install.binary(&hash).exists());
        assert!(!install.root.join("removals.json").exists());
    }

    struct ChildGuard(Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[test]
    #[ignore = "subprocess helper invoked by killed_removal_reconciles_every_durable_boundary"]
    fn removal_crash_child() {
        let home = PathBuf::from(std::env::var_os("NEWPORT_TEST_REMOVAL_HOME").unwrap());
        let stage = std::env::var("NEWPORT_TEST_REMOVAL_STAGE").unwrap();
        let id = Uuid::parse_str(&std::env::var("NEWPORT_TEST_REMOVAL_ID").unwrap()).unwrap();
        let installation = Installation::open(&home).unwrap();
        let hash = crate::runtime::build_identity().unwrap();
        installation
            .retain(&std::env::current_exe().unwrap(), &hash)
            .unwrap();
        record_compatibility(&installation, &hash).unwrap();
        installation.replace(&hash).unwrap();
        let path = home.join(".bashrc");
        fs::write(&path, "original settings").unwrap();
        let mut tx = helper_transaction::Transaction::open(&home, Uuid::new_v4()).unwrap();
        let before = tx.read(&path).unwrap();
        let after = tx.file(b"installed settings", 0o600).unwrap();
        tx.plan(&path, before, after).unwrap();
        tx.apply().unwrap();
        remove_with(&installation, &home, &hash, id, |at| {
            if at == stage {
                fs::write(home.join("checkpoint"), at)?;
                loop {
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
            Ok(())
        })
        .unwrap();
        panic!("checkpoint was not reached");
    }

    #[test]
    fn killed_removal_reconciles_every_durable_boundary() {
        for stage in ["intent", "helpers", "entry"] {
            let home = tempfile::tempdir().unwrap();
            let id = Uuid::new_v4();
            let mut child = ChildGuard(
                Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "deployment::removal::tests::removal_crash_child",
                        "--ignored",
                        "--nocapture",
                    ])
                    .env("NEWPORT_TEST_REMOVAL_HOME", home.path())
                    .env("NEWPORT_TEST_REMOVAL_STAGE", stage)
                    .env("NEWPORT_TEST_REMOVAL_ID", id.to_string())
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::inherit())
                    .spawn()
                    .unwrap(),
            );
            // A debug Linux test executable is much larger than the shipped
            // agent. Setup copies, fsyncs and hashes it repeatedly before the
            // durable checkpoint; allow cold shared CI disks to finish.
            let deadline = Instant::now() + Duration::from_secs(120);
            while !home.path().join("checkpoint").exists() {
                assert!(
                    child.0.try_wait().unwrap().is_none(),
                    "child exited before {stage}"
                );
                assert!(Instant::now() < deadline, "checkpoint timed out: {stage}");
                std::thread::sleep(Duration::from_millis(10));
            }
            child.0.kill().unwrap();
            child.0.wait().unwrap();
            assert!(require_ready(home.path()).is_err());
            let installation = Installation::inspect(home.path()).unwrap();
            let hash = crate::runtime::build_identity().unwrap();
            assert_eq!(
                observe(&installation, id).unwrap().unwrap().state,
                Phase::Removing
            );
            assert_eq!(
                remove(&installation, home.path(), &hash, id).unwrap().state,
                Phase::Removed
            );
            assert_eq!(
                fs::read_to_string(home.path().join(".bashrc")).unwrap(),
                "original settings"
            );
            assert_eq!(installation.inspect_current(false).unwrap(), "missing");
            assert!(require_ready(home.path()).is_err());
            assert_eq!(
                restore(&installation, home.path(), id).unwrap().state,
                Phase::Restored
            );
            assert_eq!(
                fs::read_to_string(home.path().join(".bashrc")).unwrap(),
                "installed settings"
            );
            assert_eq!(installation.inspect_current(false).unwrap(), hash);
            require_ready(home.path()).unwrap();
        }
    }
}
