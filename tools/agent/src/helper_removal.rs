//! Compose installation ownership in reverse order before general removal.
//! Planning reads evidence and current targets, but never writes either.
use super::*;
use crate::deployment_protocol::{HelperFile, HelperRemovalPlan};
use std::collections::BTreeMap;

pub(super) fn inspect(home: &Path) -> io::Result<HelperRemovalPlan> {
    let root = home.join(".local/share/newport/needle/helpers");
    let lock = private_file(&root.join("lock"))?;
    acquire_lock(&lock)?;
    let journal = Journal::load_existing(&root).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            io::Error::other("Helper ownership evidence is missing; removal requires review")
        } else {
            error
        }
    })?;
    if journal.records.iter().any(|r| r.state.unsettled()) {
        return Err(io::Error::other(
            "Resolve unfinished helper operations before planning removal",
        ));
    }
    let tx = Transaction {
        root,
        journal,
        id: Uuid::nil(),
        changes: Vec::new(),
        _lock: lock,
    };
    plan(&tx)
}

fn paths(journal: &Journal) -> BTreeMap<&Path, Vec<&Change>> {
    let mut paths: BTreeMap<&Path, Vec<&Change>> = BTreeMap::new();
    for record in &journal.records {
        for change in &record.changes {
            if record.removal {
                // Keeping a file during restoration releases the earlier
                // installation's ownership, even after removal is rolled back.
                if record.state == State::Applied || change.kept.is_some() {
                    paths.remove(change.path.as_path());
                }
                continue;
            }
            // An explicit keep decision releases older ownership of this path.
            // Later approved installations may establish new ownership above it.
            if record.state == State::Applied || change.kept.is_some() {
                paths.entry(&change.path).or_default().push(change);
            }
        }
    }
    paths
}

pub(super) fn prepare(home: &Path, id: Uuid) -> io::Result<Transaction> {
    let mut tx = Transaction::open(home, id)?;
    // Missing evidence is not proof that an installation owns no helpers.
    tx.journal = Journal::load_existing(&tx.root)?;
    let mut changes = Vec::new();
    for (path, history) in paths(&tx.journal) {
        let (before, after, _, bytes) = compose(&tx, path, &history)?;
        if before != after {
            if let Snapshot::File { mode, .. } = &after {
                tx.file(&bytes, *mode)?;
            }
            tx.read(path)?;
            changes.push((path.to_owned(), before, after));
        }
    }
    for (path, before, after) in changes {
        tx.plan(&path, before, after)?;
    }
    Ok(tx)
}

fn plan(tx: &Transaction) -> io::Result<HelperRemovalPlan> {
    let evidence = Transaction::hash(&serde_json::to_vec(&tx.journal)?);
    let mut files = Vec::new();
    for (index, (path, changes)) in paths(&tx.journal).into_iter().enumerate() {
        let mut file = HelperFile {
            index,
            path: path.display().to_string(),
            status: "conflict".into(),
            message: String::new(),
            fingerprint: None,
        };
        match target(tx, path, &changes) {
            Ok((current, target, kept)) => {
                file.fingerprint = Some(Transaction::hash(&serde_json::to_vec(&(
                    &evidence, path, &current, &target,
                ))?));
                (file.status, file.message) = if current == target && kept {
                    (
                        "kept".into(),
                        "Preserved by an earlier keep-file choice.".into(),
                    )
                } else if current == target {
                    (
                        "unchanged".into(),
                        "No owned changes remain at this path.".into(),
                    )
                } else if target == Snapshot::Missing {
                    (
                        "remove".into(),
                        "Remove the file or alias created by Newport.".into(),
                    )
                } else {
                    (
                        "restore".into(),
                        "Restore recorded content or link, preserving surrounding shell edits where block ownership is recorded."
                            .into(),
                    )
                };
            }
            Err(error) => file.message = error.to_string(),
        }
        files.push(file);
    }
    Ok(HelperRemovalPlan { files })
}

fn target(
    tx: &Transaction,
    path: &Path,
    changes: &[&Change],
) -> io::Result<(Snapshot, Snapshot, bool)> {
    compose(tx, path, changes).map(|(current, target, kept, _)| (current, target, kept))
}

