//! Shared preparation and recovery for merge, cherry-pick and revert.
use super::protocol::Path as WirePath;
use super::{branches, checkout, operations::IndexLock, protocol::Error, repository};
use git2::{Index, Repository};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    ffi::OsStr,
    fs::{self, File},
    io::Write,
    os::unix::{ffi::OsStrExt, fs::PermissionsExt},
    path::{Component, Path},
};
const MARKER: &str = "newport-merge.json";
#[derive(Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Kind {
    #[default]
    Merge,
    CherryPick,
    Revert,
}
impl Kind {
    pub fn current(repo: &Repository) -> Option<Self> {
        match repo.state() {
            git2::RepositoryState::Merge => Some(Self::Merge),
            git2::RepositoryState::CherryPick => Some(Self::CherryPick),
            git2::RepositoryState::Revert => Some(Self::Revert),
            _ => None,
        }
    }
}
pub(super) struct Prepared {
    pub result: Value,
    pub fingerprint: String,
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MergeRecord {
    #[serde(default)]
    pub kind: Kind,
    #[serde(default)]
    pub mainline: u32,
    version: u32,
    original_oid: String,
    pub target_oid: String,
    branch: String,
    paths: Vec<WirePath>,
}
fn paths_from_record(record: &MergeRecord) -> Result<BTreeSet<Vec<u8>>, Error> {
    if record.version != 1 || record.paths.len() > 20_000 {
        return Err(Error::new(
            "RECOVERY_REQUIRED",
            "The integration recovery record is not supported.",
        ));
    }
    record.paths.iter().map(|path| {
        let bytes = path.decode()?;
        if Path::new(OsStr::from_bytes(&bytes)).components().any(|c| !matches!(c, Component::Normal(name) if !name.as_bytes().eq_ignore_ascii_case(b".git"))) {
            return Err(Error::new("RECOVERY_REQUIRED", "The integration record contains an invalid path."));
        }
        Ok(bytes)
    }).collect()
}
fn save_record(repo: &Repository, record: &MergeRecord) -> Result<(), Error> {
    let bytes = serde_json::to_vec(record)
        .map_err(|_| Error::invalid("Cannot encode integration record."))?;
    if bytes.len() > 1024 * 1024 {
        return Err(Error::new(
            "LIMIT_EXCEEDED",
            "The integration recovery record is too large.",
        ));
    }
    let mut temporary = tempfile::NamedTempFile::new_in(repo.path()).map_err(io_error)?;
    temporary
        .as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(io_error)?;
    temporary.write_all(&bytes).map_err(io_error)?;
    temporary.as_file().sync_all().map_err(io_error)?;
    temporary
        .persist_noclobber(repo.path().join(MARKER))
        .map_err(|_| {
            Error::new(
                "RECOVERY_REQUIRED",
                "An earlier integration recovery record already exists or could not be saved.",
            )
        })?;
    File::open(repo.path())
        .and_then(|f| f.sync_all())
        .map_err(io_error)
}
pub(super) fn cleanup(repo: &Repository) -> Result<(), Error> {
    repo.cleanup_state().map_err(engine)?;
    match fs::remove_file(repo.path().join(MARKER)) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(io_error(e)),
    }
    File::open(repo.path())
        .and_then(|f| f.sync_all())
        .map_err(io_error)
}

fn engine(_: git2::Error) -> Error {
    Error::new("GIT_ERROR", "The integration could not be prepared.")
}
fn io_error(_: std::io::Error) -> Error {
    Error::new("IO_ERROR", "The integration index could not be prepared.")
}
fn unknown() -> Error {
    Error::new("OUTCOME_UNKNOWN", "The integration may have changed files, its index or integration metadata. Inspect it before retrying.")
}

