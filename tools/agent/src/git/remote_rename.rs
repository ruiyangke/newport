//! Rename local remote configuration and tracking refs without force replacement.
use super::{config_keys::in_section, protocol::Error, remotes, repository};
use git2::{ConfigLevel, Repository};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::PathBuf,
};
fn engine(_: git2::Error) -> Error {
    Error::new("REMOTE_ERROR", "Cannot prepare the remote rename.")
}
fn io_error(_: std::io::Error) -> Error {
    Error::new("IO_ERROR", "Cannot prepare remote configuration.")
}
fn unknown() -> Error {
    Error::new("OUTCOME_UNKNOWN","Remote configuration or tracking references may have changed. Inspect the saved operation before retrying.")
}
fn unsupported() -> Error {
    Error::new(
        "UNSUPPORTED_CONFIGURATION",
        "Renaming inherited, included, valueless or non-UTF-8 remote settings is not supported.",
    )
}
struct ConfigLock {
    path: PathBuf,
    published: bool,
}
impl Drop for ConfigLock {
    fn drop(&mut self) {
        if !self.published {
            let _ = fs::remove_file(&self.path);
        }
    }
}
fn mapped(value: &str, old: &str, new: &str) -> String {
    value
        .strip_prefix(old)
        .map(|suffix| format!("{new}{suffix}"))
        .unwrap_or_else(|| value.into())
}
fn refspec(value: &str, old: &str, new: &str, fetch: bool) -> String {
    let (force, value) = value
        .strip_prefix('+')
        .map(|v| ("+", v))
        .unwrap_or(("", value));
    if let Some((src, dst)) = value.split_once(':') {
        if fetch {
            format!("{force}{src}:{}", mapped(dst, old, new))
        } else {
            format!("{force}{}:{dst}", mapped(src, old, new))
        }
    } else if fetch {
        format!("{force}{value}")
    } else {
        format!("{force}{}", mapped(value, old, new))
    }
}
pub fn apply(
    repo: &Repository,
    old: &str,
    new: &str,
    expected_token: &str,
    expected: &str,
) -> Result<Value, Error> {
    remotes::name(old)?;
    remotes::name(new)?;
    if old == new {
        return Err(Error::invalid("Choose a different remote name."));
    }
    let old_refs = format!("refs/remotes/{old}/");
    let new_refs = format!("refs/remotes/{new}/");
    for remote in repo.remotes().map_err(engine)?.iter() {
        let remote = remote.map_err(engine)?.ok_or_else(unsupported)?;
        if remote == new {
            return Err(Error::new(
                "REMOTE_EXISTS",
                "The destination remote already exists.",
            ));
        }
        for candidate in [old, new] {
            if remote != old
                && (remote.starts_with(&format!("{candidate}/"))
                    || candidate.starts_with(&format!("{remote}/")))
            {
                return Err(Error::new(
                    "REMOTE_NAMESPACE_CONFLICT",
                    "Remote names have overlapping tracking namespaces.",
                ));
            }
        }
    }
    if old_refs.starts_with(&new_refs) || new_refs.starts_with(&old_refs) {
        return Err(Error::new(
            "REMOTE_NAMESPACE_CONFLICT",
            "The old and new tracking namespaces overlap.",
        ));
    }
    let config_path = repo.commondir().join("config");
    let metadata = fs::symlink_metadata(&config_path).map_err(io_error)?;
    if !metadata.is_file() || metadata.len() > 4 * 1024 * 1024 {
        return Err(unsupported());
    }
    let lock_path = repo.commondir().join("config.lock");
    let lock_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&lock_path)
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                Error::new("REPOSITORY_BUSY", "Git configuration is locked.")
            } else {
                io_error(e)
            }
        })?;
    let mut lock = ConfigLock {
        path: lock_path,
        published: false,
    };
    let fresh = Repository::open(repo.path()).map_err(engine)?;
    if repository::fingerprint(&fresh)? != expected {
        return Err(Error::new(
            "STALE_SNAPSHOT",
            "Refresh the repository before renaming its remote.",
        ));
    }
    if remotes::token(&fresh.find_remote(old).map_err(engine)?)? != expected_token {
        return Err(Error::new(
            "STALE_REMOTE",
            "Remote configuration changed. Refresh before renaming.",
        ));
    }
    let config = fresh.config().map_err(engine)?;
    let old_prefix = format!("remote.{old}.");
    let new_prefix = format!("remote.{new}.");
    let mut fields: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut references: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut entries = config.entries(None).map_err(engine)?;
    let mut count = 0;
    while let Some(entry) = entries.next() {
        count += 1;
        if count > 20_000 {
            return Err(Error::new(
                "LIMIT_EXCEEDED",
                "Configuration exceeds the rename entry limit.",
            ));
        }
        let entry = entry.map_err(engine)?;
        let key = entry.name().map_err(|_| unsupported())?;
        if in_section(key.as_bytes(), &new_prefix) {
            return Err(Error::new(
                "REMOTE_EXISTS",
                "Destination remote settings already exist.",
            ));
        }
        let remote_field = in_section(key.as_bytes(), &old_prefix);
        let branch_field = (key.starts_with("branch.")
            && (key.ends_with(".remote") || key.ends_with(".pushremote")))
            || key == "remote.pushdefault";
        if !remote_field && !branch_field {
            continue;
        }
        let value = entry.value().map_err(|_| unsupported())?;
        if value.len() > 16 * 1024 {
            return Err(Error::new(
                "LIMIT_EXCEEDED",
                "A remote setting is too large.",
            ));
        }
        if (remote_field || value == old)
            && (entry.level() != ConfigLevel::Local
                || entry.include_depth() != 0
                || !entry.has_value())
        {
            return Err(unsupported());
        }
        if remote_field {
            fields.entry(key.into()).or_default().push(value.into());
        } else {
            references.entry(key.into()).or_default().push(value.into());
        }
    }
    drop(entries);
    if fields.is_empty() {
        return Err(unsupported());
    }
    for values in references.values() {
        if values.iter().any(|v| v == old) && values.len() != 1 {
            return Err(unsupported());
        }
    }
    let mut moves = Vec::new();
    for reference in fresh.references().map_err(engine)? {
        let reference = reference.map_err(engine)?;
        let name = reference.name().map_err(|_| unsupported())?;
        if name == new_refs.trim_end_matches('/') || name.starts_with(&new_refs) {
            return Err(Error::new(
                "REFERENCE_EXISTS",
                "Destination tracking references already exist.",
            ));
        }
        if let Some(suffix) = name.strip_prefix(&old_refs) {
            moves.push((
                name.to_owned(),
                format!("{new_refs}{suffix}"),
                reference.target(),
                reference
                    .symbolic_target()
                    .map_err(|_| unsupported())?
                    .map(str::to_owned),
            ));
            if moves.len() > 10_000 {
                return Err(Error::new(
                    "LIMIT_EXCEEDED",
                    "Too many tracking references to rename.",
                ));
            }
        }
    }
    let mut transaction = fresh.transaction().map_err(engine)?;
    let names = moves
        .iter()
        .flat_map(|(old, new, _, _)| [old, new])
        .collect::<BTreeSet<_>>();
    for name in names {
        transaction.lock_ref(name).map_err(|_| {
            Error::new(
                "REPOSITORY_BUSY",
                "A tracking reference is locked or its destination conflicts.",
            )
        })?;
    }
    let temporary = tempfile::tempdir_in(repo.commondir()).map_err(io_error)?;
    let prepared_path = temporary.path().join("config");
    fs::copy(&config_path, &prepared_path).map_err(io_error)?;
    let mut prepared = git2::Config::open(&prepared_path).map_err(engine)?;
    for (key, values) in &fields {
        let suffix = key.strip_prefix(&old_prefix).ok_or_else(unsupported)?;
        let destination = format!("{new_prefix}{suffix}");
        for (i, value) in values.iter().enumerate() {
            let value = if suffix == "fetch" {
                refspec(value, &old_refs, &new_refs, true)
            } else if suffix == "push" {
                refspec(value, &old_refs, &new_refs, false)
            } else {
                value.clone()
            };
            if i == 0 {
                prepared.set_str(&destination, &value).map_err(engine)?;
            } else {
                prepared
                    .set_multivar(&destination, "a^", &value)
                    .map_err(engine)?;
            }
        }
        prepared.remove_multivar(key, ".*").map_err(engine)?;
    }
    let mut updated = Vec::new();
    for (key, values) in &references {
        if values.iter().any(|v| v == old) {
            prepared.set_str(key, new).map_err(engine)?;
            updated.push(key.clone());
        }
    }
    drop(prepared);
    let signature = fresh
        .signature()
        .or_else(|_| git2::Signature::now("Newport", "newport@localhost"))
        .map_err(engine)?;
    let mut logs = Vec::new();
    for (old_name, new_name, oid, symbolic) in &moves {
        let current = fresh.find_reference(old_name).map_err(engine)?;
        if current.target() != *oid
            || current.symbolic_target().map_err(engine)? != symbolic.as_deref()
        {
            return Err(Error::new(
                "STALE_REFERENCE",
                "A tracking reference moved before rename.",
            ));
        }
        match fresh.find_reference(new_name) {
            Ok(_) => {
                return Err(Error::new(
                    "REFERENCE_EXISTS",
                    "A destination tracking reference appeared.",
                ))
            }
            Err(e) if e.code() == git2::ErrorCode::NotFound => {}
            Err(e) => return Err(engine(e)),
        }
        if let Some(target) = symbolic {
            transaction
                .set_symbolic_target(
                    new_name,
                    &mapped(target, &old_refs, &new_refs),
                    Some(&signature),
                    "remote: rename (Newport)",
                )
                .map_err(engine)?;
        } else {
            transaction
                .set_target(
                    new_name,
                    oid.ok_or_else(unsupported)?,
                    Some(&signature),
                    "remote: rename (Newport)",
                )
                .map_err(engine)?;
        }
        if fresh.reference_has_log(new_name).map_err(engine)? {
            return Err(Error::new(
                "REFERENCE_EXISTS",
                "A destination reflog already exists.",
            ));
        }
        if fresh.reference_has_log(old_name).map_err(engine)? {
            logs.push((old_name, new_name));
        }
        transaction.remove(old_name).map_err(engine)?;
    }
    // Configuration is published before refs. Neither libgit2's ref transaction
    // nor this multi-resource operation promises all-or-nothing publication.
    fs::copy(&prepared_path, &lock.path).map_err(io_error)?;
    fs::set_permissions(
        &lock.path,
        fs::Permissions::from_mode(metadata.permissions().mode()),
    )
    .map_err(io_error)?;
    lock_file.sync_all().map_err(io_error)?;
    fs::rename(&lock.path, &config_path).map_err(io_error)?;
    lock.published = true;
    File::open(repo.commondir())
        .and_then(|f| f.sync_all())
        .map_err(|_| unknown())?;
    for (old_name, new_name) in logs {
        fresh
            .reflog_rename(old_name, new_name)
            .map_err(|_| unknown())?;
    }
    transaction.commit().map_err(|_| unknown())?;
    let fresh = Repository::open(repo.path()).map_err(|_| unknown())?;
    for reference in fresh.references().map_err(|_| unknown())? {
        let reference = reference.map_err(|_| unknown())?;
        if reference
            .name()
            .map_err(|_| unknown())?
            .starts_with(&old_refs)
        {
            return Err(unknown());
        }
    }
    let remote = fresh.find_remote(new).map_err(|_| unknown())?;
    Ok(
        json!({"oldName":old,"remote":new,"token":remotes::token(&remote).map_err(|_|unknown())?,"renamedReferences":moves.len(),"updatedSettings":updated,"refreshRequired":true}),
    )
}

