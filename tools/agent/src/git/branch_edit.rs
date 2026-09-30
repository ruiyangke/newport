//! Guarded branch rename/delete. Native branch helpers do not combine their
//! configuration changes with an expected-OID check under a reference lock.
use super::super::config_keys::in_section;
use super::*;
use std::{collections::BTreeMap, fs, io::Write, os::unix::fs::OpenOptionsExt};

fn unsupported() -> Error {
    Error::new(
        "UNSUPPORTED_CONFIGURATION",
        "Branch settings must be local, UTF-8 values outside included configuration files.",
    )
}
fn io_error(_: std::io::Error) -> Error {
    Error::new("IO_ERROR", "Cannot prepare branch configuration.")
}
struct PreparedConfig {
    path: std::path::PathBuf,
    destination: std::path::PathBuf,
    file: fs::File,
    changed: bool,
    published: bool,
}
impl Drop for PreparedConfig {
    fn drop(&mut self) {
        if !self.published {
            let _ = fs::remove_file(&self.path);
        }
    }
}
impl PreparedConfig {
    fn prepare(repo: &Repository, old: &str, new: Option<&str>) -> Result<Self, Error> {
        let destination = repo.commondir().join("config");
        let metadata = fs::symlink_metadata(&destination).map_err(io_error)?;
        if !metadata.is_file() || metadata.len() > 4 * 1024 * 1024 {
            return Err(unsupported());
        }
        let path = destination.with_extension("lock");
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    Error::new("REPOSITORY_BUSY", "Git configuration is locked.")
                } else {
                    io_error(error)
                }
            })?;
        let mut prepared = Self {
            path,
            destination,
            file,
            changed: false,
            published: false,
        };
        let old_prefix = format!("branch.{old}.");
        let new_prefix = new.map(|name| format!("branch.{name}."));
        let fresh = Repository::open(repo.path()).map_err(engine)?;
        let config = fresh.config().map_err(engine)?;
        let mut entries = config.entries(None).map_err(engine)?;
        let mut fields: BTreeMap<String, Vec<String>> = BTreeMap::new();
        while let Some(entry) = entries.next() {
            let entry = entry.map_err(engine)?;
            if new_prefix
                .as_ref()
                .is_some_and(|prefix| in_section(entry.name_bytes(), prefix))
            {
                return Err(Error::new(
                    "BRANCH_EXISTS",
                    "Destination branch settings already exist.",
                ));
            }
            if !in_section(entry.name_bytes(), &old_prefix) {
                continue;
            }
            if entry.level() != git2::ConfigLevel::Local
                || entry.include_depth() != 0
                || !entry.has_value()
            {
                return Err(unsupported());
            }
            fields
                .entry(entry.name().map_err(|_| unsupported())?.to_owned())
                .or_default()
                .push(entry.value().map_err(|_| unsupported())?.to_owned());
        }
        if fields.is_empty() {
            return Ok(prepared);
        }
        let temp = tempfile::tempdir_in(repo.commondir()).map_err(io_error)?;
        let path = temp.path().join("config");
        fs::copy(&prepared.destination, &path).map_err(io_error)?;
        let mut config = git2::Config::open(&path).map_err(engine)?;
        for (key, values) in fields {
            if let Some(prefix) = &new_prefix {
                let renamed = format!("{prefix}{}", &key[old_prefix.len()..]);
                for (index, value) in values.iter().enumerate() {
                    if index == 0 {
                        config.set_str(&renamed, value).map_err(engine)?;
                    } else {
                        config.set_multivar(&renamed, "a^", value).map_err(engine)?;
                    }
                }
            }
            config.remove_multivar(&key, ".*").map_err(engine)?;
        }
        drop(config);
        prepared
            .file
            .write_all(&fs::read(&path).map_err(io_error)?)
            .map_err(io_error)?;
        prepared
            .file
            .set_permissions(metadata.permissions())
            .map_err(io_error)?;
        prepared.file.sync_all().map_err(io_error)?;
        prepared.changed = true;
        Ok(prepared)
    }
    fn publish(&mut self) -> Result<(), Error> {
        if self.changed {
            fs::rename(&self.path, &self.destination).map_err(|_| uncertain())?;
            self.published = true;
            fs::File::open(self.destination.parent().unwrap())
                .and_then(|dir| dir.sync_all())
                .map_err(|_| uncertain())?;
        }
        Ok(())
    }
}