pub fn merge(repo: &Repository, target: &str, expected: &str) -> Result<Value, Error> {
    let oid = branches::oid(target)?;
    let head = repo.head().map_err(engine)?;
    if !head.is_branch() {
        return Err(Error::new(
            "DETACHED_HEAD",
            "Switch to a branch before merging.",
        ));
    }
    let ours = head.peel_to_commit().map_err(engine)?;
    if ours.id() == oid || repo.graph_descendant_of(ours.id(), oid).map_err(engine)? {
        return checkout::fast_forward(repo, target, expected);
    }
    if repo.graph_descendant_of(oid, ours.id()).map_err(engine)? {
        return checkout::fast_forward(repo, target, expected);
    }
    repo.merge_base(ours.id(), oid).map_err(|_| {
        Error::new(
            "UNRELATED_HISTORIES",
            "These histories have no available common ancestor.",
        )
    })?;
    Ok(prepare(repo, target, 0, Kind::Merge, expected)?.result)
}

pub(super) fn prepare(
    repo: &Repository,
    target: &str,
    mainline: u32,
    kind: Kind,
    expected: &str,
) -> Result<Prepared, Error> {
    let oid = branches::oid(target)?;
    let head = repo.head().map_err(engine)?;
    if kind == Kind::Merge && !head.is_branch() {
        return Err(Error::new(
            "DETACHED_HEAD",
            "Switch to a branch before merging.",
        ));
    }
    let ours = head.peel_to_commit().map_err(engine)?;
    let theirs = repo.find_commit(oid).map_err(engine)?;
    if kind != Kind::Merge {
        let count = theirs.parent_count();
        if (count > 1 && (mainline == 0 || mainline as usize > count))
            || (count <= 1 && mainline != 0)
        {
            return Err(Error::invalid(
                "Select a mainline parent (starting at 1) only when replaying a merge commit.",
            ));
        }
    }
    if kind == Kind::Merge {
        checkout::supported_hook(repo, "pre-merge-commit")?;
        checkout::supported_hook(repo, "post-merge")?;
    }
    if kind != Kind::Merge && theirs.parent_count() > 0 {
        let parent = theirs
            .parent(mainline.saturating_sub(1) as usize)
            .map_err(engine)?;
        checkout::supported_merge_trees(
            repo,
            &[
                theirs.tree().map_err(engine)?,
                parent.tree().map_err(engine)?,
            ],
        )?;
    } else {
        checkout::supported_merge_files(repo, &theirs.tree().map_err(engine)?)?;
    }
    let mut lock = IndexLock::acquire(repo)?;
    let mut refs = repo.transaction().map_err(engine)?;
    let busy = |_| Error::new("REPOSITORY_BUSY", "The current branch or HEAD is locked.");
    refs.lock_ref("HEAD").map_err(busy)?;
    if head.is_branch() {
        refs.lock_ref(head.name().map_err(engine)?).map_err(busy)?;
    }
    if repository::fingerprint(repo)? != expected {
        return Err(Error::new(
            "STALE_SNAPSHOT",
            "Refresh changes before merging.",
        ));
    }
    // Abort must not later discard edits that predated the merge. Untracked files
    // remain allowed; libgit2's safe checkout protects collisions with them.
    let mut incoming = Index::new().map_err(engine)?;
    incoming
        .read_tree(&theirs.tree().map_err(engine)?)
        .map_err(engine)?;
    let mut tracked: BTreeSet<Vec<u8>> = incoming
        .iter()
        .chain(repo.index().map_err(engine)?.iter())
        .map(|entry| entry.path)
        .collect();
    let preview = match kind {
        Kind::Merge => repo.merge_commits(&ours, &theirs, None),
        Kind::CherryPick => repo.cherrypick_commit(&theirs, &ours, mainline, None),
        Kind::Revert => repo.revert_commit(&theirs, &ours, mainline, None),
    }
    .map_err(engine)?;
    // Preview only creates an in-memory index. Check the union once, before
    // writing recovery state or checking out files. Include paths restored by
    // revert that are absent from both the current and selected commit trees.
    tracked.extend(preview.iter().map(|entry| entry.path));
    guard_paths(repo, &tracked, true)?;
    let mut changed = BTreeSet::new();
    let original_tree = ours.tree().map_err(engine)?;
    let diff = repo
        .diff_tree_to_index(Some(&original_tree), Some(&preview), None)
        .map_err(engine)?;
    for delta in diff.deltas() {
        for path in [delta.old_file().path_bytes(), delta.new_file().path_bytes()]
            .into_iter()
            .flatten()
        {
            changed.insert(path.to_vec());
        }
    }
    for conflict in preview.conflicts().map_err(engine)? {
        let conflict = conflict.map_err(engine)?;
        for entry in [conflict.ancestor, conflict.our, conflict.their]
            .into_iter()
            .flatten()
        {
            changed.insert(entry.path);
        }
    }
    let record = MergeRecord {
        kind,
        mainline,
        version: 1,
        original_oid: ours.id().to_string(),
        target_oid: oid.to_string(),
        branch: head.name().map_err(engine)?.into(),
        paths: changed.iter().map(|p| WirePath::new(p)).collect(),
    };
    paths_from_record(&record)?;
    let temporary = tempfile::Builder::new()
        .prefix("newport-merge-")
        .tempdir_in(repo.path())
        .map_err(io_error)?;
    let index_path = temporary.path().join("index");
    fs::copy(repo.path().join("index"), &index_path).map_err(io_error)?;
    let _index = super::operations::private_index(repo, &index_path)?;
    let mut checkout = git2::build::CheckoutBuilder::new();
    checkout
        .safe()
        .allow_conflicts(true)
        .conflict_style_merge(true)
        .overwrite_ignored(false)
        .remove_ignored(false)
        .remove_untracked(false);
    // Native integration writes state files before checking out. Any failure
    // after saving the recovery record may have changed metadata or files.
    save_record(repo, &record)?;
    let result = match kind {
        Kind::Merge => repo
            .find_annotated_commit(oid)
            .and_then(|annotated| repo.merge(&[&annotated], None, Some(&mut checkout))),
        Kind::CherryPick => {
            let mut options = git2::CherrypickOptions::new();
            options.mainline(mainline).checkout_builder(checkout);
            repo.cherrypick(&theirs, Some(&mut options))
        }
        Kind::Revert => {
            let mut options = git2::RevertOptions::new();
            options.mainline(mainline).checkout_builder(checkout);
            repo.revert(&theirs, Some(&mut options))
        }
    };
    result.map_err(|_| unknown())?;
    // IndexLock publishes a private 0600 file; include that final mode in the
    // prepared fingerprint as well as its bytes and status entries.
    fs::set_permissions(&index_path, fs::Permissions::from_mode(0o600)).map_err(|_| unknown())?;
    let fingerprint = repository::fingerprint_index(repo, &index_path).map_err(|_| unknown())?;
    lock.publish(&index_path, repo).map_err(|_| unknown())?;
    let index = repo.index().map_err(|_| unknown())?;
    let conflicts = index.conflicts().map_err(|_| unknown())?.count();
    Ok(Prepared {
        result: json!({"kind":kind,"targetOid":oid.to_string(),"originalOid":ours.id().to_string(),"needsCommit":true,"needsResolution":conflicts>0,"conflictCount":conflicts,"refreshRequired":true}),
        fingerprint,
    })
}

