//! Local branch operations. Native libgit2 updates preserve branch configuration
//! and reflogs; a failure after entering an update can have a partial outcome.
use super::{
    protocol::{Action, Error},
    repository,
};
use git2::{BranchType, Oid, Repository};
use serde_json::{json, Value};

fn engine(error: git2::Error) -> Error {
    match error.code() {
        git2::ErrorCode::NotFound => {
            Error::new("BRANCH_NOT_FOUND", "The branch or commit no longer exists.")
        }
        git2::ErrorCode::Exists => {
            Error::new("BRANCH_EXISTS", "A branch with that name already exists.")
        }
        git2::ErrorCode::Locked => Error::new(
            "REPOSITORY_BUSY",
            "Another Git operation holds a reference lock.",
        ),
        _ => Error::new("GIT_ERROR", "The branch could not be inspected."),
    }
}
pub(super) fn name(value: &str) -> Result<(), Error> {
    if value.len() > 1024 || !git2::Branch::name_is_valid(value).unwrap_or(false) {
        return Err(Error::invalid(
            "Supply a valid local branch name of at most 1024 bytes.",
        ));
    }
    Ok(())
}
pub(super) fn oid(value: &str) -> Result<Oid, Error> {
    if value.len() != 40 {
        return Err(Error::invalid("A complete SHA-1 commit ID is required."));
    }
    Oid::from_str(value).map_err(|_| Error::invalid("Invalid commit ID."))
}
fn unused(repo: &Repository, value: &str) -> Result<(), Error> {
    name(value)?;
    match repo.find_branch(value, BranchType::Local) {
        Ok(_) => Err(Error::new(
            "BRANCH_EXISTS",
            "A branch with that name already exists.",
        )),
        Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(()),
        Err(e) => Err(engine(e)),
    }
}
fn checked_out(repo: &Repository, reference: &str) -> Result<bool, Error> {
    let head = repo.find_reference("HEAD").map_err(engine)?;
    Ok(head.symbolic_target_bytes() == Some(reference.as_bytes()))
}
/// Include the main worktree even when called from a linked worktree.
pub(super) fn other_worktree(repo: &Repository, reference: &str) -> Result<bool, Error> {
    let current = repo
        .path()
        .canonicalize()
        .map_err(|_| Error::new("IO_ERROR", "Cannot resolve the worktree."))?;
    let main = Repository::open(repo.commondir()).map_err(engine)?;
    if main.path().canonicalize().ok().as_ref() != Some(&current) && checked_out(&main, reference)?
    {
        return Ok(true);
    }
    let names = repo.worktrees().map_err(engine)?;
    for name in names.iter() {
        let name = name.map_err(|_| {
            Error::new(
                "UNSUPPORTED_CAPABILITY",
                "A linked worktree name is not valid UTF-8.",
            )
        })?;
        let name =
            name.ok_or_else(|| Error::new("GIT_ERROR", "A linked worktree name is missing."))?;
        let worktree = repo.find_worktree(name).map_err(engine)?;
        let linked = Repository::open_from_worktree(&worktree).map_err(engine)?;
        if linked.path().canonicalize().ok().as_ref() != Some(&current)
            && checked_out(&linked, reference)?
        {
            return Ok(true);
        }
    }
    Ok(false)
}
fn uncertain() -> Error {
    Error::new("OUTCOME_UNKNOWN", "The branch update could not be confirmed; its reference or configuration may have changed. Inspect the operation before retrying.")
}
/// Include unresolved configuration, not only a successfully resolved ref.
/// This also detects an upstream whose remote-tracking reference was pruned.
pub fn tracking(repo: &Repository, branch: &str) -> Result<Value, Error> {
    let config = repo.config().map_err(engine)?;
    tracking_in(&config, branch)
}
/// The same, against configuration the caller has already loaded -- one
/// snapshot for a whole branch listing rather than a reload per branch.
pub fn tracking_in(config: &git2::Config, branch: &str) -> Result<Value, Error> {
    let mut fields = serde_json::Map::new();
    let mut editable = true;
    for suffix in ["remote", "merge"] {
        let key = format!("branch.{branch}.{suffix}");
        let mut values = Vec::new();
        match config.multivar(&key, None) {
            Ok(mut entries) => {
                while let Some(entry) = entries.next() {
                    let entry = entry.map_err(engine)?;
                    let value = if entry.has_value() {
                        Some(entry.value_bytes())
                    } else {
                        None
                    };
                    if values.len() == 16 || value.is_some_and(|v| v.len() > 4096) {
                        return Err(Error::new(
                            "LIMIT_EXCEEDED",
                            "Upstream configuration exceeds the supported size.",
                        ));
                    }
                    editable &= value.is_some()
                        && entry.level() == git2::ConfigLevel::Local
                        && entry.include_depth() == 0;
                    values.push(json!({"value":value.map(super::protocol::Path::new),"level":format!("{:?}",entry.level()),"includeDepth":entry.include_depth()}));
                }
            }
            Err(e) if e.code() == git2::ErrorCode::NotFound => {}
            Err(e) => return Err(engine(e)),
        }
        editable &= values.len() <= 1;
        fields.insert(suffix.into(), json!(values));
    }
    let token = super::journal::hash(
        &serde_json::to_vec(&fields)
            .map_err(|_| Error::invalid("Invalid upstream configuration."))?,
    );
    Ok(json!({"token":token,"editable":editable,"configuration":fields}))
}

