//! Explicit reset modes, guarded by both HEAD identity and a status snapshot.
use super::{
    branches, checkout, integration,
    operations::{self, IndexLock},
    protocol::{Error, ResetMode},
    repository,
};
use git2::{Index, Repository, RepositoryState};
use serde_json::{json, Value};
use std::{collections::BTreeSet, fs};
fn engine(_: git2::Error) -> Error {
    Error::new("GIT_ERROR", "Cannot prepare this reset.")
}
fn io_error(_: std::io::Error) -> Error {
    Error::new("IO_ERROR", "Cannot prepare the reset index.")
}
fn unknown() -> Error {
    Error::new("OUTCOME_UNKNOWN", "Reset may have changed files, the index or references. Inspect the saved operation before retrying.")
}
pub fn apply(
    repo: &Repository,
    target: &str,
    expected_oid: &str,
    mode: ResetMode,
    expected: &str,
) -> Result<Value, Error> {
    if repo.is_bare() || repo.state() != RepositoryState::Clean {
        return Err(Error::new(
            "INTEGRATION_IN_PROGRESS",
            "Finish the current integration before resetting a working repository.",
        ));
    }
    let target = branches::oid(target)?;
    let original = branches::oid(expected_oid)?;
    let commit = repo.find_commit(target).map_err(|_| {
        Error::new(
            "COMMIT_NOT_FOUND",
            "The reset target must be an existing commit.",
        )
    })?;
    let head = repo.head().map_err(engine)?;
    let reference = head.name().map_err(engine)?.to_owned();
    if head.target() != Some(original) {
        return Err(Error::new(
            "STALE_REFERENCE",
            "HEAD moved. Refresh before resetting.",
        ));
    }
    if head.is_branch() && branches::other_worktree(repo, &reference)? {
        return Err(Error::new(
            "BRANCH_IN_USE",
            "This branch is checked out in another worktree.",
        ));
    }
    let mut lock = IndexLock::acquire(repo)?;
    let mut refs = repo.transaction().map_err(engine)?;
    let busy = |_| Error::new("REPOSITORY_BUSY", "HEAD or a reset reference is locked.");
    refs.lock_ref("HEAD").map_err(busy)?;
    if reference != "HEAD" {
        refs.lock_ref(&reference).map_err(busy)?;
    }
    refs.lock_ref("ORIG_HEAD").map_err(busy)?;
    if repository::fingerprint(repo)? != expected {
        return Err(Error::new(
            "STALE_SNAPSHOT",
            "Refresh the repository before resetting.",
        ));
    }
    let fresh = repo.head().map_err(engine)?;
    if fresh.target() != Some(original) || fresh.name().map_err(engine)? != reference {
        return Err(Error::new(
            "STALE_REFERENCE",
            "HEAD moved. Refresh before resetting.",
        ));
    }
    if repo.index().map_err(engine)?.has_conflicts() {
        return Err(Error::new(
            "UNMERGED_INDEX",
            "Resolve conflicts before resetting.",
        ));
    }
    let signature = repo
        .signature()
        .or_else(|_| git2::Signature::now("Newport", "newport@localhost"))
        .map_err(engine)?;
    refs.set_target(
        "ORIG_HEAD",
        original,
        Some(&signature),
        "reset: previous HEAD (Newport)",
    )
    .map_err(engine)?;
    refs.set_target(&reference, target, Some(&signature), "reset: Newport")
        .map_err(engine)?;
    // Soft reset does not touch either file contents or index bytes.
    if mode != ResetMode::Soft {
        let tree = commit.tree().map_err(engine)?;
        let temporary = if mode == ResetMode::Hard {
            checkout::supported_files(repo, &tree)?;
            checkout::supported_files(
                repo,
                &repo
                    .find_commit(original)
                    .map_err(engine)?
                    .tree()
                    .map_err(engine)?,
            )?;
            let mut paths: BTreeSet<Vec<u8>> = repo
                .index()
                .map_err(engine)?
                .iter()
                .map(|e| e.path)
                .collect();
            for source in [
                repo.find_commit(original)
                    .map_err(engine)?
                    .tree()
                    .map_err(engine)?,
                tree,
            ] {
                let mut index = Index::new().map_err(engine)?;
                index.read_tree(&source).map_err(engine)?;
                paths.extend(index.iter().map(|e| e.path));
            }
            if paths.len() > 20_000 {
                return Err(Error::new(
                    "LIMIT_EXCEEDED",
                    "Reset exceeds the tracked path limit.",
                ));
            }
            // Hard reset discards tracked changes only. Untracked/ignored collisions
            // are refused before the shared restore helper writes working files.
            integration::guard_paths(repo, &paths, false)?;
            integration::restore_paths(repo, target, &paths)?
        } else {
            let tmp = tempfile::tempdir_in(repo.path()).map_err(io_error)?;
            let path = tmp.path().join("index");
            fs::copy(repo.path().join("index"), &path).map_err(io_error)?;
            let mut index = operations::private_index(repo, &path)?;
            index.read_tree(&tree).map_err(engine)?;
            index.write().map_err(engine)?;
            tmp
        };
        lock.publish(&temporary.path().join("index"), repo)
            .map_err(|_| unknown())?;
    }
    refs.commit().map_err(|_| unknown())?;
    Ok(
        json!({"mode":mode,"oid":target.to_string(),"previousOid":original.to_string(),"reference":reference,"refreshRequired":true}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use git2::Oid;
    use std::path::Path;
    fn fixture() -> (tempfile::TempDir, Repository, Oid, Oid) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = Repository::init(tmp.path()).unwrap();
        let mut cfg = repo.config().unwrap();
        cfg.set_str(
            "core.hooksPath",
            repo.path().join("hooks").to_str().unwrap(),
        )
        .unwrap();
        let sig = git2::Signature::now("Fixture", "fixture@example.test").unwrap();
        let mut builder = repo.treebuilder(None).unwrap();
        builder
            .insert("file", repo.blob(b"base").unwrap(), 0o100644)
            .unwrap();
        let tree = repo.find_tree(builder.write().unwrap()).unwrap();
        let base = repo
            .commit(Some("HEAD"), &sig, &sig, "base", &tree, &[])
            .unwrap();
        builder
            .insert("file", repo.blob(b"tip").unwrap(), 0o100644)
            .unwrap();
        builder
            .insert("added", repo.blob(b"added").unwrap(), 0o100644)
            .unwrap();
        let tip = repo
            .commit(
                Some("HEAD"),
                &sig,
                &sig,
                "tip",
                &repo.find_tree(builder.write().unwrap()).unwrap(),
                &[&repo.find_commit(base).unwrap()],
            )
            .unwrap();
        repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
            .unwrap();
        drop(tree);
        drop(builder);
        (tmp, repo, base, tip)
    }
    fn run(repo: &Repository, base: Oid, tip: Oid, mode: ResetMode) -> Result<Value, Error> {
        operations::apply(
            repo,
            &super::super::protocol::Action::Reset {
                target_oid: base.to_string(),
                expected_oid: tip.to_string(),
                mode,
            },
            &[],
            &repository::fingerprint(repo).unwrap(),
        )
    }
    #[test]
    fn soft_preserves_index_and_working_files() {
        let (tmp, repo, base, tip) = fixture();
        fs::write(tmp.path().join("file"), "working").unwrap();
        let index = fs::read(repo.path().join("index")).unwrap();
        run(&repo, base, tip, ResetMode::Soft).unwrap();
        assert_eq!(repo.head().unwrap().target(), Some(base));
        assert_eq!(
            repo.find_reference("ORIG_HEAD").unwrap().target(),
            Some(tip)
        );
        assert_eq!(fs::read(repo.path().join("index")).unwrap(), index);
        assert_eq!(fs::read(tmp.path().join("file")).unwrap(), b"working");
        assert!(tmp.path().join("added").exists());
    }
    #[test]
    fn mixed_preserves_files_and_replaces_index_in_detached_head() {
        let (tmp, repo, base, tip) = fixture();
        repo.set_head_detached(tip).unwrap();
        fs::write(tmp.path().join("file"), "working").unwrap();
        run(&repo, base, tip, ResetMode::Mixed).unwrap();
        let repo = Repository::open(tmp.path()).unwrap();
        assert!(repo.head_detached().unwrap());
        assert_eq!(repo.head().unwrap().target(), Some(base));
        assert_eq!(
            repo.index().unwrap().write_tree().unwrap(),
            repo.find_commit(base).unwrap().tree_id()
        );
        assert_eq!(fs::read(tmp.path().join("file")).unwrap(), b"working");
        assert!(tmp.path().join("added").exists());
    }
    #[test]
    fn hard_discards_tracked_edits_but_preserves_untracked_files() {
        let (tmp, repo, base, tip) = fixture();
        fs::write(tmp.path().join("file"), "working").unwrap();
        fs::write(tmp.path().join("staged-new"), "staged").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("staged-new")).unwrap();
        index.write().unwrap();
        fs::write(tmp.path().join("untracked"), "keep").unwrap();
        run(&repo, base, tip, ResetMode::Hard).unwrap();
        let repo = Repository::open(tmp.path()).unwrap();
        assert_eq!(repo.head().unwrap().target(), Some(base));
        assert_eq!(
            repo.index().unwrap().write_tree().unwrap(),
            repo.find_commit(base).unwrap().tree_id()
        );
        assert_eq!(fs::read(tmp.path().join("file")).unwrap(), b"base");
        assert_eq!(fs::read(tmp.path().join("untracked")).unwrap(), b"keep");
        assert!(!tmp.path().join("added").exists());
        assert!(!tmp.path().join("staged-new").exists());
    }
    #[test]
    fn stale_files_and_locked_recovery_reference_leave_state_unchanged() {
        let (tmp, repo, base, tip) = fixture();
        let snapshot = repository::fingerprint(&repo).unwrap();
        let index = fs::read(repo.path().join("index")).unwrap();
        fs::write(tmp.path().join("file"), "new edit").unwrap();
        assert_eq!(
            apply(
                &repo,
                &base.to_string(),
                &tip.to_string(),
                ResetMode::Hard,
                &snapshot
            )
            .unwrap_err()
            .code,
            "STALE_SNAPSHOT"
        );
        assert_eq!(repo.head().unwrap().target(), Some(tip));
        assert_eq!(fs::read(repo.path().join("index")).unwrap(), index);
        assert_eq!(fs::read(tmp.path().join("file")).unwrap(), b"new edit");
        fs::write(repo.path().join("ORIG_HEAD.lock"), "external").unwrap();
        assert_eq!(
            run(&repo, base, tip, ResetMode::Hard).unwrap_err().code,
            "REPOSITORY_BUSY"
        );
        assert_eq!(repo.head().unwrap().target(), Some(tip));
        assert_eq!(fs::read(tmp.path().join("file")).unwrap(), b"new edit");
        assert_eq!(
            fs::read(repo.path().join("ORIG_HEAD.lock")).unwrap(),
            b"external"
        );
    }
    #[test]
    fn collision_stale_head_and_native_locks_refuse_without_writes() {
        let (tmp, repo, base, tip) = fixture();
        run(&repo, base, tip, ResetMode::Hard).unwrap();
        let repo = Repository::open(tmp.path()).unwrap();
        fs::write(tmp.path().join("added"), "untracked").unwrap();
        assert_eq!(
            run(&repo, tip, base, ResetMode::Hard).unwrap_err().code,
            "CHECKOUT_CONFLICT"
        );
        assert_eq!(repo.head().unwrap().target(), Some(base));
        assert_eq!(fs::read(tmp.path().join("added")).unwrap(), b"untracked");
        assert_eq!(
            run(&repo, tip, tip, ResetMode::Soft).unwrap_err().code,
            "STALE_REFERENCE"
        );
        fs::write(repo.path().join("index.lock"), "external").unwrap();
        assert_eq!(
            run(&repo, tip, base, ResetMode::Mixed).unwrap_err().code,
            "REPOSITORY_BUSY"
        );
        assert_eq!(
            fs::read(repo.path().join("index.lock")).unwrap(),
            b"external"
        );
    }
}
