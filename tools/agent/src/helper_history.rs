//! Standalone shell ownership, observed without manufacturing deployment receipts.
use super::*;
use crate::deployment_protocol::{HelperOrigin, HelperState, StandaloneHelper};
use std::collections::HashSet;

struct Read {
    root: PathBuf,
    journal: Journal,
    lock: fs::File,
}

fn read(home: &Path) -> io::Result<Option<Read>> {
    let root = home.join(".local/share/newport/needle/helpers");
    match fs::symlink_metadata(&root) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
        Ok(metadata)
            if !metadata.is_dir()
                || metadata.uid() != unsafe { libc::geteuid() }
                || metadata.mode() & 0o077 != 0 =>
        {
            return Err(io::Error::other("Invalid Needle helper directory"));
        }
        _ => {}
    }
    // Unlike Transaction::open, this does not create directories, a lock, or a journal.
    let lock = private_file(&root.join("lock"))?;
    acquire_lock(&lock)?;
    let journal = Journal::load_existing(&root)?;
    Ok(Some(Read {
        root,
        journal,
        lock,
    }))
}

fn fingerprint(record: &Record) -> io::Result<String> {
    let changes: Vec<_> = record
        .changes
        .iter()
        .map(|change| {
            serde_json::json!({
                "path": change.path, "before": change.before, "after": change.after,
                "shell": change.shell,
            })
        })
        .collect();
    Ok(Transaction::hash(&serde_json::to_vec(
        &serde_json::json!({
        "id": record.id, "removal": record.removal, "origin":record.origin, "changes": changes,
        }),
    )?))
}

fn summary(journal: &Journal, index: usize) -> io::Result<StandaloneHelper> {
    let record = &journal.records[index];
    let state = match record.state {
        State::Prepared => HelperState::Prepared,
        State::Applied => HelperState::Applied,
        State::RollingBack => HelperState::RollingBack,
        State::RolledBack => HelperState::RolledBack,
        State::NeedsResolution => HelperState::NeedsResolution,
    };
    let restore_blocked = if record.state == State::RolledBack {
        Some("Shell changes have already been restored.".into())
    } else if record.origin != Some(Origin::Standalone) {
        Some("This operation has no recorded standalone provenance. Preserve its files until ownership is reviewed.".into())
    } else if journal.records[index + 1..].iter().any(|later| {
        later.state != State::RolledBack
            && later
                .changes
                .iter()
                .any(|new| record.changes.iter().any(|old| old.path == new.path))
    }) {
        Some("A later helper operation owns these paths. Restore that operation first.".into())
    } else {
        None
    };
    Ok(StandaloneHelper {
        operation_id: record.id,
        origin: if record.origin == Some(Origin::Standalone) {
            HelperOrigin::Standalone
        } else {
            HelperOrigin::Unattributed
        },
        state,
        file_count: record.changes.len(),
        fingerprint: fingerprint(record)?,
        restore_blocked,
    })
}

pub(crate) fn history(
    home: &Path,
    owned: &HashSet<Uuid>,
) -> io::Result<(bool, Vec<StandaloneHelper>)> {
    let Some(read) = read(home)? else {
        return Ok((false, Vec::new()));
    };
    let rows = read
        .journal
        .records
        .iter()
        .enumerate()
        .filter(|(_, record)| {
            !record.removal
                && record.origin != Some(Origin::Deployment)
                && !owned.contains(&record.id)
        })
        .map(|(index, _)| summary(&read.journal, index))
        .collect::<io::Result<_>>()?;
    Ok((true, rows))
}

fn selected(
    home: &Path,
    id: Uuid,
    expected: &str,
    owned: &HashSet<Uuid>,
) -> io::Result<(Read, usize)> {
    valid_hash(expected)?;
    let read = read(home)?.ok_or_else(|| io::Error::other("No helper ownership history"))?;
    let index = read
        .journal
        .records
        .iter()
        .position(|record| record.id == id)
        .ok_or_else(|| io::Error::other("Helper operation not found"))?;
    let record = &read.journal.records[index];
    if record.removal || owned.contains(&id) || record.origin != Some(Origin::Standalone) {
        return Err(io::Error::other("This is not a verified standalone helper operation. Use its recorded recovery operation or review its ownership."));
    }
    if fingerprint(record)? != expected {
        return Err(io::Error::other(
            "Helper ownership changed. Inspect the operation again.",
        ));
    }
    Ok((read, index))
}