/// A local branch's upstream reference, resolved as Git resolves it --
/// `branch.<name>.remote` and `.merge`, the merge ref put through the remote's
/// first matching fetch refspec, and only if that reference exists -- but
/// against one configuration snapshot and with each remote's refspecs loaded
/// once. libgit2's `Branch::upstream` reloads both per branch, which on a
/// repository with a few hundred tracked branches was most of a second.
pub(super) fn upstream_in<'r>(
    repo: &'r Repository,
    config: &git2::Config,
    remotes: &mut std::collections::HashMap<String, Option<git2::Remote<'r>>>,
    branch: &str,
) -> Option<Vec<u8>> {
    let remote = config.get_str(&format!("branch.{branch}.remote")).ok()?;
    let merge = config.get_str(&format!("branch.{branch}.merge")).ok()?;
    let target = if remote == "." {
        merge.to_owned()
    } else {
        let loaded = remotes
            .entry(remote.to_owned())
            .or_insert_with(|| repo.find_remote(remote).ok());
        let spec = loaded.as_ref()?.refspecs().find(|spec| {
            // A negative refspec (`^refs/...`) excludes; it maps nothing.
            spec.direction() == git2::Direction::Fetch
                && spec.str().is_ok_and(|text| !text.starts_with('^'))
                && spec.src_matches(merge)
        })?;
        spec.transform(merge).ok()?.as_str().ok()?.to_owned()
    };
    repo.find_reference(&target).ok()?;
    Some(target.into_bytes())
}

