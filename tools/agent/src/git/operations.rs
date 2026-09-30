//! Whole-file and partial index edits, and commits. Revalidate snapshots under native Git
//! locks before publishing an index or reference change.
use super::{
    protocol::{Action, Author, Error},
    repository,
};
use git2::{Index, Repository};
use std::{
    ffi::OsStr,
    fs::{self, File, OpenOptions},
    io,
    os::unix::{ffi::OsStrExt, fs::OpenOptionsExt},
    path::{Path, PathBuf},
};
fn engine(_: git2::Error) -> Error {
    Error::new(
        "GIT_ERROR",
        "Git could not prepare this operation. The working files were not modified.",
    )
}
fn io_error(_: io::Error) -> Error {
    Error::new("IO_ERROR", "The index change could not be saved.")
}
// Index::open does not inherit core.ignorecase/filemode/symlinks, and git2 does
// not expose libgit2's index capability setter. Open the prepared index through
// a temporary repository to initialize those capabilities from the real config.
// Drop its owner before attaching the index to the repository being edited.
pub(super) fn private_index(repo: &Repository, path: &Path) -> Result<Index, Error> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::invalid("Invalid index path."))?;
    let mut options = git2::RepositoryInitOptions::new();
    options.bare(true).external_template(false);
    let prepared = Repository::init_opts(parent, &options).map_err(engine)?;
    prepared
        .set_config(&repo.config().map_err(engine)?)
        .map_err(engine)?;
    let mut index = prepared.index().map_err(engine)?;
    drop(prepared);
    repo.set_index(&mut index).map_err(engine)?;
    Ok(index)
}

pub(super) struct IndexLock {
    path: PathBuf,
    file: File,
    published: bool,
}
impl IndexLock {
    pub(super) fn acquire(repo: &Repository) -> Result<Self, Error> {
        let path = repo.path().join("index.lock");
        let file = OpenOptions::new()
            .write(true)
            .read(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|e| {
                if e.kind() == io::ErrorKind::AlreadyExists {
                    Error::new(
                        "REPOSITORY_BUSY",
                        "Git's index is locked. No existing lock was removed.",
                    )
                } else {
                    io_error(e)
                }
            })?;
        Ok(Self {
            path,
            file,
            published: false,
        })
    }
    pub(super) fn publish(&mut self, prepared_path: &Path, repo: &Repository) -> Result<(), Error> {
        let mut prepared = File::open(prepared_path).map_err(io_error)?;
        io::copy(&mut prepared, &mut self.file).map_err(io_error)?;
        self.file.sync_all().map_err(io_error)?;
        fs::rename(&self.path, repo.path().join("index")).map_err(io_error)?;
        self.published = true;
        File::open(repo.path())
            .and_then(|f| f.sync_all())
            .map_err(|_| {
                Error::new(
                    "OUTCOME_UNKNOWN",
                    "The index was replaced but its durability could not be confirmed.",
                )
            })
    }
}
impl Drop for IndexLock {
    fn drop(&mut self) {
        if !self.published {
            let _ = fs::remove_file(&self.path);
        }
    }
}
fn remove(index: &mut Index, path: &Path) -> Result<(), Error> {
    match index.conflict_remove(path) {
        Ok(()) => {}
        Err(e) if e.code() == git2::ErrorCode::NotFound => {}
        Err(e) => return Err(engine(e)),
    }
    match index.remove_path(path) {
        Ok(()) => Ok(()),
        Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(()),
        Err(e) => Err(engine(e)),
    }
}

/// Files one index edit may name after directory selections are expanded.
const MAX_STAGED_PATHS: usize = 10_000;

/// Expand a directory selection into the working files `git add <dir>` stages:
/// everything below it that is not ignored and not inside a nested repository.
/// Walked in sorted order so the same tree always produces the same index edit.
fn directory_files(
    repo: &Repository,
    root: &Path,
    directory: &Path,
) -> Result<Vec<Vec<u8>>, Error> {
    let mut files = Vec::new();
    let mut pending = vec![directory.to_path_buf()];
    while let Some(relative) = pending.pop() {
        let mut children = Vec::new();
        for entry in fs::read_dir(root.join(&relative)).map_err(io_error)? {
            children.push(relative.join(entry.map_err(io_error)?.file_name()));
        }
        children.sort();
        for child in children {
            if child.file_name() == Some(OsStr::new(".git"))
                || repo.status_should_ignore(&child).map_err(engine)?
            {
                continue;
            }
            if fs::symlink_metadata(root.join(&child))
                .map_err(io_error)?
                .is_dir()
            {
                if Repository::open(root.join(&child)).is_ok() {
                    return Err(Error::new(
                        "UNSUPPORTED_CAPABILITY",
                        "Staging submodule directories is not supported yet.",
                    ));
                }
                pending.push(child);
            } else {
                files.push(child.as_os_str().as_bytes().to_vec());
                if files.len() > MAX_STAGED_PATHS {
                    return Err(Error::new(
                        "LIMIT_EXCEEDED",
                        "That directory holds too many files to stage at once. Stage a smaller part of it.",
                    ));
                }
            }
        }
    }
    files.sort();
    Ok(files)
}

