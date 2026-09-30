//! Worktree administration; mutations use a worktree-list snapshot and the
//! journal's common-repository lock, not a working-tree status snapshot.
use super::{
    protocol::{Action, Error, Path as WirePath},
    repository,
};
use git2::Repository;
use serde_json::{json, Value};
use std::{
    ffi::OsStr,
    fs::{self, File},
    os::unix::ffi::OsStrExt,
    path::{Component, Path},
};

fn stale() -> Error {
    Error::new(
        "STALE_SNAPSHOT",
        "Worktrees changed. Refresh the listing before continuing.",
    )
}
fn unknown() -> Error {
    Error::new(
        "OUTCOME_UNKNOWN",
        "Worktree metadata or files may have changed. Inspect the operation and refresh worktrees.",
    )
}
pub fn token(repo: &Repository) -> Result<String, Error> {
    repository::worktree_listing::scan(repo, |_, _| Ok(()))
}
fn selected(repo: &Repository, name: &str, expected: &str) -> Result<Value, Error> {
    let mut selected = None;
    let selection = WirePath::new(name.as_bytes());
    let fingerprint = repository::worktree_listing::scan(repo, |row, _| {
        if row["kind"] == "linked" && row["name"]["bytesB64"] == selection.bytes_b64 {
            selected = Some(row);
        }
        Ok(())
    })?;
    if fingerprint != expected {
        return Err(stale());
    }
    selected.ok_or_else(|| {
        Error::new(
            "WORKTREE_NOT_FOUND",
            "The linked worktree no longer exists.",
        )
    })
}
fn engine(_: git2::Error) -> Error {
    Error::new("GIT_ERROR", "Cannot prepare the linked worktree.")
}
fn absent(path: &Path) -> Result<(), Error> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(Error::new(
            "PATH_EXISTS",
            "The destination already exists; it will not be replaced.",
        )),
        Err(_) => Err(Error::new("IO_ERROR", "Cannot inspect the destination.")),
    }
}
#[allow(clippy::too_many_arguments)]
fn add(
    repo: &Repository,
    name: &str,
    path: &WirePath,
    branch: &str,
    expected_oid: &str,
    locked: bool,
    new_branch: bool,
    expected: &str,
) -> Result<Value, Error> {
    if name.is_empty()
        || name.len() > 255
        || name == "."
        || name == ".."
        || name.contains(['/', '\0'])
    {
        return Err(Error::invalid(
            "Supply a worktree name of at most 255 bytes without path separators.",
        ));
    }
    super::branches::name(branch)?;
    let expected_oid = super::branches::oid(expected_oid)?;
    let bytes = path.decode()?;
    let requested = Path::new(OsStr::from_bytes(&bytes));
    if !requested.is_absolute()
        || requested
            .components()
            .any(|c| matches!(c, Component::ParentDir))
    {
        return Err(Error::invalid(
            "Choose an absolute worktree destination without parent traversal.",
        ));
    }
    let leaf = requested
        .file_name()
        .ok_or_else(|| Error::invalid("Choose a new directory name."))?;
    let parent = requested
        .parent()
        .ok_or_else(|| Error::invalid("Choose a destination parent."))?
        .canonicalize()
        .map_err(|_| {
            Error::new(
                "DIRECTORY_REQUIRED",
                "The destination parent must already exist.",
            )
        })?;
    if !parent.is_dir() {
        return Err(Error::new(
            "DIRECTORY_REQUIRED",
            "The destination parent must be a directory.",
        ));
    }
    let destination = parent.join(leaf);
    let common = repo.commondir().canonicalize().map_err(|_| stale())?;
    if destination.starts_with(&common) {
        return Err(Error::invalid(
            "A checkout cannot be placed inside Git's administrative directory.",
        ));
    }
    let registry = common.join("worktrees");
    match fs::symlink_metadata(&registry) {
        Ok(meta) if !meta.is_dir() => {
            return Err(Error::new(
                "INVALID_WORKTREE",
                "The worktree registry is not a plain directory.",
            ))
        }
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(stale()),
        _ => {}
    }
    absent(&destination)?;
    absent(&registry.join(name))?;
    if token(repo)? != expected {
        return Err(stale());
    }
    let reference_name = format!("refs/heads/{branch}");
    if new_branch {
        // Everything that can refuse is checked before the branch exists, so
        // a refusal leaves nothing behind; the branch is created without
        // force, so a branch that appeared meanwhile is never overwritten.
        match repo.find_reference(&reference_name) {
            Ok(_) => {
                return Err(Error::new(
                    "BRANCH_EXISTS",
                    "A branch with that name already exists. Choose it as an existing branch, or pick another name.",
                ))
            }
            Err(e) if e.code() == git2::ErrorCode::NotFound => {}
            Err(e) => return Err(engine(e)),
        }
        let commit = repo.find_commit(expected_oid).map_err(|_| {
            Error::new(
                "STALE_REFERENCE",
                "The starting commit is not in this repository. Refresh and choose it again.",
            )
        })?;
        super::checkout::supported_hook(repo, "post-checkout")?;
        super::checkout::supported_files(repo, &commit.tree().map_err(engine)?)?;
        if token(repo)? != expected {
            return Err(stale());
        }
        repo.reference(
            &reference_name,
            expected_oid,
            false,
            "worktree.add: create branch",
        )
        .map_err(|e| {
            if e.code() == git2::ErrorCode::Exists {
                Error::new(
                    "BRANCH_EXISTS",
                    "A branch with that name already exists. Choose it as an existing branch, or pick another name.",
                )
            } else {
                engine(e)
            }
        })?;
        let created = repo
            .find_reference(&reference_name)
            .map_err(|_| unknown())?;
        let mut options = git2::WorktreeAddOptions::new();
        options.reference(Some(&created)).lock(locked);
        return match repo.worktree(name, &destination, Some(&options)) {
            Ok(worktree) => finish(
                &worktree,
                name,
                branch,
                expected_oid,
                locked,
                true,
                (&common, &registry, &destination, &parent),
            ),
            Err(_) => {
                // Nothing may have been checked out yet; take back the branch
                // only if it is still exactly the one just created, and still
                // report the outcome as unknown -- partial registration data
                // may remain.
                if let Ok(mut reference) = repo.find_reference(&reference_name) {
                    if reference.target() == Some(expected_oid) {
                        let _ = reference.delete();
                    }
                }
                Err(unknown())
            }
        };
    }
    let mut transaction = repo.transaction().map_err(engine)?;
    transaction.lock_ref(&reference_name).map_err(|_| {
        Error::new(
            "REPOSITORY_BUSY",
            "The selected branch is locked by another operation.",
        )
    })?;
    let reference = repo.find_reference(&reference_name).map_err(engine)?;
    if reference.target() != Some(expected_oid) {
        return Err(Error::new(
            "STALE_REFERENCE",
            "The selected branch moved. Refresh before creating the worktree.",
        ));
    }
    if (!repo.is_bare()
        && repo
            .find_reference("HEAD")
            .map_err(engine)?
            .symbolic_target_bytes()
            == Some(reference_name.as_bytes()))
        || super::branches::other_worktree(repo, &reference_name)?
    {
        return Err(Error::new(
            "BRANCH_IN_USE",
            "The selected branch is already checked out in a worktree.",
        ));
    }
    let commit = repo.find_commit(expected_oid).map_err(engine)?;
    super::checkout::supported_hook(repo, "post-checkout")?;
    super::checkout::supported_files(repo, &commit.tree().map_err(engine)?)?;
    if token(repo)? != expected {
        return Err(stale());
    }
    let mut options = git2::WorktreeAddOptions::new();
    options.reference(Some(&reference)).lock(locked);
    // Native creation exclusively creates both directories. After entering it,
    // failure may leave partial registration/checkout data; never blindly remove
    // those paths or claim that a retry is safe.
    let worktree = repo
        .worktree(name, &destination, Some(&options))
        .map_err(|_| unknown())?;
    finish(
        &worktree,
        name,
        branch,
        expected_oid,
        locked,
        false,
        (&common, &registry, &destination, &parent),
    )
}
/// Verifies a newly created worktree and makes its registration durable.
#[allow(clippy::too_many_arguments)]
fn finish(
    worktree: &git2::Worktree,
    name: &str,
    branch: &str,
    expected_oid: git2::Oid,
    locked: bool,
    branch_created: bool,
    (common, registry, destination, parent): (&Path, &Path, &Path, &Path),
) -> Result<Value, Error> {
    worktree.validate().map_err(|_| unknown())?;
    let linked = Repository::open_from_worktree(worktree).map_err(|_| unknown())?;
    if linked.head().map_err(|_| unknown())?.target() != Some(expected_oid)
        || linked.commondir().canonicalize().ok().as_deref() != Some(common)
    {
        return Err(unknown());
    }
    let admin = registry.join(name);
    for file in ["HEAD", "index", "commondir", "gitdir"] {
        File::open(admin.join(file))
            .and_then(|f| f.sync_all())
            .map_err(|_| unknown())?;
    }
    if locked {
        File::open(admin.join("locked"))
            .and_then(|f| f.sync_all())
            .map_err(|_| unknown())?;
    }
    File::open(destination.join(".git"))
        .and_then(|f| f.sync_all())
        .map_err(|_| unknown())?;
    for directory in [admin.as_path(), registry, common, destination, parent] {
        File::open(directory)
            .and_then(|f| f.sync_all())
            .map_err(|_| unknown())?;
    }
    Ok(
        json!({"name":name,"path":WirePath::new(destination.as_os_str().as_bytes()),"branch":branch,"oid":expected_oid.to_string(),"locked":locked,"branchCreated":branch_created,"openRequired":true,"refreshRequired":true}),
    )
}
fn repair(repo: &Repository, name: &str, path: &WirePath, expected: &str) -> Result<Value, Error> {
    use std::io::Write;
    if name.is_empty()
        || name.len() > 1024
        || name == "."
        || name == ".."
        || name.contains(['/', '\0'])
    {
        return Err(Error::invalid(
            "Select a linked worktree name from the listing.",
        ));
    }
    let row = selected(repo, name, expected)?;
    if !matches!(row["state"].as_str(), Some("missing" | "available")) {
        return Err(Error::new(
            "INVALID_WORKTREE",
            "This registration cannot be repaired as a moved checkout.",
        ));
    }
    let bytes = path.decode()?;
    let requested = Path::new(OsStr::from_bytes(&bytes));
    if !requested.is_absolute()
        || requested
            .components()
            .any(|c| matches!(c, Component::ParentDir))
        || !fs::symlink_metadata(requested).is_ok_and(|m| m.is_dir())
    {
        return Err(Error::invalid(
            "Select the existing moved directory, not a symbolic link.",
        ));
    }
    let destination = requested.canonicalize().map_err(|_| stale())?;
    let common = repo.commondir().canonicalize().map_err(|_| stale())?;
    let admin = common.join("worktrees").join(name);
    let checkout_link = repository::worktree_metadata(&destination.join(".git"))?;
    let link = checkout_link.strip_prefix(b"gitdir: ").ok_or_else(|| {
        Error::new(
            "WORKTREE_MISMATCH",
            "The selected directory is not a linked checkout.",
        )
    })?;
    let link = link.strip_suffix(b"\n").unwrap_or(link);
    let link = link.strip_suffix(b"\r").unwrap_or(link);
    let link = Path::new(OsStr::from_bytes(link));
    let resolved = if link.is_absolute() {
        link.to_owned()
    } else {
        destination.join(link)
    };
    if resolved.canonicalize().ok().as_ref() != Some(&admin) {
        return Err(Error::new(
            "WORKTREE_MISMATCH",
            "The selected checkout does not point to this worktree registration.",
        ));
    }
    let worktree = repo.find_worktree(name).map_err(engine)?;
    if row["state"] == "available" {
        if worktree.path().canonicalize().ok().as_ref() == Some(&destination) {
            return Ok(
                json!({"name":name,"path":WirePath::new(destination.as_os_str().as_bytes()),"repaired":true,"changed":false,"refreshRequired":true}),
            );
        }
        return Err(Error::new(
            "PATH_EXISTS",
            "The original checkout still exists. Its registration was not redirected.",
        ));
    }
    absent(worktree.path())?;
    absent(&admin.join("gitdir.lock"))?;
    let original = repository::worktree_metadata(&admin.join("gitdir"))?;
    let mut replacement = destination.join(".git").as_os_str().as_bytes().to_vec();
    replacement.push(b'\n');
    let mut temporary = tempfile::NamedTempFile::new_in(&admin).map_err(|_| stale())?;
    let permissions = fs::symlink_metadata(admin.join("gitdir"))
        .map_err(|_| stale())?
        .permissions();
    temporary
        .as_file()
        .set_permissions(permissions)
        .map_err(|_| stale())?;
    temporary.write_all(&replacement).map_err(|_| stale())?;
    temporary.as_file().sync_all().map_err(|_| stale())?;
    if token(repo)? != expected
        || repository::worktree_metadata(&admin.join("gitdir"))? != original
        || repository::worktree_metadata(&destination.join(".git"))? != checkout_link
    {
        return Err(stale());
    }
    absent(worktree.path())?;
    temporary
        .persist(admin.join("gitdir"))
        .map_err(|_| unknown())?;
    File::open(&admin)
        .and_then(|f| f.sync_all())
        .map_err(|_| unknown())?;
    let repaired = repo.find_worktree(name).map_err(|_| unknown())?;
    repaired.validate().map_err(|_| unknown())?;
    if repaired.path().canonicalize().ok().as_ref() != Some(&destination) {
        return Err(unknown());
    }
    Ok(
        json!({"name":name,"path":WirePath::new(destination.as_os_str().as_bytes()),"repaired":true,"changed":true,"refreshRequired":true,"openRequired":true}),
    )
}
fn clean_checkout(repo: &Repository) -> Result<(), Error> {
    if repo.state() != git2::RepositoryState::Clean {
        return Err(Error::new(
            "INTEGRATION_IN_PROGRESS",
            "Finish or abort the integration before removing this checkout.",
        ));
    }
    let index = repo.index().map_err(engine)?;
    if index.len() > 20_000 {
        return Err(Error::new(
            "LIMIT_EXCEEDED",
            "Worktree removal exceeds the index entry limit.",
        ));
    }
    if index.has_conflicts()
        || index
            .iter()
            .any(|e| e.flags & 0x8000 != 0 || e.flags_extended & 0x4000 != 0)
    {
        return Err(Error::new("UNSUPPORTED_INDEX", "Resolve conflicts and clear assume-unchanged or skip-worktree flags before removing this checkout."));
    }
    // Do not let Git ignore rules hide user data from a recursive removal.
    let mut options = git2::StatusOptions::new();
    options
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(true)
        .recurse_ignored_dirs(true)
        .update_index(false);
    if !repo
        .statuses(Some(&mut options))
        .map_err(engine)?
        .is_empty()
    {
        return Err(Error::new("WORKTREE_DIRTY", "This checkout contains changes, untracked or ignored files. Preserve or remove them before deleting the checkout."));
    }
    let tree = repo
        .head()
        .map_err(engine)?
        .peel_to_tree()
        .map_err(engine)?;
    super::checkout::supported_files(repo, &tree)?;
    for item in fs::read_dir(repo.path()).map_err(|_| stale())? {
        if item
            .map_err(|_| stale())?
            .file_name()
            .as_bytes()
            .ends_with(b".lock")
        {
            return Err(Error::new(
                "REPOSITORY_BUSY",
                "Another Git operation holds a worktree metadata lock.",
            ));
        }
    }
    Ok(())
}
fn remove(
    repo: &Repository,
    name: &str,
    registration_only: bool,
    expected: &str,
) -> Result<Value, Error> {
    if name.is_empty()
        || name.len() > 1024
        || name == "."
        || name == ".."
        || name.contains(['/', '\0'])
    {
        return Err(Error::invalid(
            "Select a linked worktree name from the listing.",
        ));
    }
    let row = selected(repo, name, expected)?;
    if row["current"] == true {
        return Err(Error::new(
            "CURRENT_WORKTREE",
            "Open another checkout before removing this worktree.",
        ));
    }
    if row["locked"] != false {
        return Err(Error::new(
            "WORKTREE_LOCKED",
            "Unlock this worktree before removing or pruning it.",
        ));
    }
    let required = if registration_only {
        "missing"
    } else {
        "available"
    };
    if row["state"] != required {
        return Err(Error::new(
            "INVALID_WORKTREE",
            if registration_only {
                "Only a missing checkout registration can be pruned."
            } else {
                "Only a valid checkout can be removed. Use prune for missing directories."
            },
        ));
    }
    let worktree = repo.find_worktree(name).map_err(engine)?;
    let root = worktree.path().to_owned();
    let admin = repo.commondir().join("worktrees").join(name);
    for item in fs::read_dir(&admin).map_err(|_| stale())? {
        if item
            .map_err(|_| stale())?
            .file_name()
            .as_bytes()
            .ends_with(b".lock")
        {
            return Err(Error::new(
                "REPOSITORY_BUSY",
                "Another Git operation holds a worktree metadata lock.",
            ));
        }
    }
    let linked = if registration_only {
        absent(&root)?;
        None
    } else {
        if !fs::symlink_metadata(&root).is_ok_and(|m| m.is_dir())
            || root.canonicalize().ok().as_ref() != Some(&root)
        {
            return Err(Error::new(
                "INVALID_WORKTREE",
                "The checkout path must be a plain directory without symbolic-link aliases.",
            ));
        }
        let linked = Repository::open_from_worktree(&worktree).map_err(engine)?;
        clean_checkout(&linked)?;
        Some(linked)
    };
    if token(repo)? != expected {
        return Err(stale());
    }
    if let Some(linked) = &linked {
        clean_checkout(linked)?;
    } else {
        absent(&root)?;
    }
    let mut options = git2::WorktreePruneOptions::new();
    options
        .valid(!registration_only)
        .working_tree(!registration_only)
        .locked(false);
    if !worktree.is_prunable(Some(&mut options)).map_err(engine)? {
        return Err(stale());
    }
    // libgit2 removes registration before checkout files. Any failure here can
    // be partial; retain the journal's uncertain outcome instead of retrying.
    worktree.prune(Some(&mut options)).map_err(|_| unknown())?;
    if fs::symlink_metadata(&admin).is_ok()
        || (!registration_only && fs::symlink_metadata(&root).is_ok())
    {
        return Err(unknown());
    }
    let registry = repo.commondir().join("worktrees");
    File::open(registry)
        .and_then(|f| f.sync_all())
        .map_err(|_| unknown())?;
    if !registration_only {
        File::open(root.parent().ok_or_else(unknown)?)
            .and_then(|f| f.sync_all())
            .map_err(|_| unknown())?;
    }
    Ok(
        json!({"name":name,"removed":true,"registrationOnly":registration_only,"refreshRequired":true}),
    )
}
pub fn apply(repo: &Repository, action: &Action, expected: &str) -> Result<Value, Error> {
    match action {
        Action::WorktreeRepair { name, path } => return repair(repo, name, path, expected),
        Action::WorktreeRemove { name } => return remove(repo, name, false, expected),
        Action::WorktreePrune { name } => return remove(repo, name, true, expected),
        _ => {}
    }

    if let Action::WorktreeAdd {
        name,
        path,
        branch,
        expected_oid,
        locked,
        new_branch,
    } = action
    {
        return add(
            repo,
            name,
            path,
            branch,
            expected_oid,
            *locked,
            *new_branch,
            expected,
        );
    }
    let (name, reason, locking) = match action {
        Action::WorktreeLock { name, reason } => (name, reason.as_deref(), true),
        Action::WorktreeUnlock { name } => (name, None, false),
        _ => return Err(Error::invalid("Not a worktree lock operation.")),
    };
    if name.is_empty()
        || name.len() > 1024
        || name == "."
        || name == ".."
        || name.contains(['/', '\0'])
    {
        return Err(Error::invalid(
            "Select a linked worktree name from the listing.",
        ));
    }
    if reason.is_some_and(|s| s.len() > 4096 || s.contains('\0')) {
        return Err(Error::invalid(
            "A lock reason must be at most 4 KiB without NUL bytes.",
        ));
    }
    let row = selected(repo, name, expected)?;
    if !matches!(row["state"].as_str(), Some("available" | "missing"))
        || !row["locked"].is_boolean()
    {
        return Err(Error::new(
            "INVALID_WORKTREE",
            "Repair the worktree metadata before changing its lock.",
        ));
    }
    let locked = row["locked"] == true;
    if locking && locked {
        return Err(Error::new(
            "WORKTREE_LOCKED",
            "This worktree is already locked. Its reason was not replaced.",
        ));
    }
    if !locking && !locked {
        return Ok(json!({"name":name,"locked":false,"changed":false,"refreshRequired":true}));
    }
    let worktree = repo.find_worktree(name).map_err(|_| stale())?;
    // Recheck immediately before the native operation. Other Newport sessions
    // share the journal lock; external Git clients do not participate in it.
    if token(repo)? != expected {
        return Err(stale());
    }
    if locking {
        worktree.lock(reason).map_err(|e| match e.code() {
            git2::ErrorCode::Locked | git2::ErrorCode::Exists => Error::new(
                "WORKTREE_LOCKED",
                "The worktree was locked by another operation. Refresh the listing.",
            ),
            _ => unknown(),
        })?;
    } else {
        worktree.unlock().map_err(|_| unknown())?;
    }
    let admin = repo.commondir().join("worktrees").join(name);
    if locking {
        File::open(admin.join("locked"))
            .and_then(|f| f.sync_all())
            .map_err(|_| unknown())?;
    }
    File::open(admin)
        .and_then(|f| f.sync_all())
        .map_err(|_| unknown())?;
    Ok(json!({"name":name,"locked":locking,"changed":true,"refreshRequired":true}))
}