fn set_upstream(
    repo: &Repository,
    branch_name: &str,
    expected_oid: &str,
    expected_token: &str,
    upstream: Option<&str>,
    expected: &str,
) -> Result<Value, Error> {
    name(branch_name)?;
    let reference = format!("refs/heads/{branch_name}");
    let mut refs = repo.transaction().map_err(engine)?;
    refs.lock_ref(&reference).map_err(engine)?;
    let mut branch = repo
        .find_branch(branch_name, BranchType::Local)
        .map_err(engine)?;
    if branch.get().target() != Some(oid(expected_oid)?) {
        return Err(Error::new(
            "STALE_REFERENCE",
            "The branch moved. Refresh before changing its upstream.",
        ));
    }
    let before = tracking(repo, branch_name)?;
    if before["token"] != expected_token {
        return Err(Error::new(
            "STALE_UPSTREAM",
            "Tracking configuration changed. Refresh branches before continuing.",
        ));
    }
    if before["editable"] != true {
        return Err(Error::new("UNSUPPORTED_CONFIGURATION", "Upstream settings inherited from includes or global configuration, or with multiple values, cannot be edited here yet."));
    }
    let shorthand = if let Some(target) = upstream {
        if target == reference {
            return Err(Error::invalid("A branch cannot track itself."));
        }
        let (name, remote) = if let Some(name) = target.strip_prefix("refs/heads/") {
            (name, false)
        } else if let Some(name) = target.strip_prefix("refs/remotes/") {
            (name, true)
        } else {
            return Err(Error::invalid(
                "Select a complete local or remote-tracking branch reference.",
            ));
        };
        self::name(name)?;
        let selected = repo.find_reference(target).map_err(engine)?;
        if selected.target().is_none() {
            return Err(Error::invalid(
                "Select a direct branch, not a symbolic alias.",
            ));
        }
        if remote && repo.find_branch(name, BranchType::Local).is_ok() {
            return Err(Error::new("AMBIGUOUS_UPSTREAM", "A local branch shadows this remote-tracking name. Rename that local branch before selecting it."));
        }
        // Validate remote/refspec resolution before libgit2 writes either key.
        if remote {
            repo.branch_remote_name(target).map_err(engine)?;
        }
        Some(name)
    } else {
        None
    };
    if repository::fingerprint(repo)? != expected
        || tracking(repo, branch_name)?["token"] != expected_token
    {
        return Err(Error::new(
            "STALE_SNAPSHOT",
            "The repository or tracking configuration changed before the update.",
        ));
    }
    if shorthand.is_some() {
        branch.set_upstream(shorthand).map_err(|_| uncertain())?;
    } else {
        // libgit2's unset helper fails if either key is absent. Remove only the
        // local keys that exist, so clearing an incomplete/already-clear setup
        // is well-defined and does not manufacture an uncertain outcome.
        let mut config = repo.config().map_err(engine)?;
        for suffix in ["remote", "merge"] {
            if before["configuration"][suffix]
                .as_array()
                .is_some_and(|v| !v.is_empty())
            {
                match config.remove(&format!("branch.{branch_name}.{suffix}")) {
                    Ok(()) => {}
                    Err(e) if e.code() == git2::ErrorCode::NotFound => {}
                    Err(_) => return Err(uncertain()),
                }
            }
        }
    }
    let fresh = Repository::open(repo.path()).map_err(|_| uncertain())?;
    let branch = fresh
        .find_branch(branch_name, BranchType::Local)
        .map_err(|_| uncertain())?;
    let actual = match branch.upstream() {
        Ok(upstream) => Some(upstream.get().name().map_err(|_| uncertain())?.to_owned()),
        Err(e) if e.code() == git2::ErrorCode::NotFound => None,
        Err(_) => return Err(uncertain()),
    };
    let after = tracking(&fresh, branch_name).map_err(|_| uncertain())?;
    let cleared = ["remote", "merge"].iter().all(|key| {
        after["configuration"][key]
            .as_array()
            .is_some_and(Vec::is_empty)
    });
    if actual.as_deref() != upstream
        || after["editable"] != true
        || (upstream.is_none() && !cleared)
    {
        return Err(uncertain());
    }
    Ok(json!({"name":branch_name,"upstream":actual,"tracking":after,"refreshRequired":true}))
}

