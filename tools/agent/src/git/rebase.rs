//! Bounded, reconnectable native rebases with path-scoped abort recovery.
use super::{
    branches, checkout, integration, operations,
    protocol::{Author, Error, Path as WirePath},
    repository,
};
use git2::{Repository, RepositoryState};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    ffi::OsStr,
    fs::{self, File},
    io::Write,
    os::unix::ffi::OsStrExt,
    path::{Component, Path},
};
const MARKER: &str = "newport-rebase.json";
const MAX_COMMITS: usize = 256;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    original: String,
    branch: Option<String>,
    onto: String,
    commits: Vec<String>,
    position: Option<usize>,
    head: String,
    awaiting: bool,
    paths: Vec<WirePath>,
    current_paths: Vec<WirePath>,
    native: String,
    committer: Author,
}
fn engine(_: git2::Error) -> Error {
    Error::new("GIT_ERROR", "Cannot inspect or prepare this rebase.")
}
fn io_error(_: std::io::Error) -> Error {
    recovery()
}
fn recovery() -> Error {
    Error::new(
        "RECOVERY_REQUIRED",
        "Rebase metadata changed or is not owned by Newport. Inspect it before recovery.",
    )
}
fn unknown() -> Error {
    Error::new(
        "OUTCOME_UNKNOWN",
        "Rebase may have changed files or references. Inspect its state before retrying.",
    )
}
pub fn active(repo: &Repository) -> bool {
    repo.path().join(MARKER).exists()
        || matches!(
            repo.state(),
            RepositoryState::Rebase
                | RepositoryState::RebaseInteractive
                | RepositoryState::RebaseMerge
        )
}
pub fn native_hash(repo: &Repository) -> Result<String, Error> {
    let directory = repo.path().join("rebase-merge");
    let mut hash = Sha256::new();
    match fs::symlink_metadata(&directory) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
        }
        Ok(m) if m.is_dir() => {}
        _ => return Err(recovery()),
    }
    let mut files = Vec::new();
    for entry in fs::read_dir(directory).map_err(io_error)? {
        files.push(entry.map_err(io_error)?.path());
        if files.len() > 300 {
            return Err(recovery());
        }
    }
    files.sort();
    let mut total = 0;
    for file in files {
        let meta = fs::symlink_metadata(&file).map_err(io_error)?;
        total += meta.len();
        if !meta.is_file() || total > 4 * 1024 * 1024 {
            return Err(recovery());
        }
        let name = file.file_name().ok_or_else(recovery)?.as_bytes();
        hash.update((name.len() as u64).to_be_bytes());
        hash.update(name);
        let bytes = fs::read(file).map_err(io_error)?;
        hash.update((bytes.len() as u64).to_be_bytes());
        hash.update(bytes);
    }
    Ok(hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
}
fn paths(values: &[WirePath]) -> Result<BTreeSet<Vec<u8>>, Error> {
    if values.len() > 20_000 {
        return Err(recovery());
    }
    values.iter().map(|v| {
        let bytes = v.decode()?;
        if Path::new(OsStr::from_bytes(&bytes)).components().any(|c| !matches!(c, Component::Normal(n) if !n.as_bytes().eq_ignore_ascii_case(b".git"))) { return Err(recovery()); }
        Ok(bytes)
    }).collect()
}
fn save(repo: &Repository, record: &Record, initial: bool) -> Result<(), Error> {
    let bytes = serde_json::to_vec(record).map_err(|_| recovery())?;
    if bytes.len() > 1024 * 1024 {
        return Err(recovery());
    }
    let mut file = tempfile::NamedTempFile::new_in(repo.path()).map_err(io_error)?;
    file.write_all(&bytes).map_err(io_error)?;
    file.as_file().sync_all().map_err(io_error)?;
    if initial {
        file.persist_noclobber(repo.path().join(MARKER))
            .map_err(|_| recovery())?;
    } else {
        file.persist(repo.path().join(MARKER))
            .map_err(|_| recovery())?;
    }
    File::open(repo.path())
        .and_then(|f| f.sync_all())
        .map_err(io_error)
}
fn load(repo: &Repository) -> Result<Record, Error> {
    let meta = fs::symlink_metadata(repo.path().join(MARKER)).map_err(io_error)?;
    if !meta.is_file() || meta.len() > 1024 * 1024 {
        return Err(recovery());
    }
    let record: Record =
        serde_json::from_slice(&fs::read(repo.path().join(MARKER)).map_err(io_error)?)
            .map_err(|_| recovery())?;
    paths(&record.paths)?;
    paths(&record.current_paths)?;
    if record.version != 1
        || record.commits.len() > MAX_COMMITS
        || record.native != native_hash(repo)?
        || !repo.head_detached().map_err(engine)?
        || repo.head().map_err(engine)?.target() != Some(branches::oid(&record.head)?)
    {
        return Err(recovery());
    }
    if let Some(branch) = &record.branch {
        if !branch.starts_with("refs/heads/")
            || !git2::Reference::is_valid_name(branch)
            || repo
                .find_reference(branch)
                .map_err(|_| recovery())?
                .target()
                != Some(branches::oid(&record.original)?)
        {
            return Err(recovery());
        }
    }
    let mut rebase = repo.open_rebase(None).map_err(|_| recovery())?;
    if rebase.orig_head_name().map_err(|_| recovery())? != record.branch.as_deref()
        || rebase.orig_head_id() != Some(branches::oid(&record.original)?)
        || rebase.operation_current() != record.position
        || rebase.len() != record.commits.len()
    {
        return Err(recovery());
    }
    for (i, oid) in record.commits.iter().enumerate() {
        if rebase.nth(i).map(|op| op.id().to_string()).as_ref() != Some(oid) {
            return Err(recovery());
        }
    }
    Ok(record)
}
pub fn validate(repo: &Repository) -> Result<(), Error> {
    load(repo).map(|_| ())
}
pub fn status(repo: &Repository) -> Value {
    match load(repo) {
        Ok(r) => {
            json!({"kind":"rebase","managed":true,"originalOid":r.original,"ontoOid":r.onto,"position":r.position.map(|p|p+1),"total":r.commits.len(),"awaitingCommit":r.awaiting,"canContinue":true,"canSkip":r.awaiting,"canAbort":true})
        }
        Err(_) => {
            json!({"kind":"rebase","managed":false,"canContinue":false,"canSkip":false,"canAbort":false})
        }
    }
}
fn check(repo: &Repository, expected: &str) -> Result<(), Error> {
    if repository::fingerprint(repo)? != expected {
        return Err(Error::new(
            "STALE_SNAPSHOT",
            "Refresh the rebase before changing it.",
        ));
    }
    Ok(())
}
fn diff_paths(
    repo: &Repository,
    before: &git2::Tree<'_>,
    after: &git2::Tree<'_>,
) -> Result<BTreeSet<Vec<u8>>, Error> {
    let diff = repo
        .diff_tree_to_tree(Some(before), Some(after), None)
        .map_err(engine)?;
    let mut paths = BTreeSet::new();
    for d in diff.deltas() {
        for file in [d.old_file(), d.new_file()] {
            if let Some(p) = file.path_bytes() {
                paths.insert(p.to_vec());
            }
        }
    }
    if paths.len() > 20_000 {
        return Err(Error::new(
            "LIMIT_EXCEEDED",
            "This rebase changes too many paths.",
        ));
    }
    Ok(paths)
}
fn options() -> git2::RebaseOptions<'static> {
    let mut checkout = git2::build::CheckoutBuilder::new();
    checkout
        .safe()
        .allow_conflicts(true)
        .overwrite_ignored(false);
    let mut options = git2::RebaseOptions::new();
    options.checkout_options(checkout);
    options
}
fn private(repo: &Repository) -> Result<tempfile::TempDir, Error> {
    let tmp = tempfile::tempdir_in(repo.path()).map_err(io_error)?;
    fs::copy(repo.path().join("index"), tmp.path().join("index")).map_err(io_error)?;
    operations::private_index(repo, &tmp.path().join("index"))?;
    Ok(tmp)
}
pub fn start(
    repo: &Repository,
    upstream: &str,
    onto: Option<&str>,
    committer: Option<&Author>,
    expected: &str,
) -> Result<Value, Error> {
    if repo.is_bare() || repo.state() != RepositoryState::Clean || active(repo) {
        return Err(Error::new(
            "INTEGRATION_IN_PROGRESS",
            "Rebase requires a clean, non-bare repository.",
        ));
    }
    operations::validate_commit(repo, "Rebase")?;
    for hook in ["pre-rebase", "post-rewrite", "post-checkout"] {
        checkout::supported_hook(repo, hook)?;
    }
    let signature = operations::signature(repo, committer)?;
    let head = repo.head().map_err(engine)?;
    let original = head.peel_to_commit().map_err(engine)?;
    let branch = if head.is_branch() {
        Some(head.name().map_err(engine)?.to_owned())
    } else {
        None
    };
    let annotated = repo.reference_to_annotated_commit(&head).map_err(engine)?;
    let upstream = repo
        .find_annotated_commit(branches::oid(upstream)?)
        .map_err(engine)?;
    let onto = repo
        .find_annotated_commit(
            onto.map(branches::oid)
                .transpose()?
                .unwrap_or(upstream.id()),
        )
        .map_err(engine)?;
    let target = repo.find_commit(onto.id()).map_err(engine)?;
    let mut attributes = checkout::MergeChecks::new(repo);
    attributes.check(&original.tree().map_err(engine)?)?;
    attributes.check(&target.tree().map_err(engine)?)?;
    // libgit2 omits merge commits from its replay list; do not silently flatten them.
    let mut walk = repo.revwalk().map_err(engine)?;
    walk.push(original.id()).map_err(engine)?;
    walk.hide(upstream.id()).map_err(engine)?;
    for (count, id) in walk.enumerate() {
        if count >= MAX_COMMITS {
            return Err(Error::new(
                "LIMIT_EXCEEDED",
                "Rebase is limited to 256 commits.",
            ));
        }
        if repo
            .find_commit(id.map_err(engine)?)
            .map_err(engine)?
            .parent_count()
            != 1
        {
            return Err(Error::new(
                "UNSUPPORTED_CAPABILITY",
                "Rebasing merge or root commits is not supported yet.",
            ));
        }
    }
    if let Some(branch) = &branch {
        if branches::other_worktree(repo, branch)? {
            return Err(Error::new(
                "BRANCH_IN_USE",
                "The branch is checked out in another worktree.",
            ));
        }
    }
    let mut preview_options = options();
    preview_options.inmemory(true);
    let mut preview = repo
        .rebase(
            Some(&annotated),
            Some(&upstream),
            Some(&onto),
            Some(&mut preview_options),
        )
        .map_err(engine)?;
    if preview.len() > MAX_COMMITS {
        return Err(Error::new(
            "LIMIT_EXCEEDED",
            "Rebase is limited to 256 commits per operation.",
        ));
    }
    let mut affected = diff_paths(
        repo,
        &original.tree().map_err(engine)?,
        &target.tree().map_err(engine)?,
    )?;
    let mut commits = Vec::new();
    for i in 0..preview.len() {
        let id = preview.nth(i).ok_or_else(recovery)?.id();
        let commit = repo.find_commit(id).map_err(engine)?;
        if commit.parent_count() != 1 {
            return Err(Error::new(
                "UNSUPPORTED_CAPABILITY",
                "Only linear commit replay is supported.",
            ));
        }
        let tree = commit.tree().map_err(engine)?;
        let parent = commit.parent(0).map_err(engine)?.tree().map_err(engine)?;
        attributes.check(&tree)?;
        attributes.check(&parent)?;
        operations::validate_commit(repo, commit.message().map_err(engine)?)?;
        affected.extend(diff_paths(repo, &parent, &tree)?);
        commits.push(id.to_string());
    }
    drop(preview);
    drop(attributes);
    integration::guard_paths(repo, &affected, true)?;
    let mut lock = operations::IndexLock::acquire(repo)?;
    check(repo, expected)?;
    let temporary = private(repo)?;
    let mut record = Record {
        version: 1,
        original: original.id().to_string(),
        branch,
        onto: onto.id().to_string(),
        commits,
        position: None,
        head: original.id().to_string(),
        awaiting: false,
        paths: affected.iter().map(|p| WirePath::new(p)).collect(),
        current_paths: vec![],
        native: String::new(),
        committer: Author {
            name: signature.name().map_err(engine)?.to_owned(),
            email: signature.email().map_err(engine)?.to_owned(),
        },
    };
    save(repo, &record, true)?;
    let mut rebase = repo
        .rebase(
            Some(&annotated),
            Some(&upstream),
            Some(&onto),
            Some(&mut options()),
        )
        .map_err(|_| unknown())?;
    drive(
        repo,
        &mut rebase,
        &mut record,
        &signature,
        &mut lock,
        &temporary,
    )
    .map_err(|_| unknown())
}
fn drive(
    repo: &Repository,
    rebase: &mut git2::Rebase<'_>,
    record: &mut Record,
    signature: &git2::Signature<'_>,
    lock: &mut operations::IndexLock,
    temporary: &tempfile::TempDir,
) -> Result<Value, Error> {
    for _ in 0..32 {
        let Some(step) = rebase.next() else {
            if let Some(branch) = &record.branch {
                if branches::other_worktree(repo, branch)? {
                    return Err(recovery());
                }
            }
            rebase.finish(Some(signature)).map_err(engine)?;
            lock.publish(&temporary.path().join("index"), repo)?;
            fs::remove_file(repo.path().join(MARKER)).map_err(io_error)?;
            File::open(repo.path())
                .and_then(|f| f.sync_all())
                .map_err(io_error)?;
            return Ok(
                json!({"integrationCompleted":"rebase","commitOid":repo.head().map_err(engine)?.target().map(|o|o.to_string()),"refreshRequired":true}),
            );
        };
        step.map_err(engine)?;
        record.position = rebase.operation_current();
        record.awaiting = true;
        let index = repo.index().map_err(engine)?;
        let head_tree = repo
            .head()
            .map_err(engine)?
            .peel_to_tree()
            .map_err(engine)?;
        let diff = repo
            .diff_tree_to_index(Some(&head_tree), Some(&index), None)
            .map_err(engine)?;
        let mut current = BTreeSet::new();
        for delta in diff.deltas() {
            for f in [delta.old_file(), delta.new_file()] {
                if let Some(p) = f.path_bytes() {
                    current.insert(p.to_vec());
                }
            }
        }
        for conflict in index.conflicts().map_err(engine)? {
            let c = conflict.map_err(engine)?;
            for e in [c.ancestor, c.our, c.their].into_iter().flatten() {
                current.insert(e.path);
            }
        }
        let mut all = paths(&record.paths)?;
        all.extend(current.iter().cloned());
        record.paths = all.iter().map(|p| WirePath::new(p)).collect();
        record.current_paths = current.iter().map(|p| WirePath::new(p)).collect();
        if index.has_conflicts() {
            break;
        }
        match rebase.commit(None, signature, None) {
            Ok(_) => {}
            Err(e) if e.code() == git2::ErrorCode::Applied => {}
            Err(e) => return Err(engine(e)),
        }
        record.awaiting = false;
    }
    record.head = repo
        .head()
        .map_err(engine)?
        .target()
        .ok_or_else(recovery)?
        .to_string();
    record.native = native_hash(repo)?;
    save(repo, record, false)?;
    lock.publish(&temporary.path().join("index"), repo)?;
    Ok(
        json!({"kind":"rebase","needsResolution":record.awaiting,"needsContinue":!record.awaiting,"position":record.position.map(|p|p+1),"total":record.commits.len(),"refreshRequired":true}),
    )
}
pub fn resume(
    repo: &Repository,
    message: Option<&str>,
    author: Option<&Author>,
    skip: bool,
    expected: &str,
) -> Result<Value, Error> {
    let mut record = load(repo)?;
    let signature = operations::signature(repo, author.or(Some(&record.committer)))?;
    operations::validate_commit(repo, message.unwrap_or("Rebase"))?;
    checkout::supported_hook(repo, "post-rewrite")?;
    if skip && !record.awaiting {
        return Err(Error::new(
            "NO_REBASE_COMMIT",
            "There is no paused commit to skip.",
        ));
    }
    if !skip && repo.index().map_err(engine)?.has_conflicts() {
        return Err(Error::new(
            "UNMERGED_INDEX",
            "Resolve and stage conflicts before continuing.",
        ));
    }
    let current = paths(&record.current_paths)?;
    let mut statuses = git2::StatusOptions::new();
    statuses.update_index(false);
    for entry in repo.statuses(Some(&mut statuses)).map_err(engine)?.iter() {
        if entry.status().intersects(
            git2::Status::INDEX_NEW
                | git2::Status::INDEX_MODIFIED
                | git2::Status::INDEX_DELETED
                | git2::Status::INDEX_RENAMED
                | git2::Status::INDEX_TYPECHANGE,
        ) && !current.contains(entry.path_bytes())
        {
            return Err(Error::new(
                "UNRELATED_STAGED_CHANGES",
                "Unstage unrelated changes before continuing or skipping.",
            ));
        }
    }
    integration::guard_paths(repo, &paths(&record.paths)?, false)?;
    let mut status_options = git2::StatusOptions::new();
    status_options.update_index(false);
    for entry in repo
        .statuses(Some(&mut status_options))
        .map_err(engine)?
        .iter()
    {
        if entry.status().intersects(
            git2::Status::WT_MODIFIED
                | git2::Status::WT_DELETED
                | git2::Status::WT_RENAMED
                | git2::Status::WT_TYPECHANGE,
        ) && (!skip || !current.contains(entry.path_bytes()))
        {
            return Err(Error::new(
                "DIRTY_WORKTREE",
                "Stage the resolution and preserve unrelated working changes before continuing.",
            ));
        }
    }
    let mut lock = operations::IndexLock::acquire(repo)?;
    check(repo, expected)?;
    load(repo)?;
    let temporary = if skip {
        integration::restore_paths(repo, branches::oid(&record.head)?, &current)
            .map_err(|_| unknown())?
    } else {
        private(repo)?
    };
    let mut rebase = repo.open_rebase(Some(&mut options())).map_err(engine)?;
    if record.awaiting && !skip {
        match rebase.commit(None, &signature, message) {
            Ok(_) => {}
            Err(e) if e.code() == git2::ErrorCode::Applied => {}
            Err(_) => return Err(unknown()),
        }
    }
    record.awaiting = false;
    drive(
        repo,
        &mut rebase,
        &mut record,
        &signature,
        &mut lock,
        &temporary,
    )
    .map_err(|_| unknown())
}
pub fn abort(repo: &Repository, expected: &str) -> Result<Value, Error> {
    let record = load(repo)?;
    let mut lock = operations::IndexLock::acquire(repo)?;
    let mut refs = repo.transaction().map_err(engine)?;
    refs.lock_ref("HEAD").map_err(engine)?;
    if let Some(branch) = &record.branch {
        refs.lock_ref(branch).map_err(engine)?;
    }
    check(repo, expected)?;
    load(repo)?;
    let original = branches::oid(&record.original)?;
    let temporary = integration::restore_paths(repo, original, &paths(&record.paths)?)?;
    lock.publish(&temporary.path().join("index"), repo)
        .map_err(|_| unknown())?;
    if let Some(branch) = &record.branch {
        refs.set_symbolic_target("HEAD", branch, None, "rebase: abort")
            .map_err(|_| unknown())?;
    } else {
        refs.set_target("HEAD", original, None, "rebase: abort")
            .map_err(|_| unknown())?;
    }
    refs.commit().map_err(|_| unknown())?;
    repo.cleanup_state().map_err(|_| unknown())?;
    fs::remove_file(repo.path().join(MARKER)).map_err(|_| unknown())?;
    File::open(repo.path())
        .and_then(|f| f.sync_all())
        .map_err(|_| unknown())?;
    Ok(json!({"aborted":true,"oid":record.original,"refreshRequired":true}))
}

