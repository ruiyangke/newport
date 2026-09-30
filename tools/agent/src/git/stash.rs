//! Stash selection is bound to a reflog snapshot, never a stale numeric index.
use super::{
    branches, checkout,
    operations::{self, IndexLock},
    protocol::{Action, Error},
    repository,
};
use git2::{Reflog, Repository};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{cell::Cell, ffi::OsStr, fs, os::unix::ffi::OsStrExt, path::Path};
const STASH: &str = "refs/stash";
fn engine(_: git2::Error) -> Error {
    Error::new("STASH_ERROR", "The stash could not be prepared.")
}
fn io_error(_: std::io::Error) -> Error {
    Error::new("IO_ERROR", "The stash index could not be prepared.")
}
fn unknown() -> Error {
    Error::new("OUTCOME_UNKNOWN", "The stash operation may have changed files, its index or stash references. Inspect it before retrying.")
}
fn log(repo: &Repository) -> Result<Option<Reflog>, Error> {
    if fs::metadata(repo.commondir().join("logs/refs/stash"))
        .is_ok_and(|m| m.len() > 16 * 1024 * 1024)
    {
        return Err(Error::new(
            "LIMIT_EXCEEDED",
            "The stash log exceeds the supported size.",
        ));
    }
    match repo.find_reference(STASH) {
        Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(None),
        Err(e) => Err(engine(e)),
        Ok(_) => repo.reflog(STASH).map(Some).map_err(engine),
    }
}
fn rows(log: Option<&Reflog>) -> Result<(Vec<Value>, String), Error> {
    let mut rows = Vec::new();
    let mut token = Sha256::new();
    token.update(b"newport-stash-list-v2\0");
    if let Some(log) = log {
        if log.len() > 10_000 {
            return Err(Error::new("LIMIT_EXCEEDED", "Too many stash entries."));
        }
        for (index, entry) in log.iter().enumerate() {
            let message = entry.message_bytes().unwrap_or_default();
            let committer = entry.committer();
            token.update(entry.id_old().as_bytes());
            token.update(entry.id_new().as_bytes());
            token.update(committer.when().seconds().to_be_bytes());
            token.update(committer.when().offset_minutes().to_be_bytes());
            for bytes in [committer.name_bytes(), committer.email_bytes(), message] {
                token.update((bytes.len() as u64).to_be_bytes());
                token.update(bytes);
            }
            rows.push(json!({"index":index,"oid":entry.id_new().to_string(),"previousOid":entry.id_old().to_string(),"message":String::from_utf8_lossy(&message[..message.len().min(1024)]),"messageTruncated":message.len()>1024,"time":entry.committer().when().seconds()}));
        }
    }
    Ok((
        rows,
        token
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    ))
}
pub fn list(repo: &Repository) -> Result<(Vec<Value>, String), Error> {
    rows(log(repo)?.as_ref())
}

fn drop_locked(
    repo: &Repository,
    tx: &mut git2::Transaction<'_>,
    mut log: Reflog,
    index: usize,
) -> Result<(), Error> {
    log.remove(index, true).map_err(engine)?;
    let first = log.get(0).map(|e| e.id_new());
    tx.set_reflog(STASH, log).map_err(engine)?;
    if let Some(first) = first {
        let signature = repo
            .signature()
            .or_else(|_| git2::Signature::now("Newport", "newport@localhost"))
            .map_err(engine)?;
        tx.set_target(STASH, first, Some(&signature), "stash drop: Newport")
            .map_err(engine)?;
    } else {
        tx.remove(STASH).map_err(engine)?;
    }
    Ok(())
}
// ALLOW_CONFLICTS is needed for merge markers, but it also permits checkout
// to skip obstructing local files. Refuse those obstructions before applying so
// a successful native call cannot cause pop to drop incompletely restored work.
fn guard_apply(repo: &Repository, commit: &git2::Commit<'_>) -> Result<(), Error> {
    let mut incoming = git2::Index::new().map_err(engine)?;
    incoming
        .read_tree(&commit.tree().map_err(engine)?)
        .map_err(engine)?;
    let mut paths: std::collections::BTreeSet<Vec<u8>> = incoming.iter().map(|e| e.path).collect();
    if commit.parent_count() > 2 {
        let tree = commit.parent(2).and_then(|p| p.tree()).map_err(engine)?;
        incoming.read_tree(&tree).map_err(engine)?;
        let root = repo
            .workdir()
            .ok_or_else(|| Error::invalid("Stash requires a working tree."))?;
        for entry in incoming.iter() {
            let path = Path::new(OsStr::from_bytes(&entry.path));
            for (depth, ancestor) in path.ancestors().enumerate() {
                if ancestor.as_os_str().is_empty() {
                    break;
                }
                match fs::symlink_metadata(root.join(ancestor)) {
                    Ok(metadata) if depth == 0 || !metadata.is_dir() => {
                        return Err(Error::new(
                            "CHECKOUT_CONFLICT",
                            "An existing path would prevent restoring an untracked stash file.",
                        ))
                    }
                    Ok(_) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(io_error(e)),
                }
            }
            paths.insert(entry.path);
        }
    }
    super::integration::guard_paths(repo, &paths, true)
}