pub(crate) fn recover_standalone(
    home: &Path,
    id: Uuid,
    expected: &str,
    owned: &HashSet<Uuid>,
) -> io::Result<StandaloneHelper> {
    let (read, index) = selected(home, id, expected, owned)?;
    let mut tx = Transaction {
        root: read.root,
        journal: read.journal,
        id,
        changes: Vec::new(),
        _lock: read.lock,
    };
    tx.restore_record(index)?;
    summary(&tx.journal, index)
}

pub(crate) fn review_standalone(
    home: &Path,
    id: Uuid,
    expected: &str,
    owned: &HashSet<Uuid>,
    keep: Option<(usize, &str)>,
) -> io::Result<crate::deployment_protocol::HelperReview> {
    let (read, _) = selected(home, id, expected, owned)?;
    // The caller holds the activation lock, so the operation cannot acquire a
    // deployment owner here. Legitimate helper actions never rewrite immutable
    // ownership; review rechecks later overlapping ownership under its own lock.
    drop(read);
    Transaction::review(home, id, keep)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(home: &Path, text: &str) -> Uuid {
        let id = Uuid::new_v4();
        let mut tx = Transaction::open(home, id).unwrap();
        let path = home.join("owned-file");
        let before = tx.read(&path).unwrap();
        let after = tx.file(text.as_bytes(), 0o600).unwrap();
        tx.plan(&path, before, after).unwrap();
        tx.apply_install(Origin::Standalone).unwrap();
        id
    }

    #[test]
    fn inspection_is_read_only_and_distinguishes_absent_from_lost_history() {
        let home = tempfile::tempdir().unwrap();
        let owned = HashSet::new();
        assert!(!history(home.path(), &owned).unwrap().0);
        assert_eq!(fs::read_dir(home.path()).unwrap().count(), 0);
        let id = apply(home.path(), "managed");
        let path = home
            .path()
            .join(".local/share/newport/needle/helpers/journal.json");
        let bytes = fs::read(&path).unwrap();
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        let (recorded, rows) = history(home.path(), &owned).unwrap();
        assert!(recorded);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].operation_id, id);
        assert_eq!(rows[0].state, HelperState::Applied);
        assert_eq!(rows[0].file_count, 1);
        assert!(rows[0].restore_blocked.is_none());
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
        fs::remove_file(&path).unwrap();
        assert!(history(home.path(), &owned).is_err());
        assert!(!path.exists());
    }

    #[test]
    fn fingerprint_and_owner_checks_precede_restoration_and_retries_are_idempotent() {
        let home = tempfile::tempdir().unwrap();
        fs::write(home.path().join("owned-file"), "original").unwrap();
        let id = apply(home.path(), "managed");
        let owned = HashSet::new();
        let row = history(home.path(), &owned).unwrap().1.remove(0);
        assert!(recover_standalone(home.path(), id, &"0".repeat(64), &owned).is_err());
        let deployment_owned = HashSet::from([id]);
        assert!(history(home.path(), &deployment_owned)
            .unwrap()
            .1
            .is_empty());
        assert!(recover_standalone(home.path(), id, &row.fingerprint, &deployment_owned).is_err());
        assert_eq!(
            fs::read_to_string(home.path().join("owned-file")).unwrap(),
            "managed"
        );
        let restored = recover_standalone(home.path(), id, &row.fingerprint, &owned).unwrap();
        assert_eq!(restored.state, HelperState::RolledBack);
        assert_eq!(restored.fingerprint, row.fingerprint);
        assert_eq!(
            fs::read_to_string(home.path().join("owned-file")).unwrap(),
            "original"
        );
        fs::write(home.path().join("owned-file"), "later personal edit").unwrap();
        recover_standalone(home.path(), id, &row.fingerprint, &owned).unwrap();
        assert_eq!(
            fs::read_to_string(home.path().join("owned-file")).unwrap(),
            "later personal edit"
        );
    }

    #[test]
    fn later_ownership_and_removal_records_cannot_be_bypassed() {
        let home = tempfile::tempdir().unwrap();
        let first = apply(home.path(), "first");
        let second = apply(home.path(), "second");
        let owned = HashSet::new();
        let rows = history(home.path(), &owned).unwrap().1;
        assert!(rows[0].restore_blocked.is_some());
        assert!(recover_standalone(home.path(), first, &rows[0].fingerprint, &owned).is_err());
        assert_eq!(
            fs::read_to_string(home.path().join("owned-file")).unwrap(),
            "second"
        );
        let mut read = read(home.path()).unwrap().unwrap();
        read.journal.records[1].removal = true;
        read.journal.records[1].origin = None;
        read.journal.save(&read.root).unwrap();
        drop(read);
        assert_eq!(history(home.path(), &owned).unwrap().1.len(), 1);
        assert!(recover_standalone(home.path(), second, &rows[1].fingerprint, &owned).is_err());
    }

    #[test]
    fn standalone_conflicts_use_the_existing_file_review_and_keep_guards() {
        let home = tempfile::tempdir().unwrap();
        let id = apply(home.path(), "managed");
        let owned = HashSet::new();
        let fingerprint = history(home.path(), &owned).unwrap().1[0]
            .fingerprint
            .clone();
        fs::write(home.path().join("owned-file"), "personal edit").unwrap();
        assert!(recover_standalone(home.path(), id, &fingerprint, &owned).is_err());
        let summary = history(home.path(), &owned).unwrap().1.remove(0);
        assert_eq!(summary.state, HelperState::NeedsResolution);
        assert_eq!(summary.fingerprint, fingerprint);
        let review = review_standalone(home.path(), id, &fingerprint, &owned, None).unwrap();
        let token = review.files[0].fingerprint.clone().unwrap();
        fs::write(home.path().join("owned-file"), "newer edit").unwrap();
        assert!(
            review_standalone(home.path(), id, &fingerprint, &owned, Some((0, &token))).is_err()
        );
        let fresh = review_standalone(home.path(), id, &fingerprint, &owned, None).unwrap();
        review_standalone(
            home.path(),
            id,
            &fingerprint,
            &owned,
            Some((0, fresh.files[0].fingerprint.as_ref().unwrap())),
        )
        .unwrap();
        recover_standalone(home.path(), id, &fingerprint, &owned).unwrap();
        assert_eq!(
            fs::read_to_string(home.path().join("owned-file")).unwrap(),
            "newer edit"
        );
    }

    #[test]
    fn missing_deployment_journal_does_not_turn_old_or_managed_records_into_standalone() {
        let home = tempfile::tempdir().unwrap();
        let id = apply(home.path(), "managed");
        let mut read = read(home.path()).unwrap().unwrap();
        read.journal.records[0].origin = Some(Origin::Deployment);
        read.journal.save(&read.root).unwrap();
        drop(read);
        let owned = HashSet::new(); // No deployment journal is available.
        assert!(history(home.path(), &owned).unwrap().1.is_empty());
        let mut read = super::read(home.path()).unwrap().unwrap();
        read.journal.records[0].origin = None; // Legacy provenance was not recorded.
        read.journal.save(&read.root).unwrap();
        drop(read);
        let row = history(home.path(), &owned).unwrap().1.remove(0);
        assert_eq!(row.origin, HelperOrigin::Unattributed);
        assert!(row.restore_blocked.is_some());
        assert!(recover_standalone(home.path(), id, &row.fingerprint, &owned).is_err());
        assert_eq!(
            fs::read_to_string(home.path().join("owned-file")).unwrap(),
            "managed"
        );
    }
}
