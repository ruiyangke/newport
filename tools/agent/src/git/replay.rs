//! Single-commit replay with automatic clean completion and explicit resume.
use super::{
    branches,
    integration::{self, Kind},
    operations,
    protocol::{Author, Error},
    repository,
};
use git2::Repository;
use serde_json::Value;

fn default_message(repo: &Repository, target: &str, kind: Kind) -> Result<String, Error> {
    let commit = repo
        .find_commit(branches::oid(target)?)
        .map_err(|_| Error::new("COMMIT_NOT_FOUND", "The selected commit no longer exists."))?;
    if kind == Kind::CherryPick {
        commit.message().map(str::to_owned).map_err(|_| {
            Error::new(
                "UNSUPPORTED_ENCODING",
                "Non-UTF-8 commit messages are not supported for replay yet.",
            )
        })
    } else {
        let summary = commit
            .summary()
            .map_err(|_| Error::new("UNSUPPORTED_ENCODING", "The commit summary is not UTF-8."))?
            .unwrap_or("");
        Ok(format!(
            "Revert \"{summary}\"\n\nThis reverts commit {target}.\n"
        ))
    }
}

pub fn start(
    repo: &Repository,
    target: &str,
    mainline: u32,
    kind: Kind,
    author: Option<&Author>,
    expected: &str,
) -> Result<Value, Error> {
    let message = default_message(repo, target, kind)?;
    operations::validate_commit(repo, &message)?;
    operations::signature(repo, author)?;
    let mut prepared = integration::prepare(repo, target, mainline, kind, expected)?;
    prepared.result["message"] = message.clone().into();
    if prepared.result["needsResolution"] == true {
        return Ok(prepared.result);
    }
    // Open the published index, not the preparation handle's temporary index.
    let fresh = Repository::open(repo.path()).map_err(|_| {
        Error::new(
            "OUTCOME_UNKNOWN",
            "Replay changed files but the repository could not be reopened.",
        )
    })?;
    let empty = fresh
        .index()
        .and_then(|mut i| i.write_tree())
        .and_then(|tree| {
            fresh
                .head()?
                .peel_to_commit()
                .map(|head| head.tree_id() == tree)
        })
        .map_err(|_| {
            Error::new(
                "OUTCOME_UNKNOWN",
                "Replay was prepared but its result could not be inspected.",
            )
        })?;
    if empty {
        prepared.result["empty"] = true.into();
        prepared.result["needsResolution"] = true.into();
        return Ok(prepared.result);
    }
    match operations::commit(&fresh, &message, author, &prepared.fingerprint) {
        Ok(mut result) => {
            result["targetOid"] = target.into();
            result["needsCommit"] = false.into();
            Ok(result)
        }
        Err(error) if error.code == "OUTCOME_UNKNOWN" => Err(error),
        Err(error) => {
            // A known refusal to commit does not undo the prepared replay. The
            // caller can refresh and continue/abort under a new operation ID.
            prepared.result["commitError"] =
                serde_json::to_value(error).expect("serializable error");
            prepared.result["needsResolution"] = true.into();
            Ok(prepared.result)
        }
    }
}

pub fn resume(
    repo: &Repository,
    message: Option<&str>,
    author: Option<&Author>,
    expected: &str,
) -> Result<Value, Error> {
    let record = integration::current_record(repo)?;
    let default;
    let message = if let Some(message) = message {
        message
    } else if record.kind == Kind::Merge {
        default = "Merge commit".to_owned();
        &default
    } else {
        default = default_message(repo, &record.target_oid, record.kind)?;
        &default
    };
    if repository::fingerprint(repo)? != expected {
        return Err(Error::new(
            "STALE_SNAPSHOT",
            "Refresh the integration before continuing.",
        ));
    }
    operations::commit(repo, message, author, expected)
}