pub fn apply(source: &Repository, action: &Action, expected: &str) -> Result<Value, Error> {
    let mut repo = Repository::open(source.path()).map_err(engine)?;
    let mut head_lock = source.transaction().map_err(engine)?;
    let busy = |_| {
        Error::new(
            "REPOSITORY_BUSY",
            "Another operation holds a Git reference lock.",
        )
    };
    head_lock.lock_ref("HEAD").map_err(busy)?;
    let head = source.head().map_err(|_| {
        Error::new(
            "UNBORN_HEAD",
            "Create an initial commit before using stashes.",
        )
    })?;
    if head.is_branch() {
        head_lock
            .lock_ref(head.name().map_err(engine)?)
            .map_err(busy)?;
    }
    if repository::fingerprint(&repo)? != expected {
        return Err(Error::new(
            "STALE_SNAPSHOT",
            "Refresh changes before using this stash.",
        ));
    }
    if let Action::StashSave {
        message,
        include_untracked,
        keep_index,
        author,
    } = action
    {
        if message.len() > 4096 || message.contains('\0') {
            return Err(Error::invalid(
                "Stash messages must be at most 4096 bytes and contain no NUL.",
            ));
        }
        let signature = operations::signature(&repo, author.as_ref())?;
        checkout::supported_files(&repo, &head.peel_to_tree().map_err(engine)?)?;
        let mut status_options = git2::StatusOptions::new();
        status_options
            .include_untracked(*include_untracked)
            .recurse_untracked_dirs(true)
            .update_index(false);
        let statuses = repo.statuses(Some(&mut status_options)).map_err(engine)?;
        if statuses.is_empty() {
            return Err(Error::new(
                "NOTHING_TO_STASH",
                "There are no selected changes to stash.",
            ));
        }
        for status in statuses.iter() {
            let path = Path::new(OsStr::from_bytes(status.path_bytes()));
            for attribute in ["filter", "working-tree-encoding"] {
                let value = repo
                    .get_attr_bytes(path, attribute, git2::AttrCheckFlags::FILE_THEN_INDEX)
                    .map_err(engine)?;
                if !matches!(
                    git2::AttrValue::from_bytes(value),
                    git2::AttrValue::Unspecified | git2::AttrValue::False
                ) {
                    return Err(Error::new(
                        "UNSUPPORTED_FILTER",
                        "This stash requires an unsupported filter or encoding.",
                    ));
                }
            }
        }
        drop(statuses);
        let mut lock = IndexLock::acquire(&repo)?;
        if repository::fingerprint(&repo)? != expected {
            return Err(Error::new(
                "STALE_SNAPSHOT",
                "The repository changed before stashing.",
            ));
        }
        let temp = tempfile::Builder::new()
            .prefix("newport-stash-")
            .tempdir_in(repo.path())
            .map_err(io_error)?;
        let path = temp.path().join("index");
        fs::copy(repo.path().join("index"), &path).map_err(io_error)?;
        let index = operations::private_index(&repo, &path)?;
        if index.has_conflicts() {
            return Err(Error::new(
                "UNMERGED_INDEX",
                "Resolve conflicts before saving a stash.",
            ));
        }
        let mut flags = git2::StashFlags::DEFAULT;
        if *include_untracked {
            flags |= git2::StashFlags::INCLUDE_UNTRACKED;
        }
        if *keep_index {
            flags |= git2::StashFlags::KEEP_INDEX;
        }
        let oid = repo
            .stash_save(&signature, message, Some(flags))
            .map_err(|_| unknown())?;
        lock.publish(&path, &repo).map_err(|_| unknown())?;
        return Ok(json!({"oid":oid.to_string(),"saved":true,"refreshRequired":true}));
    }
    let (selected, expected_token, selected_index) = match action {
        Action::StashApply {
            oid,
            expected_token,
            index,
            ..
        }
        | Action::StashPop {
            oid,
            expected_token,
            index,
            ..
        }
        | Action::StashDrop {
            oid,
            expected_token,
            index,
        } => (branches::oid(oid)?, expected_token, *index),
        _ => return Err(Error::invalid("Not a stash operation.")),
    };
    let mut stash_lock = source.transaction().map_err(engine)?;
    stash_lock.lock_ref(STASH).map_err(busy)?;
    let log = log(&repo)?
        .ok_or_else(|| Error::new("STASH_NOT_FOUND", "The selected stash no longer exists."))?;
    if rows(Some(&log))?.1 != *expected_token {
        return Err(Error::new(
            "STALE_STASH_LIST",
            "The stash list changed. Refresh before continuing.",
        ));
    }
    let index = if let Some(index) = selected_index {
        if log.get(index).map(|entry| entry.id_new()) != Some(selected) {
            return Err(Error::new(
                "STALE_STASH_ENTRY",
                "The selected stash entry changed. Refresh before continuing.",
            ));
        }
        index
    } else {
        // Older callers can still select a unique object, but never silently
        // choose the first occurrence when several reflog entries share it.
        let mut matches = log
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.id_new() == selected);
        let index = matches
            .next()
            .map(|(index, _)| index)
            .ok_or_else(|| Error::new("STASH_NOT_FOUND", "The selected stash no longer exists."))?;
        if matches.next().is_some() {
            return Err(Error::new("AMBIGUOUS_STASH", "Multiple stash entries share this object. Update the client and select the exact entry."));
        }
        index
    };
    if matches!(action, Action::StashDrop { .. }) {
        drop_locked(&repo, &mut stash_lock, log, index)?;
        stash_lock.commit().map_err(|_| unknown())?;
        return Ok(json!({"oid":selected.to_string(),"dropped":true,"refreshRequired":true}));
    }
    let stash_commit = repo.find_commit(selected).map_err(engine)?;
    checkout::supported_merge_files(&repo, &stash_commit.tree().map_err(engine)?)?;
    for parent in stash_commit.parents().skip(1) {
        checkout::supported_merge_files(&repo, &parent.tree().map_err(engine)?)?;
    }
    guard_apply(&repo, &stash_commit)?;
    drop(stash_commit);
    let mut lock = IndexLock::acquire(&repo)?;
    if repository::fingerprint(&repo)? != expected {
        return Err(Error::new(
            "STALE_SNAPSHOT",
            "The repository changed before applying the stash.",
        ));
    }
    let temp = tempfile::Builder::new()
        .prefix("newport-unstash-")
        .tempdir_in(repo.path())
        .map_err(io_error)?;
    let path = temp.path().join("index");
    fs::copy(repo.path().join("index"), &path).map_err(io_error)?;
    let _prepared = operations::private_index(&repo, &path)?;
    let started = Cell::new(false);
    let mut options = git2::StashApplyOptions::new();
    if matches!(
        action,
        Action::StashApply {
            reinstate_index: true,
            ..
        } | Action::StashPop {
            reinstate_index: true,
            ..
        }
    ) {
        options.reinstantiate_index();
    }
    let mut checkout = git2::build::CheckoutBuilder::new();
    checkout
        .safe()
        .allow_conflicts(true)
        .remove_untracked(false)
        .remove_ignored(false)
        .overwrite_ignored(false);
    options.checkout_options(checkout).progress_cb(|progress| {
        if matches!(
            progress,
            git2::StashApplyProgress::CheckoutUntracked
                | git2::StashApplyProgress::CheckoutModified
        ) {
            started.set(true);
        }
        true
    });
    repo.stash_apply(index, Some(&mut options)).map_err(|e| {
        if started.get() {
            unknown()
        } else if e.code() == git2::ErrorCode::Conflict {
            Error::new(
                "STASH_CONFLICT",
                "The stash cannot be applied to the current index.",
            )
        } else {
            engine(e)
        }
    })?;
    lock.publish(&path, &repo).map_err(|_| unknown())?;
    let conflicts = repo.index().map_err(|_| unknown())?.has_conflicts();
    let pop = matches!(action, Action::StashPop { .. }) && !conflicts;
    if pop {
        drop_locked(&repo, &mut stash_lock, log, index).map_err(|_| unknown())?;
        stash_lock.commit().map_err(|_| unknown())?;
    }
    Ok(
        json!({"oid":selected.to_string(),"applied":true,"dropped":pop,"needsResolution":conflicts,"refreshRequired":true}),
    )
}