pub(super) fn apply(repo: &Repository, action: &Action, expected: &str) -> Result<Value, Error> {
    apply_with(repo, action, expected, || {}, || {})
}
fn apply_with(
    repo: &Repository,
    action: &Action,
    expected: &str,
    before_lock: impl FnOnce(),
    locked: impl FnOnce(),
) -> Result<Value, Error> {
    let (old, expected_oid, new, force) = match action {
        Action::BranchRename {
            name,
            expected_oid,
            new_name,
        } => (name, expected_oid, Some(new_name.as_str()), false),
        Action::BranchDelete {
            name,
            expected_oid,
            force,
        } => (name, expected_oid, None, *force),
        _ => return Err(Error::invalid("Not a branch edit.")),
    };
    name(old)?;
    if let Some(new) = new {
        name(new)?;
        if new == old {
            return Err(Error::new(
                "BRANCH_EXISTS",
                "Choose a different branch name.",
            ));
        }
    }
    let expected_oid = oid(expected_oid)?;
    let source = format!("refs/heads/{old}");
    let destination = new.map(|new| format!("refs/heads/{new}"));
    let mut config = PreparedConfig::prepare(repo, old, new)?;
    before_lock();
    // Separate transactions control publication order: destination first, HEAD
    // second, source removal last. A partial failure cannot erase the only ref.
    let mut source_tx = repo.transaction().map_err(engine)?;
    source_tx.lock_ref(&source).map_err(engine)?;
    let mut head_tx = repo.transaction().map_err(engine)?;
    head_tx.lock_ref("HEAD").map_err(engine)?;
    let mut destination_tx = repo.transaction().map_err(engine)?;
    if let Some(destination) = &destination {
        destination_tx.lock_ref(destination).map_err(engine)?;
    }
    locked();
    let fresh = Repository::open(repo.path()).map_err(engine)?;
    if fresh.find_reference(&source).map_err(engine)?.target() != Some(expected_oid) {
        return Err(Error::new(
            "STALE_REFERENCE",
            "The branch moved. Refresh branches before changing it.",
        ));
    }
    if let Some(new) = new {
        unused(&fresh, new)?;
    }
    if other_worktree(&fresh, &source)? {
        return Err(Error::new(
            "BRANCH_IN_USE",
            "This branch is checked out in another worktree.",
        ));
    }
    let current = checked_out(&fresh, &source)?;
    if new.is_none() {
        if current {
            return Err(Error::new(
                "BRANCH_IN_USE",
                "Switch to another branch before deleting this branch.",
            ));
        }
        if !force {
            let head = fresh
                .head()
                .map_err(engine)?
                .peel_to_commit()
                .map_err(engine)?
                .id();
            if head != expected_oid
                && !fresh
                    .graph_descendant_of(head, expected_oid)
                    .map_err(engine)?
            {
                return Err(Error::new("BRANCH_NOT_MERGED", "This branch contains commits not reachable from HEAD. Explicit force is required to delete it."));
            }
        }
    }
    if repository::fingerprint(&fresh)? != expected {
        return Err(Error::new(
            "STALE_SNAPSHOT",
            "Refresh repository status before changing branches.",
        ));
    }
    let signature = fresh
        .signature()
        .or_else(|_| git2::Signature::now("Newport", "newport@localhost"))
        .map_err(engine)?;
    let message = format!(
        "branch: renamed {source} to {}",
        destination.as_deref().unwrap_or_default()
    );
    if let Some(destination) = &destination {
        if fresh.reference_has_log(destination).map_err(engine)? {
            return Err(Error::new(
                "BRANCH_EXISTS",
                "A destination branch reflog already exists.",
            ));
        }
        destination_tx
            .set_target(destination, expected_oid, Some(&signature), &message)
            .map_err(engine)?;
        if current {
            head_tx
                .set_symbolic_target("HEAD", destination, Some(&signature), &message)
                .map_err(engine)?;
        }
    }
    source_tx.remove(&source).map_err(engine)?;
    // All validation/preparation finishes before the first published change.
    // Refs, logs and config are separate resources: failures from here onward
    // are uncertain outcomes and must never invite automatic write retries.
    if let Some(destination) = &destination {
        if fresh.reference_has_log(&source).map_err(engine)? {
            fresh
                .reflog_rename(&source, destination)
                .map_err(|_| uncertain())?;
        }
        destination_tx.commit().map_err(|_| uncertain())?;
        head_tx.commit().map_err(|_| uncertain())?;
    }
    config.publish()?;
    source_tx.commit().map_err(|_| uncertain())?;
    Ok(if let Some(new) = new {
        json!({"oldName":old,"name":new,"oid":expected_oid.to_string(),"refreshRequired":true})
    } else {
        json!({"deleted":old,"previousOid":expected_oid.to_string(),"refreshRequired":true})
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, Repository, Oid, Oid) {
        let temp = tempfile::tempdir().unwrap();
        let repo = Repository::init(temp.path()).unwrap();
        let sig = git2::Signature::now("Test", "test@example.test").unwrap();
        let tree = repo.treebuilder(None).unwrap().write().unwrap();
        let old = repo
            .commit(
                Some("HEAD"),
                &sig,
                &sig,
                "old",
                &repo.find_tree(tree).unwrap(),
                &[],
            )
            .unwrap();
        let newer = repo
            .commit(
                None,
                &sig,
                &sig,
                "new",
                &repo.find_tree(tree).unwrap(),
                &[&repo.find_commit(old).unwrap()],
            )
            .unwrap();
        repo.branch("topic", &repo.find_commit(old).unwrap(), false)
            .unwrap();
        repo.config()
            .unwrap()
            .set_str("branch.topic.description", "keep me")
            .unwrap();
        (temp, repo, old, newer)
    }
    fn action(old: Oid, rename: bool) -> Action {
        if rename {
            Action::BranchRename {
                name: "topic".into(),
                new_name: "renamed".into(),
                expected_oid: old.to_string(),
            }
        } else {
            Action::BranchDelete {
                name: "topic".into(),
                expected_oid: old.to_string(),
                force: true,
            }
        }
    }
    #[test]
    fn moved_source_is_rejected_before_any_configuration_or_log_changes() {
        for rename in [false, true] {
            let (_temp, repo, old, newer) = fixture();
            let config = fs::read(repo.path().join("config")).unwrap();
            let expected = repository::fingerprint(&repo).unwrap();
            let error = apply_with(
                &repo,
                &action(old, rename),
                &expected,
                || {
                    Repository::open(repo.path())
                        .unwrap()
                        .reference("refs/heads/topic", newer, true, "external move")
                        .unwrap();
                },
                || {},
            )
            .unwrap_err();
            assert_eq!(error.code, "STALE_REFERENCE");
            assert_eq!(repo.refname_to_id("refs/heads/topic").unwrap(), newer);
            assert!(repo.find_reference("refs/heads/renamed").is_err());
            assert_eq!(fs::read(repo.path().join("config")).unwrap(), config);
            assert!(!repo.path().join("config.lock").exists());
            assert_eq!(
                repo.reflog("refs/heads/topic")
                    .unwrap()
                    .get(0)
                    .unwrap()
                    .id_new(),
                newer
            );
        }
    }
    #[test]
    fn native_writers_cannot_move_source_or_create_destination_while_validating() {
        for rename in [false, true] {
            let (_temp, repo, old, newer) = fixture();
            let expected = repository::fingerprint(&repo).unwrap();
            apply_with(
                &repo,
                &action(old, rename),
                &expected,
                || {},
                || {
                    let external = Repository::open(repo.path()).unwrap();
                    assert_eq!(
                        external
                            .reference("refs/heads/topic", newer, true, "external move")
                            .err()
                            .unwrap()
                            .code(),
                        git2::ErrorCode::Locked
                    );
                    if rename {
                        assert_eq!(
                            external
                                .reference("refs/heads/renamed", newer, false, "external create")
                                .err()
                                .unwrap()
                                .code(),
                            git2::ErrorCode::Locked
                        );
                    }
                    assert!(fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(repo.path().join("config.lock"))
                        .is_err());
                },
            )
            .unwrap();
            assert!(repo.find_reference("refs/heads/topic").is_err());
            if rename {
                assert_eq!(repo.refname_to_id("refs/heads/renamed").unwrap(), old);
            }
            assert!(!repo.path().join("config.lock").exists());
        }
    }
    #[test]
    fn rename_preserves_multivars_and_reflog_history() {
        let (_temp, repo, old, _) = fixture();
        let mut config = repo.config().unwrap();
        config
            .set_multivar("branch.topic.merge", "a^", "refs/heads/a")
            .unwrap();
        config
            .set_multivar("branch.topic.merge", "a^", "refs/heads/b")
            .unwrap();
        let original = repo.reflog("refs/heads/topic").unwrap();
        apply(
            &repo,
            &action(old, true),
            &repository::fingerprint(&repo).unwrap(),
        )
        .unwrap();
        let log = repo.reflog("refs/heads/renamed").unwrap();
        assert_eq!(log.len(), original.len() + 1);
        for index in 0..original.len() {
            assert_eq!(
                log.get(index + 1).unwrap().id_new(),
                original.get(index).unwrap().id_new()
            );
            assert_eq!(
                log.get(index + 1).unwrap().message_bytes(),
                original.get(index).unwrap().message_bytes()
            );
        }
        let config = Repository::open(repo.path()).unwrap().config().unwrap();
        let mut entries = config.multivar("branch.renamed.merge", None).unwrap();
        let mut values = Vec::new();
        while let Some(entry) = entries.next() {
            values.push(entry.unwrap().value().unwrap().to_owned());
        }
        assert_eq!(values, ["refs/heads/a", "refs/heads/b"]);
    }
    #[test]
    fn dotted_branch_subsections_are_not_moved_or_deleted_with_their_prefix() {
        for rename in [false, true] {
            let (_temp, repo, old, _) = fixture();
            let mut config = repo.config().unwrap();
            config
                .set_str("branch.topic.child.description", "source neighbor")
                .unwrap();
            config
                .set_str("branch.renamed.child.description", "destination neighbor")
                .unwrap();
            apply(
                &repo,
                &action(old, rename),
                &repository::fingerprint(&repo).unwrap(),
            )
            .unwrap();
            let config = Repository::open(repo.path()).unwrap().config().unwrap();
            assert_eq!(
                config.get_string("branch.topic.child.description").unwrap(),
                "source neighbor"
            );
            assert_eq!(
                config
                    .get_string("branch.renamed.child.description")
                    .unwrap(),
                "destination neighbor"
            );
        }
    }

    #[test]
    fn configuration_publish_failure_is_unknown_and_retains_source_ref() {
        let (_temp, repo, old, _) = fixture();
        let config = fs::read(repo.path().join("config")).unwrap();
        let expected = repository::fingerprint(&repo).unwrap();
        let error = apply_with(
            &repo,
            &action(old, true),
            &expected,
            || {},
            || {
                // Fault injection: prepared config loses its publication path.
                fs::remove_file(repo.path().join("config.lock")).unwrap();
            },
        )
        .unwrap_err();
        assert_eq!(error.code, "OUTCOME_UNKNOWN");
        assert_eq!(repo.refname_to_id("refs/heads/topic").unwrap(), old);
        assert_eq!(repo.refname_to_id("refs/heads/renamed").unwrap(), old);
        assert_eq!(fs::read(repo.path().join("config")).unwrap(), config);
    }

    #[test]
    fn inherited_settings_are_refused_before_reference_changes() {
        let (temp, repo, old, _) = fixture();
        let included = temp.path().join("included.cfg");
        fs::write(
            &included,
            "[branch \"topic\"]\n merge = refs/heads/remote\n",
        )
        .unwrap();
        repo.config()
            .unwrap()
            .set_str("include.path", included.to_str().unwrap())
            .unwrap();
        let config = fs::read(repo.path().join("config")).unwrap();
        for rename in [false, true] {
            assert_eq!(
                apply(
                    &repo,
                    &action(old, rename),
                    &repository::fingerprint(&repo).unwrap()
                )
                .unwrap_err()
                .code,
                "UNSUPPORTED_CONFIGURATION"
            );
            assert_eq!(repo.refname_to_id("refs/heads/topic").unwrap(), old);
            assert!(repo.find_reference("refs/heads/renamed").is_err());
            assert_eq!(fs::read(repo.path().join("config")).unwrap(), config);
        }
    }

    #[test]
    fn existing_destination_lock_preserves_source_configuration_and_logs() {
        let (_temp, repo, old, _) = fixture();
        let config = fs::read(repo.path().join("config")).unwrap();
        let log = fs::read(repo.path().join("logs/refs/heads/topic")).unwrap();
        fs::write(
            repo.path().join("refs/heads/renamed.lock"),
            b"external lock",
        )
        .unwrap();
        assert_eq!(
            apply(
                &repo,
                &action(old, true),
                &repository::fingerprint(&repo).unwrap()
            )
            .unwrap_err()
            .code,
            "REPOSITORY_BUSY"
        );
        assert_eq!(repo.refname_to_id("refs/heads/topic").unwrap(), old);
        assert_eq!(fs::read(repo.path().join("config")).unwrap(), config);
        assert_eq!(
            fs::read(repo.path().join("logs/refs/heads/topic")).unwrap(),
            log
        );
    }
}