#[cfg(test)]
mod tests {
    use super::super::protocol::Action;
    use super::*;
    use git2::{Oid, RepositoryState};
    use std::{fs, path::Path};
    fn author() -> Author {
        Author {
            name: "Current User".into(),
            email: "current@example.test".into(),
        }
    }
    fn fixture(conflict: bool) -> (tempfile::TempDir, Repository, Oid, Oid) {
        let temp = tempfile::tempdir().unwrap();
        let repo = Repository::init(temp.path()).unwrap();
        repo.config()
            .unwrap()
            .set_bool("commit.gpgsign", false)
            .unwrap();
        repo.config()
            .unwrap()
            .set_str(
                "core.hooksPath",
                repo.path().join("hooks").to_str().unwrap(),
            )
            .unwrap();
        let base = make_commit(&repo, None, "file", "base\n", "base");
        let ours = make_commit(&repo, Some(base), "file", "ours\n", "ours");
        repo.reference("refs/heads/main", ours, true, "fixture")
            .unwrap();
        repo.set_head("refs/heads/main").unwrap();
        repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
            .unwrap();
        let source = make_commit(
            &repo,
            Some(base),
            if conflict { "file" } else { "added" },
            "topic\n",
            "Topic message\n\nDetails",
        );
        (temp, repo, ours, source)
    }
    fn make_commit(
        repo: &Repository,
        parent: Option<Oid>,
        path: &str,
        text: &str,
        message: &str,
    ) -> Oid {
        let parent = parent.map(|id| repo.find_commit(id).unwrap());
        let tree = parent.as_ref().map(|c| c.tree().unwrap());
        let mut builder = repo.treebuilder(tree.as_ref()).unwrap();
        builder
            .insert(path, repo.blob(text.as_bytes()).unwrap(), 0o100644)
            .unwrap();
        let tree = repo.find_tree(builder.write().unwrap()).unwrap();
        let signature = git2::Signature::new(
            "Original Author",
            "original@example.test",
            &git2::Time::new(123456, 60),
        )
        .unwrap();
        repo.commit(
            None,
            &signature,
            &signature,
            message,
            &tree,
            &parent.iter().collect::<Vec<_>>(),
        )
        .unwrap()
    }
    fn run(repo: &Repository, action: Action) -> Result<Value, Error> {
        operations::apply(repo, &action, &[], &repository::fingerprint(repo).unwrap())
    }
    fn pick(oid: Oid) -> Action {
        Action::CherryPick {
            target_oid: oid.to_string(),
            mainline: 0,
            author: Some(author()),
        }
    }
    fn revert(oid: Oid) -> Action {
        Action::Revert {
            target_oid: oid.to_string(),
            mainline: 0,
            author: Some(author()),
        }
    }
    fn resume(repo: &Repository) -> Value {
        run(
            repo,
            Action::IntegrationContinue {
                message: None,
                author: Some(author()),
            },
        )
        .unwrap()
    }
    #[test]
    fn merge_mainline_and_detached_head_are_supported() {
        let (temp, repo, ours, source) = fixture(false);
        let ours_commit = repo.find_commit(ours).unwrap();
        let source_commit = repo.find_commit(source).unwrap();
        let mut merged = repo
            .merge_commits(&ours_commit, &source_commit, None)
            .unwrap();
        let tree = repo
            .find_tree(merged.write_tree_to(&repo).unwrap())
            .unwrap();
        let sig = git2::Signature::now("Fixture", "fixture@example.test").unwrap();
        let merge = repo
            .commit(
                None,
                &sig,
                &sig,
                "Merged source",
                &tree,
                &[&ours_commit, &source_commit],
            )
            .unwrap();
        assert_eq!(run(&repo, pick(merge)).unwrap_err().code, "INVALID_REQUEST");
        repo.set_head_detached(ours).unwrap();
        let result = run(
            &repo,
            Action::CherryPick {
                target_oid: merge.to_string(),
                mainline: 1,
                author: Some(author()),
            },
        )
        .unwrap();
        assert_eq!(result["needsCommit"], false, "{result}");
        let repo = Repository::open(temp.path()).unwrap();
        assert!(repo.head_detached().unwrap());
        let picked = repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(picked.parent_count(), 1);
        assert_eq!(picked.tree_id(), tree.id());
        let result = run(
            &repo,
            Action::Revert {
                target_oid: merge.to_string(),
                mainline: 1,
                author: Some(author()),
            },
        )
        .unwrap();
        assert_eq!(result["needsCommit"], false, "{result}");
        assert_eq!(
            repo.head().unwrap().peel_to_commit().unwrap().tree_id(),
            ours_commit.tree_id()
        );
    }
    #[test]
    fn empty_replay_requires_explicit_continue_or_abort() {
        let (temp, repo, _, source) = fixture(false);
        run(&repo, pick(source)).unwrap();
        let repo = Repository::open(temp.path()).unwrap();
        let original = repo.head().unwrap().target().unwrap();
        let result = run(&repo, pick(source)).unwrap();
        assert_eq!(result["empty"], true, "{result}");
        assert_eq!(repo.head().unwrap().target(), Some(original));
        let repo = Repository::open(temp.path()).unwrap();
        resume(&repo);
        let commit = repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(commit.parent_id(0).unwrap(), original);
        assert_eq!(
            commit.tree_id(),
            repo.find_commit(original).unwrap().tree_id()
        );
        let original = commit.id();
        let repo = Repository::open(temp.path()).unwrap();
        run(&repo, pick(source)).unwrap();
        let repo = Repository::open(temp.path()).unwrap();
        run(&repo, Action::IntegrationAbort {}).unwrap();
        assert_eq!(repo.head().unwrap().target(), Some(original));
        assert_eq!(repo.state(), RepositoryState::Clean);
    }
    #[test]
    fn guards_revert_restoration_collisions_and_tampered_recovery_state() {
        let (temp, repo, ours, source) = fixture(true);
        let empty = repo
            .find_tree(repo.treebuilder(None).unwrap().write().unwrap())
            .unwrap();
        let parent = repo.find_commit(ours).unwrap();
        let sig = git2::Signature::now("Fixture", "fixture@example.test").unwrap();
        let deleted = repo
            .commit(Some("HEAD"), &sig, &sig, "Delete file", &empty, &[&parent])
            .unwrap();
        repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
            .unwrap();
        fs::write(temp.path().join("file"), "untracked data").unwrap();
        assert_eq!(
            run(&repo, revert(deleted)).unwrap_err().code,
            "CHECKOUT_CONFLICT"
        );
        assert_eq!(
            fs::read(temp.path().join("file")).unwrap(),
            b"untracked data"
        );
        assert_eq!(repo.state(), RepositoryState::Clean);
        // Revert's preview restores a path absent from both input indexes.
        // Ignored files must receive the same protection as untracked files.
        repo.add_ignore_rule("file").unwrap();
        assert_eq!(
            run(&repo, revert(deleted)).unwrap_err().code,
            "CHECKOUT_CONFLICT"
        );
        assert_eq!(
            fs::read(temp.path().join("file")).unwrap(),
            b"untracked data"
        );
        assert_eq!(repo.head().unwrap().target(), Some(deleted));
        assert_eq!(repo.state(), RepositoryState::Clean);
        repo.clear_ignore_rules().unwrap();
        fs::remove_file(temp.path().join("file")).unwrap();
        repo.reference("refs/heads/main", ours, true, "fixture")
            .unwrap();
        repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
            .unwrap();
        run(&repo, pick(source)).unwrap();
        let repo = Repository::open(temp.path()).unwrap();
        fs::write(repo.path().join("CHERRY_PICK_HEAD"), format!("{ours}\n")).unwrap();
        assert_eq!(
            run(&repo, Action::IntegrationAbort {}).unwrap_err().code,
            "RECOVERY_REQUIRED"
        );
        assert!(repo.index().unwrap().has_conflicts());
        assert_eq!(repo.head().unwrap().target(), Some(ours));
    }