pub fn apply(
    repo: &Repository,
    action: &Action,
    paths: &[Vec<u8>],
    expected: &str,
) -> Result<serde_json::Value, Error> {
    if matches!(
        action,
        Action::WorktreeRepair { .. }
            | Action::WorktreeRemove { .. }
            | Action::WorktreePrune { .. }
            | Action::WorktreeAdd { .. }
            | Action::WorktreeLock { .. }
            | Action::WorktreeUnlock { .. }
    ) {
        return super::worktrees::apply(repo, action, expected);
    }
    if super::rebase::active(repo) {
        match action {
            Action::IntegrationContinue { message, author } => {
                return super::rebase::resume(
                    repo,
                    message.as_deref(),
                    author.as_ref(),
                    false,
                    expected,
                )
            }
            Action::IntegrationSkip {} => {
                return super::rebase::resume(repo, None, None, true, expected)
            }
            Action::IntegrationAbort {} => return super::rebase::abort(repo, expected),
            Action::Stage { .. } => super::rebase::validate(repo)?,
            _ => {
                return Err(Error::new(
                    "INTEGRATION_IN_PROGRESS",
                    "Continue, skip or abort the rebase before this operation.",
                ))
            }
        }
    }
    if let Action::Rebase {
        upstream_oid,
        onto_oid,
        committer,
    } = action
    {
        return super::rebase::start(
            repo,
            upstream_oid,
            onto_oid.as_deref(),
            committer.as_ref(),
            expected,
        );
    }
    if matches!(action, Action::IntegrationSkip {}) {
        return Err(Error::new(
            "NO_REBASE",
            "There is no rebase commit to skip.",
        ));
    }
    if matches!(action, Action::TagCreate { .. } | Action::TagDelete { .. }) {
        return super::tags::apply(repo, action, expected);
    }
    if matches!(
        action,
        Action::RemoteAdd { .. }
            | Action::RemoteRename { .. }
            | Action::RemoteSetUrl { .. }
            | Action::RemoteRemove { .. }
            | Action::Fetch { .. }
            | Action::BranchDeleteRemote { .. }
            | Action::TagDeleteRemote { .. }
            | Action::PushWithLease { .. }
            | Action::Push { .. }
            | Action::TagPush { .. }
    ) {
        return super::remotes::apply(repo, action, expected);
    }
    if repo.is_bare()
        || (repo.state() != git2::RepositoryState::Clean
            && !((super::integration::Kind::current(repo).is_some()
                || (super::rebase::active(repo) && matches!(action, Action::Stage { .. })))
                && matches!(
                    action,
                    Action::Stage { .. }
                        | Action::Commit { .. }
                        | Action::ConflictResolve { .. }
                        | Action::MergeAbort { .. }
                        | Action::IntegrationContinue { .. }
                        | Action::IntegrationAbort {}
                )))
    {
        return Err(Error::new(
            "UNSUPPORTED_CAPABILITY",
            "Finish the repository's current integration before editing its index.",
        ));
    }
    if let Action::ConflictResolve {
        side, expected_oid, ..
    } = action
    {
        return super::conflicts::apply(repo, paths, *side, expected_oid.as_deref(), expected);
    }
    if let Action::Discard { source, hunks, .. } = action {
        return super::discard::apply(repo, paths, *source, hunks.as_ref(), expected);
    }
    if let Action::Reset {
        target_oid,
        expected_oid,
        mode,
    } = action
    {
        return super::reset::apply(repo, target_oid, expected_oid, *mode, expected);
    }
    if matches!(
        action,
        Action::StashSave { .. }
            | Action::StashApply { .. }
            | Action::StashPop { .. }
            | Action::StashDrop { .. }
    ) {
        return super::stash::apply(repo, action, expected);
    }
    if matches!(
        action,
        Action::BranchCreate { .. }
            | Action::BranchRename { .. }
            | Action::BranchDelete { .. }
            | Action::BranchSetUpstream { .. }
    ) {
        return super::branches::apply(repo, action, expected);
    }
    if matches!(action, Action::MergeAbort { .. }) && repo.state() != git2::RepositoryState::Merge {
        return Err(Error::new("NO_MERGE", "There is no merge to abort."));
    }
    if matches!(
        action,
        Action::MergeAbort { .. } | Action::IntegrationAbort {}
    ) {
        return super::integration::abort(repo, expected);
    }
    if let Action::IntegrationContinue { message, author } = action {
        return super::replay::resume(repo, message.as_deref(), author.as_ref(), expected);
    }
    if let Action::CherryPick {
        target_oid,
        mainline,
        author,
    }
    | Action::Revert {
        target_oid,
        mainline,
        author,
    } = action
    {
        let kind = if matches!(action, Action::CherryPick { .. }) {
            super::integration::Kind::CherryPick
        } else {
            super::integration::Kind::Revert
        };
        return super::replay::start(repo, target_oid, *mainline, kind, author.as_ref(), expected);
    }
    if let Action::Merge { target_oid } = action {
        return super::integration::merge(repo, target_oid, expected);
    }
    if let Action::PullFastForward {
        remote,
        expected_token,
        remote_branch,
    } = action
    {
        return super::remotes::pull_fast_forward(
            repo,
            remote,
            expected_token,
            remote_branch,
            expected,
        );
    }
    if let Action::FastForward { target_oid } = action {
        return super::checkout::fast_forward(repo, target_oid, expected);
    }
    if let Action::Checkout { target } = action {
        return super::checkout::apply(repo, target, expected);
    }
    if let Action::Amend {
        expected_oid,
        message,
        committer,
        author,
    } = action
    {
        return commit_inner(
            repo,
            message,
            committer.as_ref(),
            expected,
            CommitMode::Amend {
                expected_oid,
                author: author.as_ref(),
            },
        );
    }
    if let Action::Commit { message, author } = action {
        return commit(repo, message, author.as_ref(), expected);
    }
    let root = repo
        .workdir()
        .ok_or_else(|| Error::new("UNSUPPORTED_CAPABILITY", "No working tree."))?;
    let original = repo.path().join("index");
    if fs::symlink_metadata(&original).is_ok_and(|m| !m.is_file()) {
        return Err(Error::new(
            "UNSUPPORTED_CAPABILITY",
            "A non-regular Git index is not supported.",
        ));
    }
    let mut lock = IndexLock::acquire(repo)?;
    if repository::fingerprint(repo)? != expected {
        return Err(Error::new(
            "STALE_SNAPSHOT",
            "Refresh changes before editing the index.",
        ));
    }
    let original_index = repo.index().map_err(engine)?;
    if original_index.has_conflicts() && !matches!(action, Action::Stage { .. }) {
        return Err(Error::new(
            "UNMERGED_INDEX",
            "Resolve index conflicts before this operation.",
        ));
    }
    let mut head_index = Index::new().map_err(engine)?;
    match repo.head() {
        Ok(head) => head_index
            .read_tree(&head.peel_to_tree().map_err(engine)?)
            .map_err(engine)?,
        Err(e) if e.code() == git2::ErrorCode::UnbornBranch => {}
        Err(e) => return Err(engine(e)),
    }
    // Status collapses an untracked directory into one row, the way `git status`
    // does. Expand such a selection into the files `git add <dir>` would stage so
    // every check below still sees real blobs.
    let partial = matches!(
        action,
        Action::Stage { hunks: Some(_), .. } | Action::Unstage { hunks: Some(_), .. }
    );
    let mut selected: Vec<Vec<u8>> = Vec::with_capacity(paths.len());
    for bytes in paths {
        let path = Path::new(OsStr::from_bytes(bytes));
        if path.is_absolute()
            || path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(Error::invalid("Invalid index path."));
        }
        if matches!(action, Action::Stage { .. })
            && fs::symlink_metadata(root.join(path)).is_ok_and(|m| m.is_dir())
        {
            if Repository::open(root.join(path)).is_ok() {
                return Err(Error::new(
                    "UNSUPPORTED_CAPABILITY",
                    "Staging submodule directories is not supported yet.",
                ));
            }
            if partial {
                return Err(Error::new(
                    "UNSUPPORTED_CAPABILITY",
                    "Staging selected changes needs a file, not a directory.",
                ));
            }
            selected.extend(directory_files(repo, root, path)?);
        } else {
            selected.push(bytes.clone());
        }
    }
    if selected.len() > MAX_STAGED_PATHS {
        return Err(Error::new(
            "LIMIT_EXCEEDED",
            "That selection holds too many files to stage at once. Stage a smaller part of it.",
        ));
    }
    let paths = &selected[..];
    // Reject unsupported filters rather than silently committing non-filtered content.
    if matches!(action, Action::Stage { .. }) {
        for bytes in paths {
            let path = Path::new(OsStr::from_bytes(bytes));
            for attribute in ["filter", "working-tree-encoding"] {
                let value = repo
                    .get_attr_bytes(path, attribute, git2::AttrCheckFlags::FILE_THEN_INDEX)
                    .map_err(engine)?;
                if !matches!(
                    git2::AttrValue::from_bytes(value),
                    git2::AttrValue::Unspecified | git2::AttrValue::False
                ) {
                    return Err(Error::new("UNSUPPORTED_FILTER","This file requires a filter or encoding that the Git backend does not support yet."));
                }
            }
        }
    }
    let temporary = tempfile::Builder::new()
        .prefix("newport-index-")
        .tempdir_in(repo.path())
        .map_err(io_error)?;
    let temp_index = temporary.path().join("index");
    if original.exists() {
        fs::copy(&original, &temp_index).map_err(io_error)?;
    }
    let mut index = private_index(repo, &temp_index)?;
    let hunk_selection = match action {
        Action::Stage { hunks, .. } | Action::Unstage { hunks, .. } => hunks.as_ref(),
        _ => None,
    };
    if let Some(selection) = hunk_selection {
        super::hunks::apply(
            repo,
            &mut index,
            paths,
            matches!(action, Action::Unstage { .. }),
            selection,
        )?;
    } else {
        for bytes in paths {
            let path = Path::new(OsStr::from_bytes(bytes));
            match action {
                Action::ConflictResolve { .. }
                | Action::Discard { .. }
                | Action::Reset { .. }
                | Action::TagCreate { .. }
                | Action::TagDelete { .. }
                | Action::RemoteAdd { .. }
                | Action::RemoteRename { .. }
                | Action::RemoteSetUrl { .. }
                | Action::RemoteRemove { .. }
                | Action::Fetch { .. }
                | Action::BranchDeleteRemote { .. }
                | Action::TagDeleteRemote { .. }
                | Action::PushWithLease { .. }
                | Action::Push { .. }
                | Action::TagPush { .. }
                | Action::StashSave { .. }
                | Action::StashApply { .. }
                | Action::StashPop { .. }
                | Action::StashDrop { .. }
                | Action::CherryPick { .. }
                | Action::Revert { .. }
                | Action::IntegrationContinue { .. }
                | Action::IntegrationAbort {}
                | Action::MergeAbort { .. }
                | Action::Merge { .. }
                | Action::PullFastForward { .. }
                | Action::FastForward { .. }
                | Action::Checkout { .. }
                | Action::Commit { .. }
                | Action::Rebase { .. }
                | Action::IntegrationSkip {}
                | Action::Amend { .. }
                | Action::BranchSetUpstream { .. }
                | Action::BranchCreate { .. }
                | Action::BranchRename { .. }
                | Action::WorktreeRepair { .. }
                | Action::WorktreeRemove { .. }
                | Action::WorktreePrune { .. }
                | Action::WorktreeAdd { .. }
                | Action::WorktreeLock { .. }
                | Action::WorktreeUnlock { .. }
                | Action::BranchDelete { .. } => unreachable!("handled before index edits"),
                Action::Stage { .. } => match fs::symlink_metadata(root.join(path)) {
                    Ok(_) => index.add_path(path).map_err(engine)?,
                    Err(e) if e.kind() == io::ErrorKind::NotFound => remove(&mut index, path)?,
                    Err(e) => return Err(io_error(e)),
                },
                Action::Unstage { .. } => {
                    if let Some(entry) = head_index.get_path(path, 0) {
                        index.add(&entry).map_err(engine)?;
                    } else {
                        remove(&mut index, path)?;
                    }
                }
            }
        }
    }
    index.write().map_err(engine)?;
    // A new handle sees the original on-disk index, not our temporary index.
    let fresh = Repository::open(repo.path()).map_err(engine)?;
    if repository::fingerprint(&fresh)? != expected {
        return Err(Error::new(
            "STALE_SNAPSHOT",
            "The repository changed while preparing this operation. No index change was published.",
        ));
    }
    lock.publish(&temp_index, repo)?;
    Ok(serde_json::json!({"indexChanged":true,"workingTreeChanged":false,"refreshRequired":true}))
}