#[cfg(test)]
mod tests {
    use super::super::protocol::Author;
    use super::*;
    fn author() -> Author {
        Author {
            name: "Fixture".into(),
            email: "fixture@example.test".into(),
        }
    }
    fn fixture() -> (tempfile::TempDir, Repository) {
        let temp = tempfile::tempdir().unwrap();
        // Exercise case-insensitive index capabilities on Linux as well as macOS.
        let repo = Repository::init(temp.path()).unwrap();
        repo.config()
            .unwrap()
            .set_bool("core.ignorecase", true)
            .unwrap();
        commit(&repo, "base\n");
        (temp, repo)
    }
    fn commit(repo: &Repository, content: &str) {
        fs::write(repo.workdir().unwrap().join("file"), content).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("file")).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
        let sig = git2::Signature::now("Fixture", "fixture@example.test").unwrap();
        repo.commit(
            Some("HEAD"),
            &sig,
            &sig,
            "fixture",
            &tree,
            &parent.iter().collect::<Vec<_>>(),
        )
        .unwrap();
    }
    fn run(repo: &Repository, action: Action) -> Value {
        apply(repo, &action, &repository::fingerprint(repo).unwrap()).unwrap()
    }
    fn save(repo: &Repository, include_untracked: bool, keep_index: bool) -> Value {
        run(
            repo,
            Action::StashSave {
                message: "Saved work".into(),
                include_untracked,
                keep_index,
                author: Some(author()),
            },
        )
    }
    #[test]
    fn stash_token_covers_message_bytes_hidden_by_display_truncation() {
        let (temp, repo) = fixture();
        fs::write(temp.path().join("file"), "saved").unwrap();
        let saved = save(&repo, false, false);
        let oid = branches::oid(saved["oid"].as_str().unwrap()).unwrap();
        let signature = git2::Signature::new(
            "Fixture",
            "fixture@example.test",
            &git2::Time::new(1234, 60),
        )
        .unwrap();
        let mut log = repo.reflog(STASH).unwrap();
        log.append(oid, &signature, Some(&format!("{}A", "x".repeat(1024))))
            .unwrap();
        log.write().unwrap();
        let first = list(&repo).unwrap();
        log.remove(0, false).unwrap();
        log.append(oid, &signature, Some(&format!("{}B", "x".repeat(1024))))
            .unwrap();
        log.write().unwrap();
        let second = list(&repo).unwrap();
        assert_eq!(first.0, second.0);
        assert_ne!(first.1, second.1);
    }
    #[test]
    fn duplicate_objects_require_and_preserve_exact_reflog_selection() {
        for mode in ["apply", "pop", "drop"] {
            let (temp, repo) = fixture();
            fs::write(temp.path().join("file"), "selected stash content").unwrap();
            let saved = save(&repo, false, false);
            let oid = saved["oid"].as_str().unwrap().to_owned();
            let mut log = repo.reflog(STASH).unwrap();
            log.append(
                branches::oid(&oid).unwrap(),
                &git2::Signature::now("Fixture", "fixture@example.test").unwrap(),
                Some("duplicate entry retained"),
            )
            .unwrap();
            log.write().unwrap();
            let (before, token) = list(&repo).unwrap();
            assert_eq!(before.len(), 2);
            assert_eq!(before[0]["oid"], before[1]["oid"]);
            let action = |index| match mode {
                "apply" => Action::StashApply {
                    oid: oid.clone(),
                    index,
                    expected_token: token.clone(),
                    reinstate_index: false,
                },
                "pop" => Action::StashPop {
                    oid: oid.clone(),
                    index,
                    expected_token: token.clone(),
                    reinstate_index: false,
                },
                _ => Action::StashDrop {
                    oid: oid.clone(),
                    index,
                    expected_token: token.clone(),
                },
            };
            let fingerprint = repository::fingerprint(&repo).unwrap();
            assert_eq!(
                apply(&repo, &action(None), &fingerprint).unwrap_err().code,
                "AMBIGUOUS_STASH"
            );
            assert_eq!(
                apply(&repo, &action(Some(2)), &fingerprint)
                    .unwrap_err()
                    .code,
                "STALE_STASH_ENTRY"
            );
            assert_eq!(list(&repo).unwrap().1, token);
            assert_eq!(fs::read(temp.path().join("file")).unwrap(), b"base\n");
            run(&repo, action(Some(1)));
            let after = list(&repo).unwrap().0;
            assert_eq!(after.len(), if mode == "apply" { 2 } else { 1 });
            assert_eq!(after[0]["message"], "duplicate entry retained");
            assert_eq!(
                fs::read(temp.path().join("file")).unwrap(),
                if mode == "drop" {
                    b"base\n".as_slice()
                } else {
                    b"selected stash content".as_slice()
                }
            );
        }
    }
    #[test]
    fn stale_state_and_existing_index_lock_leave_changes_intact() {
        let (temp, repo) = fixture();
        let stale = repository::fingerprint(&repo).unwrap();
        fs::write(temp.path().join("file"), "local work").unwrap();
        let action = Action::StashSave {
            message: "Saved work".into(),
            include_untracked: false,
            keep_index: false,
            author: Some(author()),
        };
        assert_eq!(
            apply(&repo, &action, &stale).unwrap_err().code,
            "STALE_SNAPSHOT"
        );
        fs::write(repo.path().join("index.lock"), "external lock").unwrap();
        assert_eq!(
            apply(&repo, &action, &repository::fingerprint(&repo).unwrap())
                .unwrap_err()
                .code,
            "REPOSITORY_BUSY"
        );
        assert_eq!(
            fs::read(repo.path().join("index.lock")).unwrap(),
            b"external lock"
        );
        assert_eq!(fs::read(temp.path().join("file")).unwrap(), b"local work");
        assert!(list(&repo).unwrap().0.is_empty());
    }

    #[test]
    fn pop_preserves_untracked_collisions_and_the_stash() {
        let (temp, repo) = fixture();
        fs::write(temp.path().join("untracked"), "saved content").unwrap();
        let saved = save(&repo, true, false);
        fs::write(temp.path().join("untracked"), "new content").unwrap();
        let repo = Repository::open(temp.path()).unwrap();
        let token = list(&repo).unwrap().1;
        let result = apply(
            &repo,
            &Action::StashPop {
                index: None,
                oid: saved["oid"].as_str().unwrap().into(),
                expected_token: token.clone(),
                reinstate_index: false,
            },
            &repository::fingerprint(&repo).unwrap(),
        );
        assert!(result.is_err());
        assert_eq!(
            fs::read(temp.path().join("untracked")).unwrap(),
            b"new content"
        );
        assert_eq!(list(&repo).unwrap().1, token);
        assert_eq!(fs::read(temp.path().join("file")).unwrap(), b"base\n");
        fs::remove_file(temp.path().join("untracked")).unwrap();
        fs::write(temp.path().join("file"), "unsaved work").unwrap();
        let result = apply(
            &repo,
            &Action::StashPop {
                index: None,
                oid: saved["oid"].as_str().unwrap().into(),
                expected_token: token.clone(),
                reinstate_index: false,
            },
            &repository::fingerprint(&repo).unwrap(),
        );
        assert_eq!(result.unwrap_err().code, "DIRTY_WORKTREE");
        assert_eq!(fs::read(temp.path().join("file")).unwrap(), b"unsaved work");
        assert_eq!(list(&repo).unwrap().1, token);
        assert!(!temp.path().join("untracked").exists());
    }

    #[test]
    fn stash_roundtrip_restores_staged_unstaged_and_untracked_content() {
        let (temp, repo) = fixture();
        fs::write(temp.path().join("file"), "staged\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("file")).unwrap();
        index.write().unwrap();
        fs::write(temp.path().join("file"), "working\n").unwrap();
        fs::write(temp.path().join("untracked"), "notes").unwrap();
        let saved = save(&repo, true, false);
        assert_eq!(fs::read(temp.path().join("file")).unwrap(), b"base\n");
        assert!(!temp.path().join("untracked").exists());
        let repo = Repository::open(temp.path()).unwrap();
        let (entries, token) = list(&repo).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["oid"], saved["oid"]);
        let result = run(
            &repo,
            Action::StashApply {
                index: None,
                oid: saved["oid"].as_str().unwrap().into(),
                expected_token: token,
                reinstate_index: true,
            },
        );
        assert_eq!(result["needsResolution"], false);
        assert_eq!(fs::read(temp.path().join("file")).unwrap(), b"working\n");
        assert_eq!(fs::read(temp.path().join("untracked")).unwrap(), b"notes");
        let repo = Repository::open(temp.path()).unwrap();
        let staged = repo
            .index()
            .unwrap()
            .get_path(Path::new("file"), 0)
            .unwrap()
            .id;
        assert_eq!(repo.find_blob(staged).unwrap().content(), b"staged\n");
        assert_eq!(list(&repo).unwrap().0.len(), 1);
        run(
            &repo,
            Action::StashDrop {
                index: None,
                oid: saved["oid"].as_str().unwrap().into(),
                expected_token: list(&repo).unwrap().1,
            },
        );
        assert!(list(&repo).unwrap().0.is_empty());
        assert_eq!(fs::read(temp.path().join("file")).unwrap(), b"working\n");
    }
    #[test]
    fn keep_index_and_stale_selection_cannot_drop_a_different_stash() {
        let (temp, repo) = fixture();
        fs::write(temp.path().join("file"), "staged").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("file")).unwrap();
        index.write().unwrap();
        fs::write(temp.path().join("file"), "working").unwrap();
        let old = save(&repo, false, true)["oid"].as_str().unwrap().to_owned();
        assert_eq!(fs::read(temp.path().join("file")).unwrap(), b"staged");
        let repo = Repository::open(temp.path()).unwrap();
        let old_token = list(&repo).unwrap().1;
        let new = save(&repo, false, false)["oid"]
            .as_str()
            .unwrap()
            .to_owned();
        let repo = Repository::open(temp.path()).unwrap();
        let err = apply(
            &repo,
            &Action::StashDrop {
                index: None,
                oid: old.clone(),
                expected_token: old_token,
            },
            &repository::fingerprint(&repo).unwrap(),
        )
        .unwrap_err();
        assert_eq!(err.code, "STALE_STASH_LIST");
        assert_eq!(list(&repo).unwrap().0.len(), 2);
        run(
            &repo,
            Action::StashDrop {
                index: None,
                oid: old,
                expected_token: list(&repo).unwrap().1,
            },
        );
        assert_eq!(list(&repo).unwrap().0[0]["oid"], new);
    }
    #[test]
    fn pop_retains_a_conflicted_stash_and_drops_a_successful_one() {
        let (temp, repo) = fixture();
        fs::write(temp.path().join("file"), "stash change\n").unwrap();
        let saved = save(&repo, false, false)["oid"]
            .as_str()
            .unwrap()
            .to_owned();
        let repo = Repository::open(temp.path()).unwrap();
        let result = run(
            &repo,
            Action::StashPop {
                index: None,
                oid: saved,
                expected_token: list(&repo).unwrap().1,
                reinstate_index: false,
            },
        );
        assert_eq!(result["dropped"], true);
        assert!(list(&repo).unwrap().0.is_empty());
        let repo = Repository::open(temp.path()).unwrap();
        let saved = save(&repo, false, false)["oid"]
            .as_str()
            .unwrap()
            .to_owned();
        let repo = Repository::open(temp.path()).unwrap();
        commit(&repo, "branch change\n");
        let result = run(
            &repo,
            Action::StashPop {
                index: None,
                oid: saved,
                expected_token: list(&repo).unwrap().1,
                reinstate_index: false,
            },
        );
        assert_eq!(result["needsResolution"], true);
        assert_eq!(result["dropped"], false);
        assert_eq!(list(&repo).unwrap().0.len(), 1);
        assert!(Repository::open(temp.path())
            .unwrap()
            .index()
            .unwrap()
            .has_conflicts());
    }
}
