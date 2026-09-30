//! Safe checkout with a private replacement index and journaled partial outcomes.
use super::{
    branches,
    operations::IndexLock,
    protocol::{CheckoutTarget, Error},
    repository,
};
use git2::{AttrCheckFlags, Index, Repository};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    ffi::OsStr,
    fs,
    os::unix::{ffi::OsStrExt, fs::PermissionsExt},
    path::Path,
};

fn engine(_: git2::Error) -> Error {
    Error::new("GIT_ERROR", "The checkout could not be prepared.")
}
fn io_error(_: std::io::Error) -> Error {
    Error::new("IO_ERROR", "The checkout index could not be prepared.")
}
fn uncertain() -> Error {
    Error::new("OUTCOME_UNKNOWN", "Checkout may have changed working files or the index. Inspect the operation and repository before retrying.")
}

pub(super) fn supported_files(repo: &Repository, target: &git2::Tree<'_>) -> Result<(), Error> {
    supported_attributes(repo, target, false, None)
}
/// Selected-file restoration already checks working-tree attributes and file
/// modes through discard::guard_path. Check the destination index's rules only
/// for those paths, including attributes inherited from their parent directories.
pub(super) fn supported_index_paths(
    repo: &Repository,
    target: &mut Index,
    paths: &[Vec<u8>],
) -> Result<(), Error> {
    let destination = Repository::open(repo.path()).map_err(engine)?;
    destination.set_index(target).map_err(engine)?;
    for bytes in paths {
        let path = Path::new(OsStr::from_bytes(bytes));
        for attribute in ["filter", "working-tree-encoding"] {
            let value = destination
                .get_attr_bytes(path, attribute, AttrCheckFlags::INDEX_ONLY)
                .map_err(engine)?;
            if !matches!(
                git2::AttrValue::from_bytes(value),
                git2::AttrValue::Unspecified | git2::AttrValue::False
            ) {
                return Err(Error::new(
                    "UNSUPPORTED_FILTER",
                    "This file requires an unsupported filter or encoding.",
                ));
            }
        }
    }
    Ok(())
}
pub(super) fn supported_merge_files(
    repo: &Repository,
    target: &git2::Tree<'_>,
) -> Result<(), Error> {
    supported_attributes(repo, target, true, None)
}
/// Validate every input tree while resolving working-tree attributes only once
/// per path. The cache belongs to this preparation, never to the repository.
pub(super) fn supported_merge_trees(
    repo: &Repository,
    targets: &[git2::Tree<'_>],
) -> Result<(), Error> {
    let mut checks = MergeChecks::new(repo);
    for target in targets {
        checks.check(target)?;
    }
    Ok(())
}
/// Reuse only during one read-only preparation phase. Tree objects are immutable;
/// working-tree attributes must be checked afresh after any repository mutation.
pub(super) struct MergeChecks<'repo> {
    repo: &'repo Repository,
    trees: HashSet<git2::Oid>,
    current: HashSet<Vec<u8>>,
}
impl<'repo> MergeChecks<'repo> {
    pub(super) fn new(repo: &'repo Repository) -> Self {
        Self {
            repo,
            trees: HashSet::new(),
            current: HashSet::new(),
        }
    }
    pub(super) fn check(&mut self, target: &git2::Tree<'_>) -> Result<(), Error> {
        if !self.trees.contains(&target.id()) {
            supported_attributes(self.repo, target, true, Some(&mut self.current))?;
            self.trees.insert(target.id());
        }
        Ok(())
    }
}
fn supported_attributes(
    repo: &Repository,
    target: &git2::Tree<'_>,
    merging: bool,
    mut checked_current: Option<&mut HashSet<Vec<u8>>>,
) -> Result<(), Error> {
    let target_repo = Repository::open(repo.path()).map_err(engine)?;
    let mut target_index = Index::new().map_err(engine)?;
    target_index.read_tree(target).map_err(engine)?;
    target_repo.set_index(&mut target_index).map_err(engine)?;
    let current_index = repo.index().map_err(engine)?;
    // Index entries are path-sorted. Walk their union without building another
    // repository-sized collection or checking shared paths twice. Inspect both
    // modes when a path exists in both indexes (including file/submodule changes).
    let mut destination = target_index.iter().peekable();
    let mut current = current_index.iter().peekable();
    loop {
        let (entry, other_is_submodule) = match (destination.peek(), current.peek()) {
            (Some(a), Some(b)) if a.path == b.path => {
                let other = current.next().expect("peeked current entry");
                (
                    destination.next().expect("peeked destination entry"),
                    other.mode == 0o160000,
                )
            }
            (Some(a), Some(b)) if a.path < b.path => {
                (destination.next().expect("peeked destination entry"), false)
            }
            (Some(_), Some(_)) | (None, Some(_)) => {
                (current.next().expect("peeked current entry"), false)
            }
            (Some(_), None) => (destination.next().expect("peeked destination entry"), false),
            (None, None) => break,
        };
        if entry.mode == 0o160000 || other_is_submodule {
            return Err(Error::new(
                "UNSUPPORTED_CAPABILITY",
                "Submodule checkout is not implemented yet.",
            ));
        }
        let path = Path::new(OsStr::from_bytes(&entry.path));
        for (source, flags) in [
            (repo, AttrCheckFlags::FILE_THEN_INDEX),
            (&target_repo, AttrCheckFlags::INDEX_ONLY),
        ] {
            if flags == AttrCheckFlags::FILE_THEN_INDEX {
                if let Some(checked) = checked_current.as_mut() {
                    if checked.contains(entry.path.as_slice()) {
                        continue;
                    }
                }
            }
            for attribute in ["filter", "working-tree-encoding", "merge"] {
                if attribute == "merge" && !merging {
                    continue;
                }
                let value = source
                    .get_attr_bytes(path, attribute, flags)
                    .map_err(engine)?;
                if attribute == "merge" {
                    if !matches!(
                        git2::AttrValue::from_bytes(value),
                        git2::AttrValue::Unspecified
                            | git2::AttrValue::True
                            | git2::AttrValue::False
                            | git2::AttrValue::String("text" | "binary" | "union")
                    ) {
                        return Err(Error::new(
                            "UNSUPPORTED_MERGE_DRIVER",
                            "Custom merge drivers are not implemented yet.",
                        ));
                    }
                    continue;
                }
                if !matches!(
                    git2::AttrValue::from_bytes(value),
                    git2::AttrValue::Unspecified | git2::AttrValue::False
                ) {
                    return Err(Error::new("UNSUPPORTED_FILTER", "Checkout requires a custom filter or encoding that is not implemented yet."));
                }
            }
            if flags == AttrCheckFlags::FILE_THEN_INDEX {
                if let Some(checked) = checked_current.as_mut() {
                    checked.insert(entry.path.clone());
                }
            }
        }
    }
    Ok(())
}
pub(super) fn supported_hook(repo: &Repository, hook: &str) -> Result<(), Error> {
    let hooks = match repo.config().map_err(engine)?.get_path("core.hooksPath") {
        Ok(p) if p.is_absolute() => p,
        Ok(p) => repo.workdir().unwrap_or(repo.path()).join(p),
        Err(e) if e.code() == git2::ErrorCode::NotFound => repo.commondir().join("hooks"),
        Err(e) => return Err(engine(e)),
    };
    match fs::metadata(hooks.join(hook)) {
        Ok(meta) if meta.permissions().mode() & 0o111 != 0 => Err(Error::new(
            "UNSUPPORTED_HOOK",
            format!("The {hook} hook is executable; hook execution is not implemented yet."),
        )),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io_error(e)),
    }
}
pub fn apply(repo: &Repository, target: &CheckoutTarget, expected: &str) -> Result<Value, Error> {
    apply_inner(repo, target, expected, false)
}
pub fn fast_forward(repo: &Repository, oid: &str, expected: &str) -> Result<Value, Error> {
    apply_inner(
        repo,
        &CheckoutTarget::Detached { oid: oid.into() },
        expected,
        true,
    )
}
fn apply_inner(
    repo: &Repository,
    target: &CheckoutTarget,
    expected: &str,
    advance: bool,
) -> Result<Value, Error> {
    supported_hook(
        repo,
        if advance {
            "post-merge"
        } else {
            "post-checkout"
        },
    )?;
    let (reference, oid) = match target {
        CheckoutTarget::Branch { name, expected_oid } => {
            branches::name(name)?;
            let reference = format!("refs/heads/{name}");
            if branches::other_worktree(repo, &reference)? {
                return Err(Error::new(
                    "BRANCH_IN_USE",
                    "This branch is checked out in another worktree.",
                ));
            }
            (Some(reference), branches::oid(expected_oid)?)
        }
        CheckoutTarget::Detached { oid } => (None, branches::oid(oid)?),
    };
    let (reference, previous) = if advance {
        let head = repo.head().map_err(engine)?;
        if !head.is_branch() {
            return Err(Error::new(
                "DETACHED_HEAD",
                "Switch to a branch before integrating changes.",
            ));
        }
        let reference = head.name().map_err(engine)?.to_owned();
        if branches::other_worktree(repo, &reference)? {
            return Err(Error::new(
                "BRANCH_IN_USE",
                "This branch is checked out in another worktree.",
            ));
        }
        (
            Some(reference),
            Some(
                head.target()
                    .ok_or_else(|| Error::new("GIT_ERROR", "HEAD is not a commit reference."))?,
            ),
        )
    } else {
        (reference, None)
    };
    let commit = repo.find_commit(oid).map_err(engine)?;
    let tree = commit.tree().map_err(engine)?;
    let mut index_lock = IndexLock::acquire(repo)?;
    let mut refs = repo.transaction().map_err(engine)?;
    let busy = |_| {
        Error::new(
            "REPOSITORY_BUSY",
            "HEAD or the destination branch is locked.",
        )
    };
    refs.lock_ref("HEAD").map_err(busy)?;
    if let Some(reference) = &reference {
        refs.lock_ref(reference).map_err(busy)?;
        if repo.find_reference(reference).map_err(engine)?.target() != Some(previous.unwrap_or(oid))
        {
            return Err(Error::new(
                "STALE_REFERENCE",
                "The destination branch moved. Refresh before switching.",
            ));
        }
    }
    if repository::fingerprint(repo)? != expected {
        return Err(Error::new(
            "STALE_SNAPSHOT",
            "The repository changed. Refresh before switching.",
        ));
    }
    if let Some(previous) = previous {
        if previous == oid || repo.graph_descendant_of(previous, oid).map_err(engine)? {
            return Ok(
                json!({"oid":previous.to_string(),"reference":reference,"fastForwarded":false,"alreadyUpToDate":true,"refreshRequired":true}),
            );
        }
        if !repo.graph_descendant_of(oid, previous).map_err(engine)? {
            return Err(Error::new("DIVERGED_HISTORY", "These histories have diverged. Choose merge or rebase; no files or references were changed."));
        }
    }
    if repo.index().map_err(engine)?.has_conflicts() {
        return Err(Error::new(
            "UNMERGED_INDEX",
            "Resolve index conflicts before switching.",
        ));
    }
    supported_files(repo, &tree)?;
    // Ref updates must not fail just because a server has no configured commit author.
    let signature = repo
        .signature()
        .or_else(|_| git2::Signature::now("Newport", "newport@localhost"))
        .map_err(engine)?;
    match &reference {
        Some(reference) if advance => refs
            .set_target(
                reference,
                oid,
                Some(&signature),
                "merge: fast-forward (Newport)",
            )
            .map_err(engine)?,
        Some(reference) => refs
            .set_symbolic_target("HEAD", reference, Some(&signature), "checkout: Newport")
            .map_err(engine)?,
        None => refs
            .set_target("HEAD", oid, Some(&signature), "checkout: Newport")
            .map_err(engine)?,
    }
    let temp = tempfile::Builder::new()
        .prefix("newport-checkout-")
        .tempdir_in(repo.path())
        .map_err(io_error)?;
    let temp_index = temp.path().join("index");
    let original = repo.path().join("index");
    if original.exists() {
        fs::copy(&original, &temp_index).map_err(io_error)?;
    }
    let _index = super::operations::private_index(repo, &temp_index)?;
    // Conflict analysis precedes the first progress callback and working-file writes.
    // The Rust dry_run helper uses CHECKOUT_NONE, which skips that analysis.
    let started = std::cell::Cell::new(false);
    let mut safe = git2::build::CheckoutBuilder::new();
    safe.safe()
        .allow_conflicts(false)
        .remove_untracked(false)
        .remove_ignored(false)
        .overwrite_ignored(false)
        .progress(|_, _, _| started.set(true));
    repo.checkout_tree(tree.as_object(), Some(&mut safe)).map_err(|e| {
        if !started.get() && e.code() == git2::ErrorCode::Conflict {
            Error::new("CHECKOUT_CONFLICT", "Local changes or untracked files would be overwritten. Commit or stash them before switching.")
        } else if !started.get() { engine(e) } else { uncertain() }
    })?;
    index_lock
        .publish(&temp_index, repo)
        .map_err(|_| uncertain())?;
    refs.commit().map_err(|_| uncertain())?;
    Ok(
        json!({"oid":oid.to_string(),"reference":reference,"detached":reference.is_none(),"fastForwarded":advance,"previousOid":previous.map(|v|v.to_string()),"refreshRequired":true}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn commit(repo: &Repository, files: &[(&str, &str)]) -> git2::Oid {
        let mut index = repo.index().unwrap();
        for (path, contents) in files {
            fs::write(repo.workdir().unwrap().join(path), contents).unwrap();
            index.add_path(Path::new(path)).unwrap();
        }
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
        let sig = git2::Signature::now("Fixture", "test@example.test").unwrap();
        repo.commit(
            Some("HEAD"),
            &sig,
            &sig,
            "fixture",
            &tree,
            &parent.iter().collect::<Vec<_>>(),
        )
        .unwrap()
    }
    fn fixture() -> (tempfile::TempDir, Repository, git2::Oid, git2::Oid) {
        let temp = tempfile::tempdir().unwrap();
        let repo = Repository::init(temp.path()).unwrap();
        repo.config()
            .unwrap()
            .set_str(
                "core.hooksPath",
                repo.path().join("hooks").to_str().unwrap(),
            )
            .unwrap();
        let old = commit(&repo, &[("file", "base"), ("local", "unchanged")]);
        repo.branch("previous", &repo.find_commit(old).unwrap(), false)
            .unwrap();
        let new = commit(&repo, &[("file", "next"), ("added", "new")]);
        (temp, repo, old, new)
    }
    fn run(repo: &Repository, target: CheckoutTarget) -> Result<Value, Error> {
        apply(repo, &target, &repository::fingerprint(repo).unwrap())
    }
    #[test]
    fn checkout_switches_branch_and_detaches_preserving_unrelated_edits() {
        let (temp, repo, old, new) = fixture();
        fs::write(temp.path().join("local"), "staged local").unwrap();
        let mut staged = repo.index().unwrap();
        staged.add_path(Path::new("local")).unwrap();
        staged.write().unwrap();
        fs::write(temp.path().join("local"), "my changes").unwrap();
        fs::write(temp.path().join("untracked"), "keep").unwrap();
        let result = run(
            &repo,
            CheckoutTarget::Branch {
                name: "previous".into(),
                expected_oid: old.to_string(),
            },
        )
        .unwrap();
        assert_eq!(result["detached"], false);
        let repo = Repository::open(temp.path()).unwrap();
        assert_eq!(repo.head().unwrap().shorthand().unwrap(), "previous");
        assert_eq!(fs::read(temp.path().join("file")).unwrap(), b"base");
        let staged_oid = repo
            .index()
            .unwrap()
            .get_path(Path::new("local"), 0)
            .unwrap()
            .id;
        assert_eq!(
            repo.find_blob(staged_oid).unwrap().content(),
            b"staged local"
        );
        assert!(!temp.path().join("added").exists());
        assert_eq!(fs::read(temp.path().join("local")).unwrap(), b"my changes");
        assert_eq!(fs::read(temp.path().join("untracked")).unwrap(), b"keep");
        run(
            &repo,
            CheckoutTarget::Detached {
                oid: new.to_string(),
            },
        )
        .unwrap();
        let repo = Repository::open(temp.path()).unwrap();
        assert!(repo.head_detached().unwrap());
        assert_eq!(repo.head().unwrap().target(), Some(new));
        assert_eq!(fs::read(temp.path().join("added")).unwrap(), b"new");
        assert!(!repo
            .status_file(Path::new("file"))
            .unwrap()
            .is_index_modified());
        assert_eq!(fs::read(temp.path().join("local")).unwrap(), b"my changes");
    }
    #[test]
    fn checkout_ignores_missing_unrelated_checkout_but_preserves_its_reservation() {
        let (temp, repo, old, new) = fixture();
        let parent = tempfile::tempdir().unwrap();
        let linked_path = parent.path().join("linked");
        repo.worktree("linked", &linked_path, None).unwrap();
        fs::remove_dir_all(&linked_path).unwrap();
        let admin_head = repo.commondir().join("worktrees/linked/HEAD");
        let registered_head = fs::read(&admin_head).unwrap();
        let index = fs::read(repo.path().join("index")).unwrap();
        assert_eq!(
            run(
                &repo,
                CheckoutTarget::Branch {
                    name: "linked".into(),
                    expected_oid: new.to_string(),
                }
            )
            .unwrap_err()
            .code,
            "BRANCH_IN_USE"
        );
        assert_eq!(repo.head().unwrap().target(), Some(new));
        assert_eq!(fs::read(repo.path().join("index")).unwrap(), index);
        run(
            &repo,
            CheckoutTarget::Branch {
                name: "previous".into(),
                expected_oid: old.to_string(),
            },
        )
        .unwrap();
        let reopened = Repository::open(temp.path()).unwrap();
        assert_eq!(reopened.head().unwrap().shorthand().unwrap(), "previous");
        assert_eq!(fs::read(temp.path().join("file")).unwrap(), b"base");
        assert_eq!(fs::read(admin_head).unwrap(), registered_head);
        assert!(!linked_path.exists());
    }
    #[test]
    fn unreadable_registered_head_refuses_checkout_before_mutation() {
        let (temp, repo, old, new) = fixture();
        let parent = tempfile::tempdir().unwrap();
        repo.worktree("linked", &parent.path().join("linked"), None)
            .unwrap();
        let head = repo.commondir().join("worktrees/linked/HEAD");
        fs::remove_file(&head).unwrap();
        let index = fs::read(repo.path().join("index")).unwrap();
        for malformed in [
            None,
            Some("not a HEAD"),
            Some("ref: refs/heads/invalid name\n"),
        ] {
            if let Some(contents) = malformed {
                fs::write(&head, contents).unwrap();
            }
            assert_eq!(
                run(
                    &repo,
                    CheckoutTarget::Branch {
                        name: "previous".into(),
                        expected_oid: old.to_string(),
                    }
                )
                .unwrap_err()
                .code,
                "WORKTREE_METADATA_UNAVAILABLE"
            );
            assert_eq!(repo.head().unwrap().target(), Some(new));
            assert_eq!(fs::read(repo.path().join("index")).unwrap(), index);
            assert_eq!(fs::read(temp.path().join("file")).unwrap(), b"next");
        }
    }
    #[test]
    fn checkout_conflicts_do_not_change_head_index_or_files() {
        let (temp, repo, old, new) = fixture();
        fs::write(temp.path().join("file"), "mine").unwrap();
        let before = fs::read(repo.path().join("index")).unwrap();
        assert_eq!(
            run(
                &repo,
                CheckoutTarget::Detached {
                    oid: old.to_string()
                }
            )
            .unwrap_err()
            .code,
            "CHECKOUT_CONFLICT"
        );
        assert_eq!(repo.head().unwrap().target(), Some(new));
        assert_eq!(fs::read(repo.path().join("index")).unwrap(), before);
        assert_eq!(fs::read(temp.path().join("file")).unwrap(), b"mine");
        assert!(!repo.path().join("HEAD.lock").exists());
        assert!(!repo.path().join("index.lock").exists());
    }
    #[test]
    fn preserves_untracked_collisions_and_rejects_worktree_and_index_locks() {
        let (temp, repo, old, new) = fixture();
        run(
            &repo,
            CheckoutTarget::Branch {
                name: "previous".into(),
                expected_oid: old.to_string(),
            },
        )
        .unwrap();
        let repo = Repository::open(temp.path()).unwrap();
        fs::write(temp.path().join("added"), "untracked contents").unwrap();
        let before = fs::read(repo.path().join("index")).unwrap();
        assert_eq!(
            run(
                &repo,
                CheckoutTarget::Detached {
                    oid: new.to_string()
                }
            )
            .unwrap_err()
            .code,
            "CHECKOUT_CONFLICT"
        );
        assert_eq!(
            fs::read(temp.path().join("added")).unwrap(),
            b"untracked contents"
        );
        assert_eq!(fs::read(repo.path().join("index")).unwrap(), before);
        assert_eq!(repo.head().unwrap().target(), Some(old));
        fs::write(repo.path().join("info/exclude"), "added\n").unwrap();
        assert_eq!(
            run(
                &repo,
                CheckoutTarget::Detached {
                    oid: new.to_string()
                }
            )
            .unwrap_err()
            .code,
            "CHECKOUT_CONFLICT"
        );
        fs::write(repo.path().join("index.lock"), "external").unwrap();
        assert_eq!(
            run(
                &repo,
                CheckoutTarget::Detached {
                    oid: new.to_string()
                }
            )
            .unwrap_err()
            .code,
            "REPOSITORY_BUSY"
        );
        assert_eq!(
            fs::read(repo.path().join("index.lock")).unwrap(),
            b"external"
        );
        fs::remove_file(repo.path().join("index.lock")).unwrap();
        let linked_dir = tempfile::tempdir().unwrap();
        repo.worktree("linked", &linked_dir.path().join("linked"), None)
            .unwrap();
        assert_eq!(
            run(
                &repo,
                CheckoutTarget::Branch {
                    name: "linked".into(),
                    expected_oid: old.to_string()
                }
            )
            .unwrap_err()
            .code,
            "BRANCH_IN_USE"
        );
    }
    #[test]
    fn fast_forward_updates_current_branch_and_preserves_local_changes() {
        let (temp, repo, old, new) = fixture();
        run(
            &repo,
            CheckoutTarget::Branch {
                name: "previous".into(),
                expected_oid: old.to_string(),
            },
        )
        .unwrap();
        let repo = Repository::open(temp.path()).unwrap();
        fs::write(temp.path().join("local"), "local edit").unwrap();
        let result = fast_forward(
            &repo,
            &new.to_string(),
            &repository::fingerprint(&repo).unwrap(),
        )
        .unwrap();
        assert_eq!(result["fastForwarded"], true);
        let repo = Repository::open(temp.path()).unwrap();
        assert_eq!(repo.head().unwrap().shorthand().unwrap(), "previous");
        assert_eq!(repo.head().unwrap().target(), Some(new));
        assert!(!repo.head_detached().unwrap());
        assert_eq!(fs::read(temp.path().join("file")).unwrap(), b"next");
        assert_eq!(fs::read(temp.path().join("local")).unwrap(), b"local edit");
        let result = fast_forward(
            &repo,
            &old.to_string(),
            &repository::fingerprint(&repo).unwrap(),
        )
        .unwrap();
        assert_eq!(result["alreadyUpToDate"], true);
        assert_eq!(repo.head().unwrap().target(), Some(new));
    }
    #[test]
    fn fast_forward_rejects_conflicts_and_divergence_without_ref_updates() {
        let (temp, repo, old, new) = fixture();
        run(
            &repo,
            CheckoutTarget::Branch {
                name: "previous".into(),
                expected_oid: old.to_string(),
            },
        )
        .unwrap();
        let repo = Repository::open(temp.path()).unwrap();
        fs::write(temp.path().join("file"), "my conflicting edit").unwrap();
        let before = fs::read(repo.path().join("index")).unwrap();
        assert_eq!(
            fast_forward(
                &repo,
                &new.to_string(),
                &repository::fingerprint(&repo).unwrap()
            )
            .unwrap_err()
            .code,
            "CHECKOUT_CONFLICT"
        );
        assert_eq!(repo.head().unwrap().target(), Some(old));
        assert_eq!(fs::read(repo.path().join("index")).unwrap(), before);
        assert_eq!(
            fs::read(temp.path().join("file")).unwrap(),
            b"my conflicting edit"
        );
        let repo = Repository::open(temp.path()).unwrap();
        let ours = commit(&repo, &[("file", "local branch commit")]);
        assert_eq!(
            fast_forward(
                &repo,
                &new.to_string(),
                &repository::fingerprint(&repo).unwrap()
            )
            .unwrap_err()
            .code,
            "DIVERGED_HISTORY"
        );
        assert_eq!(repo.head().unwrap().target(), Some(ours));
        assert_eq!(
            fs::read(temp.path().join("file")).unwrap(),
            b"local branch commit"
        );
    }
    #[test]
    fn batched_merge_attributes_check_each_tree_and_do_not_cache_across_calls() {
        let (temp, repo, _, new) = fixture();
        let plain = repo.find_commit(new).unwrap().tree().unwrap();
        let mut builder = repo.treebuilder(Some(&plain)).unwrap();
        for (rule, code) in [
            ("file filter=custom\n", "UNSUPPORTED_FILTER"),
            ("file merge=custom\n", "UNSUPPORTED_MERGE_DRIVER"),
        ] {
            builder
                .insert(
                    ".gitattributes",
                    repo.blob(rule.as_bytes()).unwrap(),
                    0o100644,
                )
                .unwrap();
            let target = repo.find_tree(builder.write().unwrap()).unwrap();
            let mut checks = MergeChecks::new(&repo);
            for _ in 0..2 {
                assert_eq!(checks.check(&target).unwrap_err().code, code);
            }
            assert_eq!(
                supported_merge_trees(&repo, &[plain.clone(), target])
                    .unwrap_err()
                    .code,
                code
            );
        }
        supported_merge_trees(&repo, &[plain.clone(), plain.clone()]).unwrap();
        fs::write(temp.path().join(".gitattributes"), "file filter=custom\n").unwrap();
        assert_eq!(
            supported_merge_trees(&repo, &[plain.clone(), plain])
                .unwrap_err()
                .code,
            "UNSUPPORTED_FILTER"
        );
    }

    #[test]
    fn batched_merge_attributes_check_paths_present_only_in_later_trees() {
        let (temp, repo, _, new) = fixture();
        let plain = repo.find_commit(new).unwrap().tree().unwrap();
        let mut builder = repo.treebuilder(Some(&plain)).unwrap();
        builder
            .insert("restored", repo.blob(b"content").unwrap(), 0o100644)
            .unwrap();
        let restored = repo.find_tree(builder.write().unwrap()).unwrap();
        fs::write(
            temp.path().join(".gitattributes"),
            "restored working-tree-encoding=UTF-16\n",
        )
        .unwrap();
        supported_merge_files(&repo, &plain).unwrap();
        assert_eq!(
            supported_merge_trees(&repo, &[plain, restored])
                .unwrap_err()
                .code,
            "UNSUPPORTED_FILTER"
        );
    }

    #[test]
    fn attribute_validation_walks_large_indexes_and_checks_late_target_rules() {
        let (_temp, repo, _, _) = fixture();
        let mut index = repo.index().unwrap();
        let template = index.get_path(Path::new("file"), 0).unwrap();
        for number in 0..20_001 {
            let mut entry = index.get_path(Path::new("file"), 0).unwrap();
            entry.path = format!("many/file-{number:05}").into_bytes();
            index.add(&entry).unwrap();
        }
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        supported_files(&repo, &repo.find_tree(tree_id).unwrap()).unwrap();

        let mut attributes = template;
        attributes.path = b".gitattributes".to_vec();
        attributes.id = repo.blob(b"many/file-20000 filter=custom\n").unwrap();
        index.add(&attributes).unwrap();
        let filtered = index.write_tree().unwrap();
        assert_eq!(
            supported_files(&repo, &repo.find_tree(filtered).unwrap())
                .unwrap_err()
                .code,
            "UNSUPPORTED_FILTER"
        );
    }

    #[test]
    fn attribute_union_checks_both_modes_for_shared_paths() {
        let (_temp, repo, _, new) = fixture();
        let target = repo.find_commit(new).unwrap().tree().unwrap();
        let mut index = repo.index().unwrap();
        let mut entry = index.get_path(Path::new("file"), 0).unwrap();
        entry.mode = 0o160000;
        entry.id = new;
        index.add(&entry).unwrap();
        index.write().unwrap();
        assert_eq!(
            supported_files(&repo, &target).unwrap_err().code,
            "UNSUPPORTED_CAPABILITY"
        );
        let submodule_tree = index.write_tree().unwrap();
        index.read_tree(&target).unwrap();
        index.write().unwrap();
        assert_eq!(
            supported_files(&repo, &repo.find_tree(submodule_tree).unwrap())
                .unwrap_err()
                .code,
            "UNSUPPORTED_CAPABILITY"
        );
    }

    #[test]
    fn rejects_destination_filters_and_stale_branches() {
        let (temp, repo, old, _) = fixture();
        let filtered = commit(&repo, &[(".gitattributes", "file filter=custom\n")]);
        // Switch using the fixture's raw engine to establish an unfiltered source.
        repo.set_head_detached(old).unwrap();
        repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
            .unwrap();
        assert!(!temp.path().join(".gitattributes").exists());
        assert_eq!(
            run(
                &repo,
                CheckoutTarget::Detached {
                    oid: filtered.to_string()
                }
            )
            .unwrap_err()
            .code,
            "UNSUPPORTED_FILTER"
        );
        assert_eq!(repo.head().unwrap().target(), Some(old));
        assert_eq!(
            run(
                &repo,
                CheckoutTarget::Branch {
                    name: "previous".into(),
                    expected_oid: filtered.to_string()
                }
            )
            .unwrap_err()
            .code,
            "STALE_REFERENCE"
        );
    }
}