/// Publish only the staged tree. Lock both HEAD and its branch before revalidating.
pub(super) fn validate_commit(repo: &Repository, message: &str) -> Result<(), Error> {
    use std::os::unix::fs::PermissionsExt;
    if message.trim().is_empty() || message.len() > 64 * 1024 || message.contains('\0') {
        return Err(Error::invalid(
            "Supply a nonempty commit message of at most 64 KiB without NUL bytes.",
        ));
    }
    let config = repo.config().map_err(engine)?;
    match config.get_bool("commit.gpgsign") {
        Ok(true) => {
            return Err(Error::new(
                "UNSUPPORTED_SIGNING",
                "Signed commits are not implemented yet.",
            ))
        }
        Ok(false) => {}
        Err(e) if e.code() == git2::ErrorCode::NotFound => {}
        Err(e) => return Err(engine(e)),
    }
    let hooks = match config.get_path("core.hooksPath") {
        Ok(path) if path.is_absolute() => path,
        Ok(path) => repo.workdir().unwrap_or(repo.path()).join(path),
        Err(e) if e.code() == git2::ErrorCode::NotFound => repo.commondir().join("hooks"),
        Err(e) => return Err(engine(e)),
    };
    for hook in [
        "pre-commit",
        "prepare-commit-msg",
        "commit-msg",
        "post-commit",
    ] {
        match fs::metadata(hooks.join(hook)) {
            Ok(meta) if meta.permissions().mode() & 0o111 != 0 => {
                return Err(Error::new(
                    "UNSUPPORTED_HOOK",
                    "This repository has commit hooks. Hook execution is not implemented yet.",
                ))
            }
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(io_error(e)),
        }
    }
    Ok(())
}