pub(super) fn guard_paths(
    repo: &Repository,
    tracked: &BTreeSet<Vec<u8>>,
    require_clean: bool,
) -> Result<(), Error> {
    let mut options = git2::StatusOptions::new();
    options
        .include_untracked(true)
        .include_ignored(true)
        .update_index(false);
    for entry in repo.statuses(Some(&mut options)).map_err(engine)?.iter() {
        let flags = entry.status();
        let path = entry
            .path_bytes()
            .strip_suffix(b"/")
            .unwrap_or(entry.path_bytes());
        let untracked = flags.intersects(git2::Status::WT_NEW | git2::Status::IGNORED);
        if !untracked {
            if require_clean {
                return Err(Error::new(
                    "DIRTY_WORKTREE",
                    "Commit or stash tracked changes before this operation.",
                ));
            }
            if tracked.contains(path) {
                continue;
            }
        }
        let mut prefix = path.to_vec();
        prefix.push(b'/');
        let nested = tracked
            .range(prefix.clone()..)
            .next()
            .is_some_and(|candidate| candidate.starts_with(&prefix));
        let parent_collision = path
            .iter()
            .enumerate()
            .any(|(i, b)| *b == b'/' && tracked.contains(&path[..i]));
        if tracked.contains(path) || nested || parent_collision {
            return Err(Error::new(
                "CHECKOUT_CONFLICT",
                "An untracked or ignored path overlaps this operation. Move it before continuing.",
            ));
        }
    }
    Ok(())
}