#[cfg(test)]
mod tests {
    use super::super::protocol::Action;
    use super::*;
    use git2::Oid;
    fn author() -> Author {
        Author {
            name: "Rebase User".into(),
            email: "rebase@example.test".into(),
        }
    }
    fn commit(repo: &Repository, parent: Option<Oid>, path: &str, content: &str) -> Oid {
        let parent = parent.map(|id| repo.find_commit(id).unwrap());
        let tree = parent.as_ref().map(|p| p.tree().unwrap());
        let mut builder = repo.treebuilder(tree.as_ref()).unwrap();
        builder
            .insert(path, repo.blob(content.as_bytes()).unwrap(), 0o100644)
            .unwrap();
        let tree = repo.find_tree(builder.write().unwrap()).unwrap();
        let sig = git2::Signature::new(
            "Source Author",
            "source@example.test",
            &git2::Time::new(12345, 60),
        )
        .unwrap();
        repo.commit(
            None,
            &sig,
            &sig,
            content,
            &tree,
            &parent.iter().collect::<Vec<_>>(),
        )
        .unwrap()
    }
    fn fixture(conflict: bool) -> (tempfile::TempDir, Repository, Oid, Oid) {
        let temp = tempfile::tempdir().unwrap();
        let repo = Repository::init(temp.path()).unwrap();
        let mut cfg = repo.config().unwrap();
        cfg.set_bool("commit.gpgsign", false).unwrap();
        cfg.set_str(
            "core.hooksPath",
            repo.path().join("hooks").to_str().unwrap(),
        )
        .unwrap();
        let base = commit(&repo, None, "file", "base\n");
        let ours = commit(&repo, Some(base), "file", "topic\n");
        let onto = commit(
            &repo,
            Some(base),
            if conflict { "file" } else { "upstream" },
            "upstream\n",
        );
        repo.reference("refs/heads/topic", ours, true, "fixture")
            .unwrap();
        repo.set_head("refs/heads/topic").unwrap();
        repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
            .unwrap();
        (temp, repo, ours, onto)
    }
    fn run(repo: &Repository, action: Action) -> Result<Value, Error> {
        operations::apply(repo, &action, &[], &repository::fingerprint(repo).unwrap())
    }
    fn begin(repo: &Repository, onto: Oid) -> Value {
        run(
            repo,
            Action::Rebase {
                upstream_oid: onto.to_string(),
                onto_oid: None,
                committer: Some(author()),
            },
        )
        .unwrap()
    }
    #[test]
    fn preparation_checks_attributes_removed_by_later_commits() {
        for (rule, code) in [
            ("file filter=custom\n", "UNSUPPORTED_FILTER"),
            ("file merge=custom\n", "UNSUPPORTED_MERGE_DRIVER"),
        ] {
            let (temp, repo, original, onto) = fixture(false);
            let with_rule = commit(&repo, Some(original), ".gitattributes", rule);
            let tip = commit(&repo, Some(with_rule), ".gitattributes", "");
            repo.reference("refs/heads/topic", tip, true, "fixture")
                .unwrap();
            repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
                .unwrap();
            let index = fs::read(repo.path().join("index")).unwrap();
            let error = run(
                &repo,
                Action::Rebase {
                    upstream_oid: onto.to_string(),
                    onto_oid: None,
                    committer: Some(author()),
                },
            )
            .unwrap_err();
            assert_eq!(error.code, code);
            assert_eq!(repo.head().unwrap().target(), Some(tip));
            assert_eq!(repo.state(), RepositoryState::Clean);
            assert_eq!(fs::read(repo.path().join("index")).unwrap(), index);
            assert_eq!(fs::read(temp.path().join("file")).unwrap(), b"topic\n");
            assert!(!repo.path().join(MARKER).exists());
        }
    }