pub(super) fn commit(
    repo: &Repository,
    message: &str,
    author: Option<&Author>,
    expected: &str,
) -> Result<serde_json::Value, Error> {
    commit_inner(repo, message, author, expected, CommitMode::New)
}

enum CommitMode<'a> {
    New,
    Amend {
        expected_oid: &'a str,
        author: Option<&'a Author>,
    },
}

fn commit_inner(
    repo: &Repository,
    message: &str,
    author: Option<&Author>,
    expected: &str,
    mode: CommitMode<'_>,
) -> Result<serde_json::Value, Error> {
    validate_commit(repo, message)?;
    let amending = matches!(mode, CommitMode::Amend { .. });
    if amending {
        super::checkout::supported_hook(repo, "post-rewrite")?;
    }
    let signature = signature(repo, author)?;
    let _index_lock = IndexLock::acquire(repo)?;
    let head = repo.find_reference("HEAD").map_err(engine)?;
    let target = match head.symbolic_target_bytes() {
        Some(bytes) => std::str::from_utf8(bytes)
            .map_err(|_| {
                Error::new(
                    "UNSUPPORTED_CAPABILITY",
                    "Non-UTF-8 branch names are not supported for commits.",
                )
            })?
            .to_owned(),
        None => "HEAD".to_owned(),
    };
    let mut transaction = repo.transaction().map_err(engine)?;
    let lock_error = |_| {
        Error::new(
            "REPOSITORY_BUSY",
            "HEAD or its branch is locked by another Git operation.",
        )
    };
    transaction.lock_ref("HEAD").map_err(lock_error)?;
    if target != "HEAD" {
        transaction.lock_ref(&target).map_err(lock_error)?;
    }
    if repository::fingerprint(repo)? != expected {
        return Err(Error::new(
            "STALE_SNAPSHOT",
            "The repository changed. Refresh before committing.",
        ));
    }
    let current_head = repo.find_reference("HEAD").map_err(engine)?;
    if current_head.symbolic_target_bytes() != head.symbolic_target_bytes()
        || current_head.target() != head.target()
    {
        return Err(Error::new(
            "STALE_SNAPSHOT",
            "HEAD changed while preparing the commit.",
        ));
    }
    let mut index = repo.index().map_err(engine)?;
    if index.has_conflicts() {
        return Err(Error::new(
            "UNMERGED_INDEX",
            "Resolve conflicts before committing.",
        ));
    }
    let tree_id = index.write_tree().map_err(engine)?;
    let tree = repo.find_tree(tree_id).map_err(engine)?;
    let parent = match repo.head() {
        Ok(head) => Some(head.peel_to_commit().map_err(engine)?),
        Err(e) if e.code() == git2::ErrorCode::UnbornBranch => None,
        Err(e) => return Err(engine(e)),
    };
    if let CommitMode::Amend { expected_oid, .. } = &mode {
        let expected_oid = super::branches::oid(expected_oid)?;
        if repo.state() != git2::RepositoryState::Clean {
            return Err(Error::new(
                "INTEGRATION_IN_PROGRESS",
                "Finish or abort the current integration before amending.",
            ));
        }
        let original = parent
            .as_ref()
            .ok_or_else(|| Error::new("UNBORN_HEAD", "Create a commit before amending."))?;
        if original.id() != expected_oid {
            return Err(Error::new(
                "STALE_REFERENCE",
                "HEAD changed. Refresh history before amending.",
            ));
        }
        // Native amendment reconstructs the commit. Do not silently strip
        // signatures or signed-merge metadata that cannot be regenerated yet.
        for header in ["gpgsig", "gpgsig-sha256", "mergetag"] {
            match original.header_field_bytes(header) {
                Ok(_) => {
                    return Err(Error::new(
                        "UNSUPPORTED_SIGNING",
                        "Amending commits with signature metadata is not supported yet.",
                    ))
                }
                Err(e) if e.code() == git2::ErrorCode::NotFound => {}
                Err(e) => return Err(engine(e)),
            }
        }
    }
    let kind = super::integration::Kind::current(repo);
    let merging = kind == Some(super::integration::Kind::Merge);
    let replaying = matches!(
        kind,
        Some(super::integration::Kind::CherryPick | super::integration::Kind::Revert)
    );
    let replay = if replaying {
        Some(super::integration::current_record(repo)?)
    } else {
        None
    };
    let original_commit = if replay
        .as_ref()
        .is_some_and(|r| r.kind == super::integration::Kind::CherryPick)
    {
        Some(
            repo.find_commit(super::branches::oid(
                &replay.as_ref().expect("replay").target_oid,
            )?)
            .map_err(engine)?,
        )
    } else {
        None
    };
    let original_author = original_commit.as_ref().map(|c| c.author());
    if !amending
        && !merging
        && !replaying
        && (parent.as_ref().is_some_and(|p| p.tree_id() == tree_id)
            || (parent.is_none() && tree.is_empty()))
    {
        return Err(Error::new(
            "NOTHING_TO_COMMIT",
            "Stage changes before committing.",
        ));
    }
    let mut merge_parents = Vec::new();
    if merging {
        let mut fresh = Repository::open(repo.path()).map_err(engine)?;
        let mut heads = Vec::new();
        fresh
            .mergehead_foreach(|oid| {
                heads.push(*oid);
                true
            })
            .map_err(engine)?;
        if heads.len() != 1 {
            return Err(Error::new(
                "UNSUPPORTED_CAPABILITY",
                "Exactly one merge parent is supported.",
            ));
        }
        for oid in heads {
            merge_parents.push(repo.find_commit(oid).map_err(engine)?);
        }
    }
    let parents: Vec<_> = parent.iter().chain(merge_parents.iter()).collect();
    let oid = match mode {
        CommitMode::New => repo
            .commit(
                None,
                original_author.as_ref().unwrap_or(&signature),
                &signature,
                message,
                &tree,
                &parents,
            )
            .map_err(engine)?,
        CommitMode::Amend { author, .. } => {
            let override_author = author.map(|a| self::signature(repo, Some(a))).transpose()?;
            parent
                .as_ref()
                .expect("validated amend target")
                .amend(
                    None,
                    override_author.as_ref(),
                    Some(&signature),
                    Some("UTF-8"),
                    Some(message),
                    Some(&tree),
                )
                .map_err(engine)?
        }
    };
    transaction
        .set_target(
            &target,
            oid,
            Some(&signature),
            if amending {
                "commit (amend): Newport"
            } else {
                "commit: Newport"
            },
        )
        .map_err(engine)?;
    transaction.commit().map_err(|_| Error::new("OUTCOME_UNKNOWN", "The commit object was created, but updating its reference could not be confirmed. Query this operation before retrying."))?;
    if merging || replaying {
        super::integration::cleanup(repo).map_err(|_| {
            Error::new(
                "OUTCOME_UNKNOWN",
                "The commit was created but integration-state cleanup failed.",
            )
        })?;
    }
    let parent_oid = if amending {
        parent.as_ref().and_then(|p| p.parent_ids().next())
    } else {
        parent.as_ref().map(|p| p.id())
    };
    Ok(
        serde_json::json!({"amended":amending,"replacedOid":if amending {parent.as_ref().map(|p|p.id().to_string())} else {None},"mergeCompleted":merging,"integrationCompleted":kind,"commitOid":oid.to_string(), "reference":target, "parentOid":parent_oid.map(|p|p.to_string()), "refreshRequired":true}),
    )
}