pub(super) fn current_record(repo: &Repository) -> Result<MergeRecord, Error> {
    let metadata = fs::symlink_metadata(repo.path().join(MARKER)).map_err(|_| Error::new("RECOVERY_REQUIRED", "This integration has no Newport recovery record; automatic abort cannot prove which changes belong to it."))?;
    if !metadata.is_file() || metadata.len() > 1024 * 1024 {
        return Err(Error::new(
            "RECOVERY_REQUIRED",
            "Invalid integration recovery record.",
        ));
    }
    let record: MergeRecord =
        serde_json::from_slice(&fs::read(repo.path().join(MARKER)).map_err(io_error)?)
            .map_err(|_| Error::new("RECOVERY_REQUIRED", "Invalid integration recovery record."))?;
    paths_from_record(&record)?;
    if Kind::current(repo) != Some(record.kind) {
        return Err(Error::new(
            "RECOVERY_REQUIRED",
            "The integration state no longer matches its recovery record.",
        ));
    }
    let original = branches::oid(&record.original_oid)?;
    let target = branches::oid(&record.target_oid)?;
    let head = repo.head().map_err(engine)?;
    let native_matches = match record.kind {
        Kind::Merge => {
            let mut heads = Vec::new();
            Repository::open(repo.path())
                .map_err(engine)?
                .mergehead_foreach(|oid| {
                    heads.push(*oid);
                    true
                })
                .map_err(engine)?;
            heads == vec![target]
                && repo.find_reference("ORIG_HEAD").map_err(engine)?.target() == Some(original)
        }
        Kind::CherryPick | Kind::Revert => {
            let name = if record.kind == Kind::CherryPick {
                "CHERRY_PICK_HEAD"
            } else {
                "REVERT_HEAD"
            };
            repo.find_reference(name).map_err(engine)?.target() == Some(target)
        }
    };
    if head.name().map_err(engine)? != record.branch
        || head.target() != Some(original)
        || !native_matches
    {
        return Err(Error::new(
            "RECOVERY_REQUIRED",
            "HEAD or integration metadata changed. Automatic recovery refused.",
        ));
    }
    Ok(record)
}

pub(super) fn status(repo: &Repository) -> Value {
    if super::rebase::active(repo) {
        return super::rebase::status(repo);
    }
    let Some(kind) = Kind::current(repo) else {
        return Value::Null;
    };
    match current_record(repo) {
        Ok(record) => {
            json!({"kind":kind,"managed":true,"targetOid":record.target_oid,"originalOid":record.original_oid,"mainline":record.mainline,"canContinue":true,"canAbort":true})
        }
        Err(_) => json!({"kind":kind,"managed":false,"canContinue":false,"canAbort":false}),
    }
}

pub fn abort(repo: &Repository, expected: &str) -> Result<Value, Error> {
    if Kind::current(repo).is_none() {
        return Err(Error::new(
            "NO_INTEGRATION",
            "There is no supported integration to abort.",
        ));
    }
    let record = current_record(repo)?;
    let paths = paths_from_record(&record)?;
    let original = branches::oid(&record.original_oid)?;
    let mut lock = IndexLock::acquire(repo)?;
    let mut refs = repo.transaction().map_err(engine)?;
    refs.lock_ref("HEAD").map_err(engine)?;
    if record.branch.contains('\0')
        || (record.branch != "HEAD"
            && (!record.branch.starts_with("refs/heads/")
                || !git2::Reference::is_valid_name(&record.branch)))
        || (record.branch == "HEAD" && record.kind == Kind::Merge)
    {
        return Err(Error::new(
            "RECOVERY_REQUIRED",
            "The integration branch is invalid.",
        ));
    }
    if record.branch != "HEAD" {
        refs.lock_ref(&record.branch).map_err(engine)?;
    }
    if repository::fingerprint(repo)? != expected {
        return Err(Error::new(
            "STALE_SNAPSHOT",
            "Refresh the integration before aborting.",
        ));
    }
    current_record(repo)?;
    guard_paths(repo, &paths, false)?;
    let temporary = restore_paths(repo, original, &paths)?;
    let index_path = temporary.path().join("index");
    lock.publish(&index_path, repo).map_err(|_| unknown())?;
    cleanup(repo).map_err(|_| unknown())?;
    Ok(
        json!({"aborted":true,"oid":original.to_string(),"reference":record.branch,"refreshRequired":true}),
    )
}