#[cfg(test)]
mod tests {
    use super::super::{operations, protocol::Action};
    use super::*;
    fn fixture() -> (tempfile::TempDir, Repository, git2::Oid) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = Repository::init(tmp.path()).unwrap();
        let mut cfg = repo.config().unwrap();
        cfg.set_bool("core.logallrefupdates", true).unwrap();
        let sig = git2::Signature::now("Test", "test@example.test").unwrap();
        let tree = repo
            .find_tree(repo.treebuilder(None).unwrap().write().unwrap())
            .unwrap();
        let id = repo
            .commit(Some("HEAD"), &sig, &sig, "base", &tree, &[])
            .unwrap();
        drop(tree);
        repo.remote("origin", "https://example.test/project.git")
            .unwrap();
        repo.reference("refs/remotes/origin/main", id, true, "fetch fixture")
            .unwrap();
        repo.reference_symbolic(
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
            true,
            "fixture",
        )
        .unwrap();
        (tmp, repo, id)
    }
    fn request(repo: &Repository, new: &str) -> Action {
        Action::RemoteRename {
            name: "origin".into(),
            new_name: new.into(),
            expected_token: remotes::token(&repo.find_remote("origin").unwrap()).unwrap(),
        }
    }
    fn run(repo: &Repository, action: Action) -> Result<Value, Error> {
        operations::apply(repo, &action, &[], &repository::fingerprint(repo).unwrap())
    }
    #[test]
    fn rename_does_not_rewrite_a_dotted_neighbor_remote() {
        for neighbor in ["origin.backup", "upstream.backup"] {
            let (_temp, repo, id) = fixture();
            repo.remote(neighbor, "https://neighbor.invalid/untouched.git")
                .unwrap();
            let reference = format!("refs/remotes/{neighbor}/main");
            repo.reference(&reference, id, true, "neighbor").unwrap();
            let token = remotes::token(&repo.find_remote(neighbor).unwrap()).unwrap();
            let log = std::fs::read(repo.commondir().join("logs").join(&reference)).unwrap();
            run(&repo, request(&repo, "upstream")).unwrap();
            let fresh = Repository::open(repo.path()).unwrap();
            assert_eq!(
                remotes::token(&fresh.find_remote(neighbor).unwrap()).unwrap(),
                token
            );
            assert_eq!(fresh.refname_to_id(&reference).unwrap(), id);
            assert_eq!(
                std::fs::read(repo.commondir().join("logs").join(&reference)).unwrap(),
                log
            );
            assert_eq!(
                fresh.find_remote("upstream").unwrap().url().unwrap(),
                "https://example.test/project.git"
            );
        }
    }

    #[test]
    fn rename_preserves_custom_multivars_tracking_symbols_and_reflogs() {
        let (tmp, repo, id) = fixture();
        let mut cfg = repo.config().unwrap();
        cfg.set_multivar(
            "remote.origin.fetch",
            "a^",
            "+refs/heads/special:refs/remotes/origin/special",
        )
        .unwrap();
        cfg.set_multivar("remote.origin.fetch", "a^", "refs/tags/*:refs/tags/*")
            .unwrap();
        cfg.set_str(
            "remote.origin.push",
            "refs/remotes/origin/main:refs/heads/main",
        )
        .unwrap();
        cfg.set_str(
            "remote.origin.pushurl",
            "ssh://git@example.test/project.git",
        )
        .unwrap();
        cfg.set_str("branch.main.remote", "origin").unwrap();
        cfg.set_str("branch.main.merge", "refs/heads/main").unwrap();
        cfg.set_str("branch.main.pushremote", "origin").unwrap();
        cfg.set_str("remote.pushdefault", "origin").unwrap();
        let log = repo.reflog("refs/remotes/origin/main").unwrap();
        let entries = log.len();
        assert!(entries > 0);
        let result = run(&repo, request(&repo, "upstream")).unwrap();
        assert_eq!(result["renamedReferences"], 2);
        let repo = Repository::open(tmp.path()).unwrap();
        assert!(repo.find_remote("origin").is_err());
        let remote = repo.find_remote("upstream").unwrap();
        assert_eq!(remote.url().unwrap(), "https://example.test/project.git");
        assert_eq!(
            remote.pushurl().unwrap(),
            Some("ssh://git@example.test/project.git")
        );
        let specs = remote
            .fetch_refspecs()
            .unwrap()
            .iter_bytes()
            .map(|v| v.to_vec())
            .collect::<Vec<_>>();
        assert_eq!(
            specs,
            vec![
                b"+refs/heads/*:refs/remotes/upstream/*".to_vec(),
                b"+refs/heads/special:refs/remotes/upstream/special".to_vec(),
                b"refs/tags/*:refs/tags/*".to_vec()
            ]
        );
        assert_eq!(
            remote.push_refspecs().unwrap().get(0).unwrap().unwrap(),
            "refs/remotes/upstream/main:refs/heads/main"
        );
        assert_eq!(
            repo.find_reference("refs/remotes/upstream/main")
                .unwrap()
                .target(),
            Some(id)
        );
        assert_eq!(
            repo.find_reference("refs/remotes/upstream/HEAD")
                .unwrap()
                .symbolic_target()
                .unwrap(),
            Some("refs/remotes/upstream/main")
        );
        assert_eq!(
            repo.reflog("refs/remotes/upstream/main").unwrap().len(),
            entries + 1
        );
        for key in [
            "branch.main.remote",
            "branch.main.pushremote",
            "remote.pushdefault",
        ] {
            assert_eq!(repo.config().unwrap().get_string(key).unwrap(), "upstream");
        }
        assert_eq!(
            repo.config()
                .unwrap()
                .get_string("branch.main.merge")
                .unwrap(),
            "refs/heads/main"
        );
        assert_eq!(repo.head().unwrap().target(), Some(id));
    }
    #[test]
    fn destination_and_native_locks_preserve_source_configuration() {
        let (tmp, repo, id) = fixture();
        let original = fs::read(repo.path().join("config")).unwrap();
        repo.reference("refs/remotes/upstream/main", id, true, "collision")
            .unwrap();
        assert_eq!(
            run(&repo, request(&repo, "upstream")).unwrap_err().code,
            "REFERENCE_EXISTS"
        );
        assert_eq!(fs::read(repo.path().join("config")).unwrap(), original);
        repo.find_reference("refs/remotes/upstream/main")
            .unwrap()
            .delete()
            .unwrap();
        fs::write(repo.path().join("config.lock"), "external").unwrap();
        assert_eq!(
            run(&repo, request(&repo, "upstream")).unwrap_err().code,
            "REPOSITORY_BUSY"
        );
        assert_eq!(
            fs::read(repo.path().join("config.lock")).unwrap(),
            b"external"
        );
        fs::remove_file(repo.path().join("config.lock")).unwrap();
        fs::write(
            repo.path().join("refs/remotes/origin/main.lock"),
            "external",
        )
        .unwrap();
        assert_eq!(
            run(&repo, request(&repo, "upstream")).unwrap_err().code,
            "REPOSITORY_BUSY"
        );
        assert_eq!(fs::read(repo.path().join("config")).unwrap(), original);
        assert!(repo.find_remote("origin").is_ok());
        assert!(!repo.path().join("config.lock").exists());
        assert!(tmp.path().exists());
    }
    #[test]
    fn stale_and_included_configuration_are_refused() {
        let (tmp, repo, _) = fixture();
        let action = request(&repo, "upstream");
        repo.remote_set_url("origin", "https://example.test/changed.git")
            .unwrap();
        assert_eq!(run(&repo, action).unwrap_err().code, "STALE_REMOTE");
        let included = tmp.path().join("included");
        fs::write(&included, "[branch \"topic\"]\nremote = origin\n").unwrap();
        repo.config()
            .unwrap()
            .set_str("include.path", included.to_str().unwrap())
            .unwrap();
        let repo = Repository::open(tmp.path()).unwrap();
        let original = fs::read(repo.path().join("config")).unwrap();
        assert_eq!(
            run(&repo, request(&repo, "upstream")).unwrap_err().code,
            "UNSUPPORTED_CONFIGURATION"
        );
        assert_eq!(fs::read(repo.path().join("config")).unwrap(), original);
    }
}