pub(super) fn signature(
    repo: &Repository,
    author: Option<&Author>,
) -> Result<git2::Signature<'static>, Error> {
    let config = repo.config().map_err(engine)?;
    let configured;
    let author = match author {
        Some(author) => author,
        None => {
            configured = Author {
                name: config.get_string("user.name").map_err(|_| {
                    Error::new(
                        "AUTHOR_REQUIRED",
                        "Configure a Git author name and email or supply them explicitly.",
                    )
                })?,
                email: config.get_string("user.email").map_err(|_| {
                    Error::new(
                        "AUTHOR_REQUIRED",
                        "Configure a Git author name and email or supply them explicitly.",
                    )
                })?,
            };
            &configured
        }
    };
    for field in [&author.name, &author.email] {
        if field.trim().is_empty()
            || field.len() > 512
            || field
                .chars()
                .any(|c| c.is_control() || c == '<' || c == '>')
        {
            return Err(Error::new("INVALID_AUTHOR", "Author name and email must be nonempty and contain no control characters or angle brackets."));
        }
    }
    git2::Signature::now(author.name.trim(), author.email.trim()).map_err(engine)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn commit_action() -> Action {
        Action::Commit {
            message: "A commit\n\nWith details".into(),
            author: Some(Author {
                name: "Test Author".into(),
                email: "author@example.test".into(),
            }),
        }
    }
    fn stage_file(repo: &Repository) {
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("file")).unwrap();
        index.write().unwrap();
    }
    fn amend_fixture() -> (tempfile::TempDir, Repository, git2::Oid) {
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
        fs::write(temp.path().join("file"), "base").unwrap();
        stage_file(&repo);
        let tree = repo
            .find_tree(repo.index().unwrap().write_tree().unwrap())
            .unwrap();
        let sig = git2::Signature::new(
            "Original",
            "original@example.test",
            &git2::Time::new(123456, 60),
        )
        .unwrap();
        let oid = repo
            .commit(Some("HEAD"), &sig, &sig, "Original message", &tree, &[])
            .unwrap();
        drop(tree);
        (temp, repo, oid)
    }
    fn amend_action(oid: git2::Oid) -> Action {
        Action::Amend {
            expected_oid: oid.to_string(),
            message: "Amended message".into(),
            committer: Some(Author {
                name: "Committer".into(),
                email: "committer@example.test".into(),
            }),
            author: None,
        }
    }
    #[test]
    fn amendment_replays_after_reconnect_without_rewriting_again() {
        use super::super::{
            journal::Journal,
            protocol::{Path as WirePath, Request},
            repository::{Output, Service},
        };
        let (temp, repo, original) = amend_fixture();
        let records = tempfile::tempdir().unwrap();
        let journal = Journal::open(
            records.path().join("journal"),
            uuid::Uuid::new_v4().to_string(),
        )
        .unwrap();
        let read = |service: &mut Service, request| match service.request(request).unwrap() {
            Output::Json(value) => value,
            _ => panic!("expected JSON"),
        };
        let mut service = Service::with_journal(journal.clone());
        let opened = read(
            &mut service,
            Request::Open {
                path: WirePath::new(temp.path().as_os_str().as_bytes()),
            },
        );
        let handle = opened["repoId"].as_str().unwrap().to_owned();
        let status = read(
            &mut service,
            Request::Status {
                repo_id: handle.clone(),
                page_size: 100,
                cursor: None,
            },
        );
        let mut request = Request::Start {
            operation_id: uuid::Uuid::new_v4().to_string(),
            repo_id: handle,
            expected_snapshot: status["snapshot"].as_str().unwrap().into(),
            action: amend_action(original),
        };
        let result = read(&mut service, request.clone());
        assert_eq!(result["state"], "succeeded", "{result}");
        let amended = repo.head().unwrap().target().unwrap();
        fs::write(temp.path().join("file"), "later working change").unwrap();
        let mut reconnected = Service::with_journal(journal);
        let opened = read(
            &mut reconnected,
            Request::Open {
                path: WirePath::new(temp.path().as_os_str().as_bytes()),
            },
        );
        if let Request::Start { repo_id, .. } = &mut request {
            *repo_id = opened["repoId"].as_str().unwrap().into();
        }
        assert_eq!(read(&mut reconnected, request), result);
        assert_eq!(repo.head().unwrap().target(), Some(amended));
        assert_eq!(
            fs::read(temp.path().join("file")).unwrap(),
            b"later working change"
        );
        let history = read(
            &mut reconnected,
            Request::History {
                repo_id: opened["repoId"].as_str().unwrap().into(),
                revision: "HEAD".into(),
                page_size: 100,
                cursor: None,
            },
        );
        assert_eq!(history["entries"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn amendment_preserves_root_author_index_and_working_files() {
        let (temp, repo, original) = amend_fixture();
        fs::write(temp.path().join("file"), "staged").unwrap();
        stage_file(&repo);
        fs::write(temp.path().join("file"), "unstaged").unwrap();
        let index = fs::read(repo.path().join("index")).unwrap();
        let result = apply(
            &repo,
            &amend_action(original),
            &[],
            &repository::fingerprint(&repo).unwrap(),
        )
        .unwrap();
        assert_eq!(result["amended"], true);
        assert_eq!(result["replacedOid"], original.to_string());
        assert!(result["parentOid"].is_null());
        let commit = repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(commit.parent_count(), 0);
        assert_eq!(commit.author().name().unwrap(), "Original");
        assert_eq!(commit.author().when().seconds(), 123456);
        assert_eq!(commit.committer().name().unwrap(), "Committer");
        assert_eq!(commit.message().unwrap(), "Amended message");
        assert_eq!(
            repo.find_blob(commit.tree().unwrap().get_name("file").unwrap().id())
                .unwrap()
                .content(),
            b"staged"
        );
        assert_eq!(fs::read(repo.path().join("index")).unwrap(), index);
        assert_eq!(fs::read(temp.path().join("file")).unwrap(), b"unstaged");
        assert!(repo.find_commit(original).is_ok());
        let replacement = commit.id();
        drop(commit);
        // Message-only amendment is meaningful even when the tree is unchanged.
        let mut action = amend_action(replacement);
        if let Action::Amend { message, .. } = &mut action {
            *message = "Message only".into();
        }
        let result = apply(
            &repo,
            &action,
            &[],
            &repository::fingerprint(&repo).unwrap(),
        );
        assert!(result.is_ok());
    }
    #[test]
    fn amendment_preserves_merge_parents_and_can_override_author_on_detached_head() {
        let (temp, repo, root) = amend_fixture();
        let first = repo.find_commit(root).unwrap();
        let tree = first.tree().unwrap();
        let sig = git2::Signature::now("Fixture", "fixture@example.test").unwrap();
        let side = repo
            .commit(None, &sig, &sig, "Side", &tree, &[&first])
            .unwrap();
        let side = repo.find_commit(side).unwrap();
        let merged = repo
            .commit(Some("HEAD"), &sig, &sig, "Merge", &tree, &[&first, &side])
            .unwrap();
        repo.set_head_detached(merged).unwrap();
        let action = Action::Amend {
            expected_oid: merged.to_string(),
            message: "Amended merge".into(),
            committer: Some(Author {
                name: "Committer".into(),
                email: "committer@example.test".into(),
            }),
            author: Some(Author {
                name: "Replacement Author".into(),
                email: "replacement@example.test".into(),
            }),
        };
        apply(
            &repo,
            &action,
            &[],
            &repository::fingerprint(&repo).unwrap(),
        )
        .unwrap();
        let fresh = Repository::open(temp.path()).unwrap();
        assert!(fresh.head_detached().unwrap());
        let commit = fresh.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(
            commit.parent_ids().collect::<Vec<_>>(),
            vec![root, side.id()]
        );
        assert_eq!(commit.author().name().unwrap(), "Replacement Author");
        assert_eq!(commit.committer().name().unwrap(), "Committer");
        assert_eq!(commit.tree_id(), tree.id());
    }
    #[test]
    fn amendment_refuses_stale_targets_rewrite_hooks_and_signed_metadata() {
        use std::os::unix::fs::PermissionsExt;
        let (temp, repo, original) = amend_fixture();
        assert_eq!(
            apply(
                &repo,
                &amend_action(git2::Oid::ZERO_SHA1),
                &[],
                &repository::fingerprint(&repo).unwrap()
            )
            .unwrap_err()
            .code,
            "STALE_REFERENCE"
        );
        let stale = repository::fingerprint(&repo).unwrap();
        fs::write(temp.path().join("file"), "changed").unwrap();
        assert_eq!(
            apply(&repo, &amend_action(original), &[], &stale)
                .unwrap_err()
                .code,
            "STALE_SNAPSHOT"
        );
        let hook = repo.path().join("hooks/post-rewrite");
        fs::create_dir_all(hook.parent().unwrap()).unwrap();
        fs::write(&hook, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(
            apply(
                &repo,
                &amend_action(original),
                &[],
                &repository::fingerprint(&repo).unwrap()
            )
            .unwrap_err()
            .code,
            "UNSUPPORTED_HOOK"
        );
        fs::remove_file(hook).unwrap();
        let odb = repo.odb().unwrap();
        let raw = odb.read(original).unwrap();
        let signed = repo
            .commit_signed(
                std::str::from_utf8(raw.data()).unwrap(),
                "test signature",
                None,
            )
            .unwrap();
        let branch = repo.head().unwrap().name().unwrap().to_owned();
        repo.reference(&branch, signed, true, "fixture").unwrap();
        assert_eq!(
            apply(
                &repo,
                &amend_action(signed),
                &[],
                &repository::fingerprint(&repo).unwrap()
            )
            .unwrap_err()
            .code,
            "UNSUPPORTED_SIGNING"
        );
        assert_eq!(repo.head().unwrap().target(), Some(signed));
        fs::write(repo.path().join("MERGE_HEAD"), format!("{original}\n")).unwrap();
        assert_eq!(
            apply(
                &repo,
                &amend_action(signed),
                &[],
                &repository::fingerprint(&repo).unwrap()
            )
            .unwrap_err()
            .code,
            "UNSUPPORTED_CAPABILITY"
        );
    }

    #[test]
    fn commits_staged_content_and_supports_unborn_and_detached_heads() {
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
        fs::write(temp.path().join("file"), "staged").unwrap();
        stage_file(&repo);
        fs::write(temp.path().join("file"), "unstaged").unwrap();
        let before = fs::read(repo.path().join("index")).unwrap();
        let result = apply(
            &repo,
            &commit_action(),
            &[],
            &repository::fingerprint(&repo).unwrap(),
        )
        .unwrap();
        let oid = git2::Oid::from_str(result["commitOid"].as_str().unwrap()).unwrap();
        let committed = repo.find_commit(oid).unwrap();
        assert_eq!(committed.parent_count(), 0);
        assert_eq!(committed.author().email().unwrap(), "author@example.test");
        assert_eq!(committed.message().unwrap(), "A commit\n\nWith details");
        let tree = committed.tree().unwrap();
        let blob = repo.find_blob(tree.get_name("file").unwrap().id()).unwrap();
        assert_eq!(blob.content(), b"staged");
        assert_eq!(fs::read(temp.path().join("file")).unwrap(), b"unstaged");
        assert_eq!(fs::read(repo.path().join("index")).unwrap(), before);
        assert_eq!(
            apply(
                &repo,
                &commit_action(),
                &[],
                &repository::fingerprint(&repo).unwrap()
            )
            .unwrap_err()
            .code,
            "NOTHING_TO_COMMIT"
        );
        stage_file(&repo);
        repo.config()
            .unwrap()
            .set_str("user.name", "Configured Author")
            .unwrap();
        repo.config()
            .unwrap()
            .set_str("user.email", "configured@example.test")
            .unwrap();
        let configured = Action::Commit {
            message: "Normal branch commit".into(),
            author: None,
        };
        let normal = apply(
            &repo,
            &configured,
            &[],
            &repository::fingerprint(&repo).unwrap(),
        )
        .unwrap();
        let normal_oid = repo.head().unwrap().target().unwrap();
        assert_eq!(normal["commitOid"], normal_oid.to_string());
        assert_eq!(normal["parentOid"], oid.to_string());
        assert_eq!(
            repo.find_commit(normal_oid)
                .unwrap()
                .author()
                .email()
                .unwrap(),
            "configured@example.test"
        );
        assert!(!repo.head_detached().unwrap());
        repo.set_head_detached(oid).unwrap();
        stage_file(&repo);
        let result = apply(
            &repo,
            &commit_action(),
            &[],
            &repository::fingerprint(&repo).unwrap(),
        )
        .unwrap();
        assert_eq!(result["reference"], "HEAD");
        assert_eq!(result["parentOid"], oid.to_string());
        assert!(repo.head_detached().unwrap());
        let next = repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(next.parent_id(0).unwrap(), oid);
    }
    #[test]
    fn commit_rejects_stale_state_locks_hooks_signing_and_invalid_authors() {
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
        fs::write(temp.path().join("file"), "staged").unwrap();
        stage_file(&repo);
        let expected = repository::fingerprint(&repo).unwrap();
        let lock = repo.path().join("HEAD.lock");
        fs::write(&lock, "external").unwrap();
        assert_eq!(
            apply(&repo, &commit_action(), &[], &expected)
                .unwrap_err()
                .code,
            "REPOSITORY_BUSY"
        );
        assert_eq!(fs::read(&lock).unwrap(), b"external");
        fs::remove_file(lock).unwrap();
        fs::write(temp.path().join("file"), "changed").unwrap();
        assert_eq!(
            apply(&repo, &commit_action(), &[], &expected)
                .unwrap_err()
                .code,
            "STALE_SNAPSHOT"
        );
        let mut config = repo.config().unwrap();
        config.set_bool("commit.gpgsign", true).unwrap();
        assert_eq!(
            apply(&repo, &commit_action(), &[], &expected)
                .unwrap_err()
                .code,
            "UNSUPPORTED_SIGNING"
        );
        config.set_bool("commit.gpgsign", false).unwrap();
        let hook = repo.path().join("hooks/pre-commit");
        fs::write(&hook, "#!/bin/sh\nexit 0").unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(
            apply(&repo, &commit_action(), &[], &expected)
                .unwrap_err()
                .code,
            "UNSUPPORTED_HOOK"
        );
        fs::remove_file(hook).unwrap();
        let bad = Action::Commit {
            message: "test".into(),
            author: Some(Author {
                name: "bad\nname".into(),
                email: "test@example.test".into(),
            }),
        };
        assert_eq!(
            apply(&repo, &bad, &[], &expected).unwrap_err().code,
            "INVALID_AUTHOR"
        );
        assert!(repo.head().is_err());
        assert!(!repo.path().join("index.lock").exists());
    }
    #[test]
    fn unborn_index_stale_snapshot_and_external_lock() {
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
        fs::write(temp.path().join("new.txt"), b"new contents").unwrap();
        let paths = vec![b"new.txt".to_vec()];
        let stage = Action::Stage {
            hunks: None,
            entry_ids: vec![],
        };
        let expected = repository::fingerprint(&repo).unwrap();
        let lock = repo.path().join("index.lock");
        fs::write(&lock, b"another process").unwrap();
        assert_eq!(
            apply(&repo, &stage, &paths, &expected).unwrap_err().code,
            "REPOSITORY_BUSY"
        );
        assert_eq!(fs::read(&lock).unwrap(), b"another process");
        fs::remove_file(&lock).unwrap();
        fs::write(temp.path().join("new.txt"), b"changed").unwrap();
        assert_eq!(
            apply(&repo, &stage, &paths, &expected).unwrap_err().code,
            "STALE_SNAPSHOT"
        );
        assert!(!repo.path().join("index").exists());
        let expected = repository::fingerprint(&repo).unwrap();
        apply(&repo, &stage, &paths, &expected).unwrap();
        let repo = Repository::open(temp.path()).unwrap();
        assert_eq!(repo.index().unwrap().len(), 1);
        let expected = repository::fingerprint(&repo).unwrap();
        apply(
            &repo,
            &Action::Unstage {
                hunks: None,
                entry_ids: vec![],
            },
            &paths,
            &expected,
        )
        .unwrap();
        let repo = Repository::open(temp.path()).unwrap();
        assert!(repo.index().unwrap().is_empty());
        assert_eq!(fs::read(temp.path().join("new.txt")).unwrap(), b"changed");
    }
}