/// Caller holds the native index lock and any references it will update.
/// Prepare a replacement index and restore only the owned integration paths.
pub(super) fn restore_paths(
    repo: &Repository,
    original: git2::Oid,
    paths: &BTreeSet<Vec<u8>>,
) -> Result<tempfile::TempDir, Error> {
    guard_paths(repo, paths, false)?;
    let original_tree = repo
        .find_commit(original)
        .map_err(engine)?
        .tree()
        .map_err(engine)?;
    checkout::supported_files(repo, &original_tree)?;
    let mut original_index = Index::new().map_err(engine)?;
    original_index.read_tree(&original_tree).map_err(engine)?;
    let temporary = tempfile::Builder::new()
        .prefix("newport-abort-")
        .tempdir_in(repo.path())
        .map_err(io_error)?;
    let index_path = temporary.path().join("index");
    fs::copy(repo.path().join("index"), &index_path).map_err(io_error)?;
    let mut index = super::operations::private_index(repo, &index_path)?;
    for bytes in paths {
        let path = Path::new(OsStr::from_bytes(bytes));
        if let Err(e) = index.conflict_remove(path) {
            if e.code() != git2::ErrorCode::NotFound {
                return Err(engine(e));
            }
        }
        if let Some(entry) = original_index.get_path(path, 0) {
            // Keep the tree entry's empty stat cache: preserving cached metadata
            // can hide equal-sized edits with restored mtimes when trustctime
            // is disabled, even during a forced hard reset.
            // add replaces an existing stage-zero entry. Avoid removing and
            // reinserting it in the sorted index for every restored path.
            index.add(&entry).map_err(engine)?;
        } else if let Err(e) = index.remove_path(path) {
            if e.code() != git2::ErrorCode::NotFound {
                return Err(engine(e));
            }
        }
    }
    index.write().map_err(engine)?;
    let mut restore = git2::build::CheckoutBuilder::new();
    restore
        .force()
        .disable_pathspec_match(true)
        .remove_untracked(false)
        .remove_ignored(false)
        .overwrite_ignored(false)
        .update_index(false);
    let mut restores = 0;
    for bytes in paths {
        let path = Path::new(OsStr::from_bytes(bytes));
        if original_index.get_path(path, 0).is_some() {
            restore.path(path);
            restores += 1;
        }
    }
    // An empty path selection means ALL paths to libgit2, so never call it then.
    if restores > 0 {
        repo.checkout_tree(original_tree.as_object(), Some(&mut restore))
            .map_err(|_| unknown())?;
    }
    for bytes in paths {
        let path = Path::new(OsStr::from_bytes(bytes));
        if original_index.get_path(path, 0).is_none() {
            match fs::remove_file(repo.workdir().ok_or_else(unknown)?.join(path)) {
                Ok(()) => {}
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                    ) => {}
                Err(_) => return Err(unknown()),
            }
        }
    }
    Ok(temporary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{
        operations,
        protocol::{Action, Author},
    };
    use std::path::Path;
    fn commit(repo: &Repository, path: &str, text: &str) -> git2::Oid {
        fs::write(repo.workdir().unwrap().join(path), text).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new(path)).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
        let sig = git2::Signature::now("Fixture", "fixture@example.test").unwrap();
        repo.commit(
            Some("HEAD"),
            &sig,
            &sig,
            text,
            &tree,
            &parent.iter().collect::<Vec<_>>(),
        )
        .unwrap()
    }
    fn fixture(conflict: bool) -> (tempfile::TempDir, Repository, git2::Oid, git2::Oid) {
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
        let base = commit(&repo, "file", "base\n");
        let ours = commit(&repo, "file", "ours\n");
        let signature = git2::Signature::now("Fixture", "fixture@example.test").unwrap();
        let ancestor = repo.find_commit(base).unwrap();
        let base_tree = ancestor.tree().unwrap();
        let mut builder = repo.treebuilder(Some(&base_tree)).unwrap();
        let blob = repo.blob(b"theirs\n").unwrap();
        builder
            .insert(if conflict { "file" } else { "other" }, blob, 0o100644)
            .unwrap();
        let tree = repo.find_tree(builder.write().unwrap()).unwrap();
        let theirs = repo
            .commit(None, &signature, &signature, "theirs", &tree, &[&ancestor])
            .unwrap();
        drop(tree);
        drop(builder);
        drop(base_tree);
        drop(ancestor);
        (temp, repo, ours, theirs)
    }
    fn finish(repo: &Repository) -> Value {
        operations::apply(
            repo,
            &Action::Commit {
                message: "Merge topic".into(),
                author: Some(Author {
                    name: "Fixture".into(),
                    email: "fixture@example.test".into(),
                }),
            },
            &[],
            &repository::fingerprint(repo).unwrap(),
        )
        .unwrap()
    }
    #[test]
    fn journaled_conflict_status_blobs_and_resolution_roundtrip() {
        use super::super::{
            journal::Journal,
            protocol::{Path as WirePath, Request},
            repository::{Output, Service},
        };
        use std::os::unix::ffi::OsStrExt;
        let (temp, _repo, _, theirs) = fixture(true);
        let records = tempfile::tempdir().unwrap();
        let mut service = Service::with_journal(
            Journal::open(
                records.path().join("journal"),
                uuid::Uuid::new_v4().to_string(),
            )
            .unwrap(),
        );
        let read = |service: &mut Service, request| match service.request(request).unwrap() {
            Output::Json(value) => value,
            _ => panic!("expected JSON"),
        };
        let opened = read(
            &mut service,
            Request::Open {
                path: WirePath::new(temp.path().as_os_str().as_bytes()),
            },
        );
        let repo_id = opened["repoId"].as_str().unwrap().to_owned();
        let status_request = Request::Status {
            filter: None,
            repo_id: repo_id.clone(),
            page_size: 100,
            cursor: None,
        };
        let status = read(&mut service, status_request.clone());
        let merged = read(
            &mut service,
            Request::Start {
                operation_id: uuid::Uuid::new_v4().to_string(),
                repo_id: repo_id.clone(),
                expected_snapshot: status["snapshot"].as_str().unwrap().into(),
                action: Action::Merge {
                    target_oid: theirs.to_string(),
                },
            },
        );
        assert_eq!(merged["state"], "needs_resolution", "{merged}");
        let status = read(&mut service, status_request.clone());
        let entry = &status["entries"][0];
        assert_eq!(entry["conflicted"], true);
        let object = entry["conflict"]["theirs"]["oid"]["hex"]
            .as_str()
            .unwrap()
            .to_owned();
        let blob = read(
            &mut service,
            Request::Blob {
                repo_id: repo_id.clone(),
                oid: object,
            },
        );
        use base64::Engine;
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(blob["bytesB64"].as_str().unwrap())
                .unwrap(),
            b"theirs\n"
        );
        fs::write(temp.path().join("file"), "resolution\n").unwrap();
        let status = read(&mut service, status_request.clone());
        let staged = read(
            &mut service,
            Request::Start {
                operation_id: uuid::Uuid::new_v4().to_string(),
                repo_id: repo_id.clone(),
                expected_snapshot: status["snapshot"].as_str().unwrap().into(),
                action: Action::Stage {
                    hunks: None,
                    entry_ids: vec![status["entries"][0]["entryId"].as_str().unwrap().into()],
                },
            },
        );
        assert_eq!(staged["state"], "succeeded", "{staged}");
        let status = read(&mut service, status_request);
        let committed = read(
            &mut service,
            Request::Start {
                operation_id: uuid::Uuid::new_v4().to_string(),
                repo_id,
                expected_snapshot: status["snapshot"].as_str().unwrap().into(),
                action: Action::Commit {
                    message: "Resolve merge".into(),
                    author: Some(Author {
                        name: "Fixture".into(),
                        email: "fixture@example.test".into(),
                    }),
                },
            },
        );
        assert_eq!(committed["state"], "succeeded", "{committed}");
        assert_eq!(committed["result"]["mergeCompleted"], true);
        assert_eq!(
            Repository::open(temp.path()).unwrap().state(),
            git2::RepositoryState::Clean
        );
    }
    #[test]
    fn refuses_preexisting_edits_untracked_collisions_and_custom_drivers() {
        let (temp, repo, ours, theirs) = fixture(false);
        fs::write(temp.path().join("file"), "local edit").unwrap();
        assert_eq!(
            merge(
                &repo,
                &theirs.to_string(),
                &repository::fingerprint(&repo).unwrap()
            )
            .unwrap_err()
            .code,
            "DIRTY_WORKTREE"
        );
        fs::write(temp.path().join("file"), "ours\n").unwrap();
        fs::write(temp.path().join("other"), "untracked data").unwrap();
        assert_eq!(
            merge(
                &repo,
                &theirs.to_string(),
                &repository::fingerprint(&repo).unwrap()
            )
            .unwrap_err()
            .code,
            "CHECKOUT_CONFLICT"
        );
        assert_eq!(
            fs::read(temp.path().join("other")).unwrap(),
            b"untracked data"
        );
        fs::write(temp.path().join(".gitattributes"), "file merge=custom\n").unwrap();
        assert_eq!(
            merge(
                &repo,
                &theirs.to_string(),
                &repository::fingerprint(&repo).unwrap()
            )
            .unwrap_err()
            .code,
            "UNSUPPORTED_MERGE_DRIVER"
        );
        assert_eq!(repo.state(), git2::RepositoryState::Clean);
        assert_eq!(repo.head().unwrap().target(), Some(ours));
        assert!(!repo.path().join("index.lock").exists());
    }
    #[test]
    fn abort_discards_resolutions_but_preserves_unrelated_staged_and_working_files() {
        let (temp, repo, ours, theirs) = fixture(true);
        merge(
            &repo,
            &theirs.to_string(),
            &repository::fingerprint(&repo).unwrap(),
        )
        .unwrap();
        let repo = Repository::open(temp.path()).unwrap();
        fs::write(temp.path().join("file"), "resolution to discard").unwrap();
        fs::write(temp.path().join("keep"), "staged content").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("keep")).unwrap();
        index.write().unwrap();
        fs::write(temp.path().join("keep"), "working edit").unwrap();
        fs::write(temp.path().join("notes"), "untracked notes").unwrap();
        let result = abort(&repo, &repository::fingerprint(&repo).unwrap()).unwrap();
        assert_eq!(result["aborted"], true);
        let repo = Repository::open(temp.path()).unwrap();
        assert_eq!(repo.state(), git2::RepositoryState::Clean);
        assert_eq!(repo.head().unwrap().target(), Some(ours));
        assert!(!repo.index().unwrap().has_conflicts());
        assert!(!repo.path().join(MARKER).exists());
        assert_eq!(fs::read(temp.path().join("file")).unwrap(), b"ours\n");
        assert_eq!(fs::read(temp.path().join("keep")).unwrap(), b"working edit");
        let staged = repo
            .index()
            .unwrap()
            .get_path(Path::new("keep"), 0)
            .unwrap()
            .id;
        assert_eq!(repo.find_blob(staged).unwrap().content(), b"staged content");
        assert_eq!(
            fs::read(temp.path().join("notes")).unwrap(),
            b"untracked notes"
        );
    }
    #[test]
    fn abort_clean_merge_removes_added_files_and_preserves_edits_elsewhere() {
        let (temp, repo, _, theirs) = fixture(false);
        merge(
            &repo,
            &theirs.to_string(),
            &repository::fingerprint(&repo).unwrap(),
        )
        .unwrap();
        let repo = Repository::open(temp.path()).unwrap();
        fs::write(temp.path().join("file"), "unrelated edit").unwrap();
        abort(&repo, &repository::fingerprint(&repo).unwrap()).unwrap();
        assert!(!temp.path().join("other").exists());
        assert_eq!(
            fs::read(temp.path().join("file")).unwrap(),
            b"unrelated edit"
        );
        let repo = Repository::open(temp.path()).unwrap();
        assert!(repo
            .index()
            .unwrap()
            .get_path(Path::new("other"), 0)
            .is_none());
        assert_eq!(repo.state(), git2::RepositoryState::Clean);
    }
    #[test]
    fn abort_rejects_unrelated_nested_files_and_missing_or_changed_recovery_state() {
        let (temp, repo, _, theirs) = fixture(false);
        merge(
            &repo,
            &theirs.to_string(),
            &repository::fingerprint(&repo).unwrap(),
        )
        .unwrap();
        let repo = Repository::open(temp.path()).unwrap();
        let old_snapshot = repository::fingerprint(&repo).unwrap();
        fs::remove_file(temp.path().join("other")).unwrap();
        fs::create_dir(temp.path().join("other")).unwrap();
        fs::write(temp.path().join("other/notes"), "keep me").unwrap();
        let mut index = repo.index().unwrap();
        index.remove_path(Path::new("other")).unwrap();
        index.add_path(Path::new("other/notes")).unwrap();
        index.write().unwrap();
        assert_eq!(
            abort(&repo, &old_snapshot).unwrap_err().code,
            "STALE_SNAPSHOT"
        );
        assert_eq!(
            abort(&repo, &repository::fingerprint(&repo).unwrap())
                .unwrap_err()
                .code,
            "CHECKOUT_CONFLICT"
        );
        assert_eq!(
            fs::read(temp.path().join("other/notes")).unwrap(),
            b"keep me"
        );
        fs::remove_file(repo.path().join(MARKER)).unwrap();
        assert_eq!(
            abort(&repo, &repository::fingerprint(&repo).unwrap())
                .unwrap_err()
                .code,
            "RECOVERY_REQUIRED"
        );
        assert_eq!(repo.state(), git2::RepositoryState::Merge);
    }
    #[test]
    fn clean_three_way_merge_commits_both_parents() {
        let (temp, repo, ours, theirs) = fixture(false);
        let result = merge(
            &repo,
            &theirs.to_string(),
            &repository::fingerprint(&repo).unwrap(),
        )
        .unwrap();
        assert_eq!(result["needsResolution"], false);
        assert_eq!(result["needsCommit"], true);
        let repo = Repository::open(temp.path()).unwrap();
        assert_eq!(repo.state(), git2::RepositoryState::Merge);
        assert_eq!(repo.head().unwrap().target(), Some(ours));
        assert_eq!(fs::read(temp.path().join("file")).unwrap(), b"ours\n");
        assert_eq!(fs::read(temp.path().join("other")).unwrap(), b"theirs\n");
        let result = finish(&repo);
        assert_eq!(result["mergeCompleted"], true);
        let merged = repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(merged.parent_count(), 2);
        assert_eq!(merged.parent_id(0).unwrap(), ours);
        assert_eq!(merged.parent_id(1).unwrap(), theirs);
        assert_eq!(repo.state(), git2::RepositoryState::Clean);
    }
    #[test]
    fn conflict_can_be_resolved_staged_and_committed_after_reopening() {
        let (temp, repo, ours, theirs) = fixture(true);
        let result = merge(
            &repo,
            &theirs.to_string(),
            &repository::fingerprint(&repo).unwrap(),
        )
        .unwrap();
        assert_eq!(result["needsResolution"], true);
        let repo = Repository::open(temp.path()).unwrap();
        assert!(repo.index().unwrap().has_conflicts());
        assert!(fs::read_to_string(temp.path().join("file"))
            .unwrap()
            .contains("<<<<<<<"));
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
        assert!(!repo.index().unwrap().has_conflicts());
        finish(&repo);
        let merged = repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(merged.parent_id(0).unwrap(), ours);
        assert_eq!(merged.parent_id(1).unwrap(), theirs);
        assert_eq!(repo.state(), git2::RepositoryState::Clean);
        assert_eq!(fs::read(temp.path().join("file")).unwrap(), b"resolved\n");
    }
}