pub fn apply(repo: &Repository, action: &Action, expected: &str) -> Result<Value, Error> {
    if repository::fingerprint(repo)? != expected {
        return Err(Error::new(
            "STALE_SNAPSHOT",
            "Refresh repository status before changing branches.",
        ));
    }
    match action {
        Action::BranchSetUpstream {
            name,
            expected_oid,
            expected_token,
            upstream,
        } => set_upstream(
            repo,
            name,
            expected_oid,
            expected_token,
            upstream.as_deref(),
            expected,
        ),
        Action::BranchCreate {
            name: branch_name,
            start_oid,
        } => {
            unused(repo, branch_name)?;
            let commit = repo.find_commit(oid(start_oid)?).map_err(engine)?;
            // force=false provides an atomic no-overwrite check in libgit2.
            repo.branch(branch_name, &commit, false)
                .map_err(|e| match e.code() {
                    git2::ErrorCode::Exists | git2::ErrorCode::Locked => engine(e),
                    _ => uncertain(),
                })?;
            Ok(json!({"name":branch_name,"oid":commit.id().to_string(),"refreshRequired":true}))
        }
        Action::BranchRename {
            name: branch_name,
            expected_oid,
            ..
        }
        | Action::BranchDelete {
            name: branch_name,
            expected_oid,
            ..
        } => {
            name(branch_name)?;
            let mut branch = repo
                .find_branch(branch_name, BranchType::Local)
                .map_err(engine)?;
            let expected_oid = oid(expected_oid)?;
            if branch.get().target() != Some(expected_oid) {
                return Err(Error::new(
                    "STALE_REFERENCE",
                    "The branch moved. Refresh branches before changing it.",
                ));
            }
            let reference = format!("refs/heads/{branch_name}");
            if other_worktree(repo, &reference)? {
                return Err(Error::new(
                    "BRANCH_IN_USE",
                    "This branch is checked out in another worktree.",
                ));
            }
            match action {
                Action::BranchRename { new_name, .. } => {
                    unused(repo, new_name)?;
                    branch.rename(new_name, false).map_err(|_| uncertain())?;
                    Ok(
                        json!({"oldName":branch_name,"name":new_name,"oid":expected_oid.to_string(),"refreshRequired":true}),
                    )
                }
                Action::BranchDelete { force, .. } => {
                    if checked_out(repo, &reference)? {
                        return Err(Error::new(
                            "BRANCH_IN_USE",
                            "Switch to another branch before deleting this branch.",
                        ));
                    }
                    if !force {
                        let head = repo
                            .head()
                            .map_err(engine)?
                            .peel_to_commit()
                            .map_err(engine)?
                            .id();
                        if head != expected_oid
                            && !repo
                                .graph_descendant_of(head, expected_oid)
                                .map_err(engine)?
                        {
                            return Err(Error::new("BRANCH_NOT_MERGED", "This branch contains commits not reachable from HEAD. Explicit force is required to delete it."));
                        }
                    }
                    // libgit2 deletes using the reference's expected old OID and
                    // removes its branch config. Config may change before a ref failure.
                    branch.delete().map_err(|_| uncertain())?;
                    Ok(
                        json!({"deleted":branch_name,"previousOid":expected_oid.to_string(),"refreshRequired":true}),
                    )
                }
                _ => unreachable!(),
            }
        }
        _ => Err(Error::invalid("Not a branch operation.")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, Repository, Oid) {
        let temp = tempfile::tempdir().unwrap();
        let repo = Repository::init(temp.path()).unwrap();
        std::fs::write(temp.path().join("file"), "base").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(std::path::Path::new("file")).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let oid = {
            let tree = repo.find_tree(tree_id).unwrap();
            let signature = git2::Signature::now("Fixture", "test@example.test").unwrap();
            repo.commit(Some("HEAD"), &signature, &signature, "base", &tree, &[])
                .unwrap()
        };
        (temp, repo, oid)
    }
    fn run(repo: &Repository, action: Action) -> Result<Value, Error> {
        apply(repo, &action, &repository::fingerprint(repo).unwrap())
    }
    fn tracking_action(repo: &Repository, oid: Oid, upstream: Option<&str>) -> Action {
        Action::BranchSetUpstream {
            name: "feature".into(),
            expected_oid: oid.to_string(),
            expected_token: tracking(repo, "feature").unwrap()["token"]
                .as_str()
                .unwrap()
                .into(),
            upstream: upstream.map(str::to_owned),
        }
    }
    #[test]
    fn upstream_selects_remote_or_local_and_clears_pruned_configuration() {
        let (temp, repo, oid) = fixture();
        repo.branch("feature", &repo.find_commit(oid).unwrap(), false)
            .unwrap();
        repo.branch("local-target", &repo.find_commit(oid).unwrap(), false)
            .unwrap();
        repo.remote("origin", "/unused/fixture.git").unwrap();
        repo.reference("refs/remotes/origin/main", oid, true, "fixture")
            .unwrap();
        let index = std::fs::read(repo.path().join("index")).unwrap();
        let result = apply(
            &repo,
            &tracking_action(&repo, oid, Some("refs/remotes/origin/main")),
            &repository::fingerprint(&repo).unwrap(),
        )
        .unwrap();
        assert_eq!(result["upstream"], "refs/remotes/origin/main");
        let repo = Repository::open(temp.path()).unwrap();
        assert_eq!(
            repo.config()
                .unwrap()
                .get_string("branch.feature.remote")
                .unwrap(),
            "origin"
        );
        assert_eq!(
            repo.config()
                .unwrap()
                .get_string("branch.feature.merge")
                .unwrap(),
            "refs/heads/main"
        );
        apply(
            &repo,
            &tracking_action(&repo, oid, Some("refs/heads/local-target")),
            &repository::fingerprint(&repo).unwrap(),
        )
        .unwrap();
        let repo = Repository::open(temp.path()).unwrap();
        assert_eq!(
            repo.config()
                .unwrap()
                .get_string("branch.feature.remote")
                .unwrap(),
            "."
        );
        apply(
            &repo,
            &tracking_action(&repo, oid, Some("refs/remotes/origin/main")),
            &repository::fingerprint(&repo).unwrap(),
        )
        .unwrap();
        let repo = Repository::open(temp.path()).unwrap();
        repo.find_reference("refs/remotes/origin/main")
            .unwrap()
            .delete()
            .unwrap();
        assert!(repo
            .find_branch("feature", BranchType::Local)
            .unwrap()
            .upstream()
            .is_err());
        assert!(
            !tracking(&repo, "feature").unwrap()["configuration"]["merge"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        apply(
            &repo,
            &tracking_action(&repo, oid, None),
            &repository::fingerprint(&repo).unwrap(),
        )
        .unwrap();
        let repo = Repository::open(temp.path()).unwrap();
        assert!(
            tracking(&repo, "feature").unwrap()["configuration"]["merge"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        apply(
            &repo,
            &tracking_action(&repo, oid, None),
            &repository::fingerprint(&repo).unwrap(),
        )
        .unwrap();
        // A partially configured branch can be cleared without needing the
        // missing half of the upstream configuration to exist.
        repo.config()
            .unwrap()
            .set_str("branch.feature.remote", "origin")
            .unwrap();
        apply(
            &repo,
            &tracking_action(&repo, oid, None),
            &repository::fingerprint(&repo).unwrap(),
        )
        .unwrap();
        let repo = Repository::open(temp.path()).unwrap();
        assert!(repo
            .config()
            .unwrap()
            .get_string("branch.feature.remote")
            .is_err());
        assert_eq!(std::fs::read(repo.path().join("index")).unwrap(), index);
        assert_eq!(std::fs::read(temp.path().join("file")).unwrap(), b"base");
    }
    #[test]
    fn upstream_rejects_stale_config_ambiguous_names_and_multivalue_settings() {
        let (_temp, repo, oid) = fixture();
        repo.branch("feature", &repo.find_commit(oid).unwrap(), false)
            .unwrap();
        let stale = tracking_action(&repo, oid, None);
        repo.config()
            .unwrap()
            .set_str("branch.feature.remote", "origin")
            .unwrap();
        assert_eq!(
            apply(&repo, &stale, &repository::fingerprint(&repo).unwrap())
                .unwrap_err()
                .code,
            "STALE_UPSTREAM"
        );
        repo.remote("origin", "/unused/fixture.git").unwrap();
        repo.reference("refs/remotes/origin/main", oid, true, "fixture")
            .unwrap();
        repo.branch("origin/main", &repo.find_commit(oid).unwrap(), false)
            .unwrap();
        assert_eq!(
            apply(
                &repo,
                &tracking_action(&repo, oid, Some("refs/remotes/origin/main")),
                &repository::fingerprint(&repo).unwrap()
            )
            .unwrap_err()
            .code,
            "AMBIGUOUS_UPSTREAM"
        );
        assert_eq!(
            apply(
                &repo,
                &tracking_action(&repo, oid, Some("refs/heads/feature")),
                &repository::fingerprint(&repo).unwrap()
            )
            .unwrap_err()
            .code,
            "INVALID_REQUEST"
        );
        assert_eq!(
            apply(
                &repo,
                &tracking_action(&repo, Oid::ZERO_SHA1, None),
                &repository::fingerprint(&repo).unwrap()
            )
            .unwrap_err()
            .code,
            "STALE_REFERENCE"
        );
        let mut config = repo.config().unwrap();
        config
            .set_multivar("branch.feature.merge", "^$", "refs/heads/a")
            .unwrap();
        config
            .set_multivar("branch.feature.merge", "^$", "refs/heads/b")
            .unwrap();
        assert_eq!(tracking(&repo, "feature").unwrap()["editable"], false);
        assert_eq!(
            apply(
                &repo,
                &tracking_action(&repo, oid, None),
                &repository::fingerprint(&repo).unwrap()
            )
            .unwrap_err()
            .code,
            "UNSUPPORTED_CONFIGURATION"
        );
    }

    #[test]
    fn create_rename_delete_and_preserve_configuration() {
        let (_temp, repo, oid) = fixture();
        let create = Action::BranchCreate {
            name: "topic".into(),
            start_oid: oid.to_string(),
        };
        run(&repo, create.clone()).unwrap();
        assert_eq!(run(&repo, create).unwrap_err().code, "BRANCH_EXISTS");
        repo.config()
            .unwrap()
            .set_str("branch.topic.remote", "origin")
            .unwrap();
        run(
            &repo,
            Action::BranchRename {
                name: "topic".into(),
                new_name: "renamed".into(),
                expected_oid: oid.to_string(),
            },
        )
        .unwrap();
        assert_eq!(
            repo.config()
                .unwrap()
                .get_string("branch.renamed.remote")
                .unwrap(),
            "origin"
        );
        assert!(repo.find_branch("topic", BranchType::Local).is_err());
        assert_eq!(
            run(
                &repo,
                Action::BranchDelete {
                    name: "renamed".into(),
                    expected_oid: "0".repeat(40),
                    force: false
                }
            )
            .unwrap_err()
            .code,
            "STALE_REFERENCE"
        );
        run(
            &repo,
            Action::BranchDelete {
                name: "renamed".into(),
                expected_oid: oid.to_string(),
                force: false,
            },
        )
        .unwrap();
        assert!(repo.find_branch("renamed", BranchType::Local).is_err());
        assert!(repo
            .config()
            .unwrap()
            .get_string("branch.renamed.remote")
            .is_err());
        let current = repo.head().unwrap().shorthand().unwrap().to_owned();
        assert_eq!(
            run(
                &repo,
                Action::BranchDelete {
                    name: current.clone(),
                    expected_oid: oid.to_string(),
                    force: true
                }
            )
            .unwrap_err()
            .code,
            "BRANCH_IN_USE"
        );
        run(
            &repo,
            Action::BranchRename {
                name: current,
                new_name: "main-renamed".into(),
                expected_oid: oid.to_string(),
            },
        )
        .unwrap();
        assert_eq!(repo.head().unwrap().shorthand().unwrap(), "main-renamed");
    }
    #[test]
    fn rejects_unmerged_and_branches_used_by_either_worktree() {
        let (_temp, repo, oid) = fixture();
        let base = repo.find_commit(oid).unwrap();
        let sig = git2::Signature::now("Fixture", "test@example.test").unwrap();
        let extra = repo
            .commit(None, &sig, &sig, "unique", &base.tree().unwrap(), &[&base])
            .unwrap();
        run(
            &repo,
            Action::BranchCreate {
                name: "unmerged".into(),
                start_oid: extra.to_string(),
            },
        )
        .unwrap();
        let delete = Action::BranchDelete {
            name: "unmerged".into(),
            expected_oid: extra.to_string(),
            force: false,
        };
        assert_eq!(run(&repo, delete).unwrap_err().code, "BRANCH_NOT_MERGED");
        run(
            &repo,
            Action::BranchDelete {
                name: "unmerged".into(),
                expected_oid: extra.to_string(),
                force: true,
            },
        )
        .unwrap();
        let parent = tempfile::tempdir().unwrap();
        let worktree = repo
            .worktree("linked", &parent.path().join("linked"), None)
            .unwrap();
        assert_eq!(
            run(
                &repo,
                Action::BranchDelete {
                    name: "linked".into(),
                    expected_oid: oid.to_string(),
                    force: true
                }
            )
            .unwrap_err()
            .code,
            "BRANCH_IN_USE"
        );
        assert_eq!(
            run(
                &repo,
                Action::BranchRename {
                    name: "linked".into(),
                    new_name: "moved".into(),
                    expected_oid: oid.to_string()
                }
            )
            .unwrap_err()
            .code,
            "BRANCH_IN_USE"
        );
        let linked = Repository::open_from_worktree(&worktree).unwrap();
        let main_name = repo.head().unwrap().shorthand().unwrap().to_owned();
        assert_eq!(
            run(
                &linked,
                Action::BranchDelete {
                    name: main_name,
                    expected_oid: oid.to_string(),
                    force: true
                }
            )
            .unwrap_err()
            .code,
            "BRANCH_IN_USE"
        );
    }
}