fn compose(
    tx: &Transaction,
    path: &Path,
    changes: &[&Change],
) -> io::Result<(Snapshot, Snapshot, bool, Vec<u8>)> {
    let (current, mut bytes) = Transaction::read_current(path)?;
    let mut result = current.clone();
    let mut kept = false;
    for change in changes.iter().rev() {
        if change.kept.is_some() {
            kept = true;
            break;
        }
        if result == change.before {
            continue;
        }
        if result == change.after {
            result = change.before.clone();
            bytes = match &result {
                Snapshot::File { .. } => tx.bytes(&result)?,
                _ => Vec::new(),
            };
        } else if change.shell {
            let restored = tx.shell_restoration(change, &result, &bytes)?;
            let Snapshot::File { mode, .. } = result else {
                unreachable!()
            };
            bytes = restored.into_bytes();
            result = Snapshot::File {
                hash: Transaction::hash(&bytes),
                mode,
            };
        } else {
            return Err(io::Error::other(
                "This path no longer matches its recorded ownership. Preserve it until the conflict is reviewed.",
            ));
        }
    }
    if current != result && matches!(current, Snapshot::File { mode, .. } if mode & 0o200 == 0) {
        return Err(io::Error::other(
            "This file is read-only; removal must preserve it.",
        ));
    }
    Ok((current, result, kept, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "subprocess helper invoked by interrupted_partial_removal_restores_ownership"]
    fn partial_removal_child() {
        let home = PathBuf::from(std::env::var_os("NEWPORT_TEST_HELPER_REMOVAL_HOME").unwrap());
        let id =
            Uuid::parse_str(&std::env::var("NEWPORT_TEST_HELPER_REMOVAL_ID").unwrap()).unwrap();
        let shell = home.join(".bashrc");
        fs::write(&shell, "original").unwrap();
        change(&home, &shell, "installed", false);
        change(&home, &home.join("alias"), "helper", false);
        let mut tx = prepare(&home, id).unwrap();
        let mut calls = 0;
        tx.apply_with_kind(
            || {
                calls += 1;
                if calls == 2 {
                    fs::write(home.join("checkpoint"), "first write")?;
                    loop {
                        std::thread::sleep(std::time::Duration::from_secs(1));
                    }
                }
                Ok(())
            },
            true,
            None,
        )
        .unwrap();
    }

    #[test]
    fn interrupted_partial_removal_restores_ownership() {
        let home = tempfile::tempdir().unwrap();
        let id = Uuid::new_v4();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "helper_transaction::removal::tests::partial_removal_child",
                "--ignored",
            ])
            .env("NEWPORT_TEST_HELPER_REMOVAL_HOME", home.path())
            .env("NEWPORT_TEST_HELPER_REMOVAL_ID", id.to_string())
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !home.path().join("checkpoint").exists() && std::time::Instant::now() < deadline {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let ready = home.path().join("checkpoint").exists();
        let _ = child.kill();
        child.wait().unwrap();
        assert!(ready);
        assert_eq!(
            fs::read_to_string(home.path().join(".bashrc")).unwrap(),
            "original"
        );
        assert_eq!(
            fs::read_to_string(home.path().join("alias")).unwrap(),
            "helper"
        );
        assert_eq!(
            removal_outcome(home.path(), id).unwrap(),
            Some(State::Prepared)
        );
        assert!(inspect(home.path()).is_err());
        Transaction::recover(home.path(), id).unwrap();
        assert_eq!(
            fs::read_to_string(home.path().join(".bashrc")).unwrap(),
            "installed"
        );
        let plan = inspect(home.path()).unwrap();
        assert_eq!(plan.files.len(), 2);
        assert_eq!(plan.files[0].status, "restore");
        assert_eq!(plan.files[1].status, "remove");
    }

    fn change(home: &Path, path: &Path, text: &str, shell: bool) -> Uuid {
        let id = Uuid::new_v4();
        let mut tx = Transaction::open(home, id).unwrap();
        let before = tx.read(path).unwrap();
        let after = tx.file(text.as_bytes(), 0o600).unwrap();
        tx.plan_kind(path, before, after, shell).unwrap();
        tx.apply().unwrap();
        id
    }
    #[test]
    fn combines_all_installations_without_writing_files_or_evidence() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join(".bashrc");
        fs::write(&path, "# mine\n").unwrap();
        let one = crate::shell_setup::block(false);
        let two = one.replace(" env ", " env --new ");
        change(home.path(), &path, &format!("# mine\n{one}"), true);
        change(home.path(), &path, &format!("# mine\n{two}"), true);
        let current = format!("# mine\n{two}# later settings\n");
        fs::write(&path, &current).unwrap();
        let root = home.path().join(".local/share/newport/needle/helpers");
        let saved = fs::read(root.join("journal.json")).unwrap();
        let count = fs::read_dir(root.join("objects")).unwrap().count();
        let plan = inspect(home.path()).unwrap();
        assert_eq!(plan.files[0].status, "restore");
        let tx = Transaction::open(home.path(), Uuid::new_v4()).unwrap();
        let changes: Vec<_> = tx.journal.records.iter().flat_map(|r| &r.changes).collect();
        let (_, result, _) = target(&tx, &path, &changes).unwrap();
        assert_eq!(
            result,
            Snapshot::File {
                hash: Transaction::hash(b"# mine\n# later settings\n"),
                mode: 0o600
            }
        );
        // A plan does not create the new composed snapshot as a backup object.
        assert_eq!(fs::read_dir(root.join("objects")).unwrap().count(), count);
        assert_eq!(fs::read(root.join("journal.json")).unwrap(), saved);
        assert_eq!(fs::read_to_string(path).unwrap(), current);
    }
    #[test]
    fn kept_paths_release_old_ownership_but_later_installations_remain_owned() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("owned");
        let id = change(home.path(), &path, "first", false);
        fs::write(&path, "mine").unwrap();
        assert!(Transaction::recover(home.path(), id).is_err());
        let review = Transaction::review(home.path(), id, None).unwrap();
        Transaction::review(
            home.path(),
            id,
            Some((0, review.files[0].fingerprint.as_deref().unwrap())),
        )
        .unwrap();
        Transaction::recover(home.path(), id).unwrap();
        assert_eq!(inspect(home.path()).unwrap().files[0].status, "kept");
        change(home.path(), &path, "new approved install", false);
        let plan = inspect(home.path()).unwrap();
        assert_eq!(plan.files[0].status, "restore");
        assert_eq!(fs::read_to_string(path).unwrap(), "new approved install");
    }
    #[test]
    fn reports_conflicts_without_claiming_unrecorded_files() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("owned");
        change(home.path(), &path, "installed", false);
        fs::write(&path, "user edit").unwrap();
        fs::write(home.path().join("unrecorded"), "not ours").unwrap();
        let plan = inspect(home.path()).unwrap();
        assert_eq!(plan.files.len(), 1);
        assert_eq!(plan.files[0].status, "conflict");
        assert!(plan.files[0].fingerprint.is_none());
        fs::write(&path, "installed").unwrap();
        assert_eq!(inspect(home.path()).unwrap().files[0].status, "remove");
    }

    #[test]
    fn missing_evidence_and_unfinished_changes_block_planning() {
        let home = tempfile::tempdir().unwrap();
        assert!(inspect(home.path()).is_err());
        assert!(!home.path().join(".local").exists());
        let path = home.path().join("owned");
        let id = change(home.path(), &path, "installed", false);
        fs::write(&path, "user edit").unwrap();
        assert!(Transaction::recover(home.path(), id).is_err());
        assert!(inspect(home.path())
            .unwrap_err()
            .to_string()
            .contains("unfinished"));
        let root = home.path().join(".local/share/newport/needle/helpers");
        fs::remove_file(root.join("journal.json")).unwrap();
        assert!(inspect(home.path())
            .unwrap_err()
            .to_string()
            .contains("evidence is missing"));
        assert!(!root.join("journal.json").exists());
        assert_eq!(fs::read_to_string(path).unwrap(), "user edit");
    }

    #[test]
    fn restores_legacy_alias_target_and_detects_corrupt_backup() {
        let home = tempfile::tempdir().unwrap();
        let alias = home.path().join("xclip");
        symlink("legacy-helper", &alias).unwrap();
        let mut tx = Transaction::open(home.path(), Uuid::new_v4()).unwrap();
        tx.plan(
            &alias,
            Snapshot::Link("legacy-helper".into()),
            Snapshot::Link("newport-agent".into()),
        )
        .unwrap();
        tx.apply().unwrap();
        assert_eq!(inspect(home.path()).unwrap().files[0].status, "restore");
        assert_eq!(fs::read_link(&alias).unwrap(), Path::new("newport-agent"));

        let file = home.path().join("startup");
        fs::write(&file, "original").unwrap();
        change(home.path(), &file, "installed", false);
        let root = home.path().join(".local/share/newport/needle/helpers");
        fs::write(
            root.join("objects").join(Transaction::hash(b"original")),
            "corrupt",
        )
        .unwrap();
        let report = inspect(home.path()).unwrap();
        let row = report
            .files
            .iter()
            .find(|f| f.path == file.to_str().unwrap())
            .unwrap();
        assert_eq!(row.status, "conflict");
        assert!(row.fingerprint.is_none());
        assert_eq!(fs::read_to_string(file).unwrap(), "installed");
    }
}