    #[test]
    fn prepared_fingerprint_does_not_authorize_later_index_edits() {
        let (temp, repo, ours, source) = fixture(false);
        let prepared = integration::prepare(
            &repo,
            &source.to_string(),
            0,
            Kind::CherryPick,
            &repository::fingerprint(&repo).unwrap(),
        )
        .unwrap();
        let repo = Repository::open(temp.path()).unwrap();
        assert_eq!(
            prepared.fingerprint,
            repository::fingerprint(&repo).unwrap()
        );
        fs::write(temp.path().join("external"), "external staging").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("external")).unwrap();
        index.write().unwrap();
        let repo = Repository::open(temp.path()).unwrap();
        let error = operations::commit(&repo, "Replay", Some(&author()), &prepared.fingerprint)
            .unwrap_err();
        assert_eq!(error.code, "STALE_SNAPSHOT");
        assert_eq!(repo.head().unwrap().target(), Some(ours));
        run(&repo, Action::IntegrationAbort {}).unwrap();
        let repo = Repository::open(temp.path()).unwrap();
        assert!(repo
            .index()
            .unwrap()
            .get_path(Path::new("external"), 0)
            .is_some());
        assert_eq!(
            fs::read(temp.path().join("external")).unwrap(),
            b"external staging"
        );
        assert!(!temp.path().join("added").exists());
    }

    #[test]
    fn clean_pick_preserves_authorship_and_revert_commits_the_inverse() {
        let (temp, repo, ours, source) = fixture(false);
        let result = run(&repo, pick(source)).unwrap();
        assert_eq!(result["needsCommit"], false, "{result}");
        let repo = Repository::open(temp.path()).unwrap();
        let commit = repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(commit.parent_count(), 1);
        assert_eq!(commit.parent_id(0).unwrap(), ours);
        assert_eq!(commit.message().unwrap(), "Topic message\n\nDetails");
        assert_eq!(commit.author().name().unwrap(), "Original Author");
        assert_eq!(commit.author().when().seconds(), 123456);
        assert_eq!(commit.committer().name().unwrap(), "Current User");
        assert_eq!(repo.state(), RepositoryState::Clean);
        assert_eq!(fs::read(temp.path().join("file")).unwrap(), b"ours\n");
        assert_eq!(fs::read(temp.path().join("added")).unwrap(), b"topic\n");
        let picked = commit.id();
        drop(commit);
        let result = run(&repo, revert(picked)).unwrap();
        assert_eq!(result["needsCommit"], false, "{result}");
        let repo = Repository::open(temp.path()).unwrap();
        let commit = repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(commit.parent_id(0).unwrap(), picked);
        assert_eq!(commit.tree_id(), repo.find_commit(ours).unwrap().tree_id());
        assert_eq!(commit.author().name().unwrap(), "Current User");
        assert!(commit
            .message()
            .unwrap()
            .starts_with("Revert \"Topic message\""));
        assert!(!temp.path().join("added").exists());
        assert!(!repo.path().join("newport-merge.json").exists());
    }
    #[test]
    fn conflicted_pick_can_abort_or_resume_after_reopening() {
        let (temp, repo, ours, source) = fixture(true);
        let result = run(&repo, pick(source)).unwrap();
        assert_eq!(result["needsResolution"], true, "{result}");
        let repo = Repository::open(temp.path()).unwrap();
        assert_eq!(repo.state(), RepositoryState::CherryPick);
        assert_eq!(
            run(
                &repo,
                Action::IntegrationContinue {
                    message: None,
                    author: Some(author())
                }
            )
            .unwrap_err()
            .code,
            "UNMERGED_INDEX"
        );
        fs::write(temp.path().join("keep"), "staged").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("keep")).unwrap();
        index.write().unwrap();
        fs::write(temp.path().join("keep"), "working").unwrap();
        run(&repo, Action::IntegrationAbort {}).unwrap();
        let repo = Repository::open(temp.path()).unwrap();
        assert_eq!(repo.head().unwrap().target(), Some(ours));
        assert_eq!(repo.state(), RepositoryState::Clean);
        assert_eq!(fs::read(temp.path().join("file")).unwrap(), b"ours\n");
        assert_eq!(fs::read(temp.path().join("keep")).unwrap(), b"working");
        assert_eq!(
            repo.find_blob(
                repo.index()
                    .unwrap()
                    .get_path(Path::new("keep"), 0)
                    .unwrap()
                    .id
            )
            .unwrap()
            .content(),
            b"staged"
        );
        let mut index = repo.index().unwrap();
        index.remove_path(Path::new("keep")).unwrap();
        index.write().unwrap();
        fs::remove_file(temp.path().join("keep")).unwrap();
        let repo = Repository::open(temp.path()).unwrap();
        run(&repo, pick(source)).unwrap();
        let repo = Repository::open(temp.path()).unwrap();
        fs::write(temp.path().join("file"), "resolution\n").unwrap();
        operations::apply(
            &repo,
            &Action::Stage {
                hunks: None,
                entry_ids: vec![],
            },
            &[b"file".to_vec()],
            &repository::fingerprint(&repo).unwrap(),
        )
        .unwrap();
        let repo = Repository::open(temp.path()).unwrap();
        let result = resume(&repo);
        assert_eq!(result["integrationCompleted"], "cherry_pick");
        let commit = repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(commit.parent_count(), 1);
        assert_eq!(commit.parent_id(0).unwrap(), ours);
        assert_eq!(commit.author().when().seconds(), 123456);
        assert_eq!(repo.state(), RepositoryState::Clean);
    }
    #[test]
    fn revert_conflicts_can_abort_then_resolve_and_continue() {
        let (temp, repo, ours, _) = fixture(true);
        let later = make_commit(&repo, Some(ours), "file", "later\n", "later");
        repo.reference("refs/heads/main", later, true, "fixture")
            .unwrap();
        repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
            .unwrap();
        let result = run(&repo, revert(ours)).unwrap();
        assert_eq!(result["needsResolution"], true, "{result}");
        let repo = Repository::open(temp.path()).unwrap();
        assert_eq!(repo.state(), RepositoryState::Revert);
        run(&repo, Action::IntegrationAbort {}).unwrap();
        assert_eq!(fs::read(temp.path().join("file")).unwrap(), b"later\n");
        let repo = Repository::open(temp.path()).unwrap();
        run(&repo, revert(ours)).unwrap();
        let repo = Repository::open(temp.path()).unwrap();
        fs::write(temp.path().join("file"), "resolved revert\n").unwrap();
        operations::apply(
            &repo,
            &Action::Stage {
                hunks: None,
                entry_ids: vec![],
            },
            &[b"file".to_vec()],
            &repository::fingerprint(&repo).unwrap(),
        )
        .unwrap();
        let repo = Repository::open(temp.path()).unwrap();
        resume(&repo);
        let commit = repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(commit.parent_id(0).unwrap(), later);
        assert_eq!(commit.author().name().unwrap(), "Current User");
        assert_eq!(repo.state(), RepositoryState::Clean);
    }
}