    #[test]
    fn detached_and_batched_rebases_resume() {
        let (temp, repo, mut original, onto) = fixture(false);
        for n in 0..33 {
            original = commit(&repo, Some(original), "file", &format!("topic {n}\n"));
        }
        repo.set_head_detached(original).unwrap();
        repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
            .unwrap();
        let result = begin(&repo, onto);
        assert_eq!(result["needsContinue"], true, "{result}");
        let repo = Repository::open(temp.path()).unwrap();
        assert_eq!(status(&repo)["managed"], true);
        let result = run(
            &repo,
            Action::IntegrationContinue {
                message: None,
                author: None,
            },
        )
        .unwrap();
        assert_eq!(result["integrationCompleted"], "rebase");
        let repo = Repository::open(temp.path()).unwrap();
        assert!(repo.head_detached().unwrap());
        assert!(repo
            .graph_descendant_of(repo.head().unwrap().target().unwrap(), onto)
            .unwrap());
        assert_eq!(fs::read(temp.path().join("file")).unwrap(), b"topic 32\n");
    }
    #[test]
    fn unrelated_staging_and_unstaged_resolution_are_known_refusals() {
        let (temp, repo, _, onto) = fixture(true);
        begin(&repo, onto);
        let repo = Repository::open(temp.path()).unwrap();
        fs::write(temp.path().join("file"), "resolved\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("file")).unwrap();
        index.write().unwrap();
        fs::write(temp.path().join("file"), "unstaged\n").unwrap();
        assert_eq!(
            run(
                &repo,
                Action::IntegrationContinue {
                    message: None,
                    author: None
                }
            )
            .unwrap_err()
            .code,
            "DIRTY_WORKTREE"
        );
        fs::write(temp.path().join("keep"), "unrelated").unwrap();
        index.add_path(Path::new("keep")).unwrap();
        index.write().unwrap();
        assert_eq!(
            run(
                &repo,
                Action::IntegrationContinue {
                    message: None,
                    author: None
                }
            )
            .unwrap_err()
            .code,
            "UNRELATED_STAGED_CHANGES"
        );
        assert_eq!(status(&repo)["managed"], true);
    }
    #[test]
    fn merge_history_is_not_silently_flattened() {
        let (_temp, repo, ours, onto) = fixture(false);
        let ours = repo.find_commit(ours).unwrap();
        let other = repo.find_commit(onto).unwrap();
        let sig = operations::signature(&repo, Some(&author())).unwrap();
        repo.commit(
            Some("HEAD"),
            &sig,
            &sig,
            "merge",
            &ours.tree().unwrap(),
            &[&ours, &other],
        )
        .unwrap();
        let base = ours.parent_id(0).unwrap();
        assert_eq!(
            run(
                &repo,
                Action::Rebase {
                    upstream_oid: base.to_string(),
                    onto_oid: None,
                    committer: Some(author())
                }
            )
            .unwrap_err()
            .code,
            "UNSUPPORTED_CAPABILITY"
        );
        assert!(!active(&repo));
    }
    #[test]
    fn clean_rebase_preserves_author_and_moves_branch() {
        let (temp, repo, original, onto) = fixture(false);
        let result = begin(&repo, onto);
        assert_eq!(result["integrationCompleted"], "rebase", "{result}");
        let repo = Repository::open(temp.path()).unwrap();
        assert_eq!(repo.state(), RepositoryState::Clean);
        assert!(!active(&repo));
        let head = repo.head().unwrap();
        assert_eq!(head.name().unwrap(), "refs/heads/topic");
        let commit = head.peel_to_commit().unwrap();
        assert_ne!(commit.id(), original);
        assert_eq!(commit.parent_id(0).unwrap(), onto);
        assert_eq!(commit.author().when().seconds(), 12345);
        assert_eq!(commit.committer().name().unwrap(), "Rebase User");
        assert_eq!(fs::read(temp.path().join("file")).unwrap(), b"topic\n");
        assert_eq!(
            fs::read(temp.path().join("upstream")).unwrap(),
            b"upstream\n"
        );
    }
    #[test]
    fn conflicts_resume_after_reopen() {
        let (temp, repo, _, onto) = fixture(true);
        let result = begin(&repo, onto);
        assert_eq!(result["needsResolution"], true, "{result}");
        let repo = Repository::open(temp.path()).unwrap();
        assert_eq!(status(&repo)["managed"], true);
        assert_eq!(
            run(
                &repo,
                Action::IntegrationContinue {
                    message: None,
                    author: None
                }
            )
            .unwrap_err()
            .code,
            "UNMERGED_INDEX"
        );
        fs::write(temp.path().join("file"), "resolved\n").unwrap();
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
        let result = run(
            &repo,
            Action::IntegrationContinue {
                message: None,
                author: None,
            },
        )
        .unwrap();
        assert_eq!(result["integrationCompleted"], "rebase");
        let repo = Repository::open(temp.path()).unwrap();
        assert_eq!(
            repo.head()
                .unwrap()
                .peel_to_commit()
                .unwrap()
                .parent_id(0)
                .unwrap(),
            onto
        );
        assert_eq!(fs::read(temp.path().join("file")).unwrap(), b"resolved\n");
    }
    #[test]
    fn abort_preserves_unrelated_staged_and_working_changes() {
        let (temp, repo, original, onto) = fixture(true);
        begin(&repo, onto);
        let repo = Repository::open(temp.path()).unwrap();
        fs::write(temp.path().join("keep"), "staged").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("keep")).unwrap();
        index.write().unwrap();
        fs::write(temp.path().join("keep"), "working").unwrap();
        run(&repo, Action::IntegrationAbort {}).unwrap();
        let repo = Repository::open(temp.path()).unwrap();
        assert_eq!(repo.head().unwrap().target(), Some(original));
        assert_eq!(repo.head().unwrap().name().unwrap(), "refs/heads/topic");
        assert_eq!(repo.state(), RepositoryState::Clean);
        assert_eq!(fs::read(temp.path().join("file")).unwrap(), b"topic\n");
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
    }
    #[test]
    fn skip_drops_conflicted_commit() {
        let (temp, repo, _, onto) = fixture(true);
        begin(&repo, onto);
        let repo = Repository::open(temp.path()).unwrap();
        let result = run(&repo, Action::IntegrationSkip {}).unwrap();
        assert_eq!(result["integrationCompleted"], "rebase");
        let repo = Repository::open(temp.path()).unwrap();
        assert_eq!(repo.head().unwrap().target(), Some(onto));
        assert_eq!(fs::read(temp.path().join("file")).unwrap(), b"upstream\n");
    }
    #[test]
    fn changed_native_state_refuses_recovery() {
        let (temp, repo, _, onto) = fixture(true);
        begin(&repo, onto);
        let repo = Repository::open(temp.path()).unwrap();
        fs::write(
            repo.path().join("rebase-merge/onto"),
            format!("{}\n", Oid::ZERO_SHA1),
        )
        .unwrap();
        assert_eq!(status(&repo)["managed"], false);
        assert_eq!(
            run(&repo, Action::IntegrationAbort {}).unwrap_err().code,
            "RECOVERY_REQUIRED"
        );
        assert!(repo.index().unwrap().has_conflicts());
    }
    #[test]
    fn untracked_collision_and_stale_snapshot_do_not_start_rebase() {
        let (temp, repo, _, onto) = fixture(false);
        let snapshot = repository::fingerprint(&repo).unwrap();
        fs::write(temp.path().join("upstream"), "untracked").unwrap();
        assert_eq!(
            run(
                &repo,
                Action::Rebase {
                    upstream_oid: onto.to_string(),
                    onto_oid: None,
                    committer: Some(author())
                }
            )
            .unwrap_err()
            .code,
            "CHECKOUT_CONFLICT"
        );
        fs::remove_file(temp.path().join("upstream")).unwrap();
        fs::write(temp.path().join("extra"), "new").unwrap();
        assert_eq!(
            start(&repo, &onto.to_string(), None, Some(&author()), &snapshot)
                .unwrap_err()
                .code,
            "STALE_SNAPSHOT"
        );
        assert!(!active(&repo));
    }
}
