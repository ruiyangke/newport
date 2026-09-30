use super::super::tokens::SnapshotRef;
use super::*;
pub(super) fn rows(repo: &Repo) -> Result<(Vec<Value>, String), Error> {
    let bytes = repo.run(&["worktree", "list", "--porcelain", "-z"])?;
    let mut rows = Vec::new();
    let mut record = Vec::new();
    for field in bytes.split(|b| *b == 0) {
        if !field.is_empty() {
            record.push(field);
            continue;
        }
        if record.is_empty() {
            continue;
        }
        let path = record
            .iter()
            .find_map(|f| f.strip_prefix(b"worktree "))
            .ok_or_else(failure)?;
        let root = Path::new(OsStr::from_bytes(path));
        let main = rows.is_empty();
        let bare = record.contains(&b"bare".as_slice());
        let mut git_dir = repo.common.clone();
        let mut name = None;
        if !main {
            for entry in fs::read_dir(repo.common.join("worktrees")).map_err(io_error)? {
                let entry = entry.map_err(io_error)?;
                if let Ok(data) = fs::read(entry.path().join("gitdir")) {
                    if Path::new(OsStr::from_bytes(trim_line(&data))).parent() == Some(root) {
                        git_dir = entry.path();
                        name = Some(WirePath::new(entry.file_name().as_bytes()));
                        break;
                    }
                }
            }
        }
        let branch = record.iter().find_map(|f| f.strip_prefix(b"branch "));
        let id = record.iter().find_map(|f| f.strip_prefix(b"HEAD "));
        let id = id.filter(|s| !s.iter().all(|b| *b == b'0'));
        let reason = record.iter().find_map(|f| f.strip_prefix(b"locked "));
        let locked = reason.is_some() || record.contains(&b"locked".as_slice());
        let prunable = record.iter().any(|f| f.starts_with(b"prunable"));
        rows.push(json!({"name":name,"kind":if bare{"bare"}else if main{"main"}else{"linked"},"path":WirePath::new(path),"gitDir":WirePath::new(git_dir.as_os_str().as_bytes()),"current":git_dir==repo.git_dir,"state":if root.exists(){"available"}else{"missing"},"head":{"oid":id.map(oid).transpose()?,"name":branch.map(WirePath::new).or_else(||Some(WirePath::new(b"HEAD"))),"detached":branch.is_none(),"unborn":id.is_none()},"locked":locked,"lockReason":reason.map(WirePath::new),"prunable":prunable}));
        record.clear();
    }
    let fingerprint = pages::hash(&json!(rows))?;
    Ok((rows, fingerprint))
}
#[allow(clippy::too_many_arguments)]
pub(super) fn page(
    repo: &Repo,
    count: usize,
    cursor: Option<String>,
    filter: String,
    branch: Option<String>,
    name: Option<String>,
    at: Option<String>,
) -> Result<Value, Error> {
    if filter.len() > 1024
        || filter.contains('\0')
        || branch
            .as_ref()
            .is_some_and(|s| s.len() > 1024 || s.contains('\0'))
        || name
            .as_ref()
            .is_some_and(|s| s.len() > 1024 || s.contains('\0'))
    {
        return Err(Error::invalid("Invalid worktree filter."));
    }
    if at.is_some() && cursor.is_some() {
        return Err(Error::invalid("atSnapshot and cursor cannot be combined."));
    }
    let (mut rows, fingerprint) = rows(repo)?;
    if let Some(at) = at {
        let s = SnapshotRef::decode(&at)?;
        if s.r != repo.id || s.q != "cli.worktrees" || s.f != fingerprint {
            return Err(Error::new(
                "SNAPSHOT_EXPIRED",
                "Worktrees changed. Refresh the listing.",
            ));
        }
    }
    let total = rows.len();
    let current = rows.iter().find(|r| r["current"] == true).cloned();
    let main = rows.iter().find(|r| r["kind"] != "linked").cloned();
    rows.retain(|r| {
        branch
            .as_ref()
            .is_none_or(|b| r["head"]["name"]["display"] == *b)
            && name.as_ref().is_none_or(|n| r["name"]["display"] == *n)
            && format!(
                "{} {}",
                r["path"]["display"].as_str().unwrap_or_default(),
                r["name"]["display"].as_str().unwrap_or_default()
            )
            .to_lowercase()
            .contains(&filter.to_lowercase())
    });
    let matching = rows.len();
    let mut result = pages::page(
        repo,
        format!("cli.worktrees:{}", json!([filter, branch, name])),
        fingerprint.clone(),
        rows,
        count,
        cursor,
        json!({"listToken":fingerprint,"totalEntries":total,"matchingEntries":matching,"current":current,"main":main}),
    )?;
    result["snapshot"] = json!(SnapshotRef {
        r: repo.id.clone(),
        q: "cli.worktrees".into(),
        f: fingerprint,
        p: None
    }
    .encode());
    Ok(result)
}

pub(super) fn is_action(action: &super::super::protocol::Action) -> bool {
    use super::super::protocol::Action;
    matches!(
        action,
        Action::WorktreeAdd { .. }
            | Action::WorktreeRemove { .. }
            | Action::WorktreeRepair { .. }
            | Action::WorktreePrune { .. }
            | Action::WorktreeLock { .. }
            | Action::WorktreeUnlock { .. }
    )
}
fn execute(repo: &Repo, args: Vec<std::ffi::OsString>) -> Result<(), Error> {
    command::write(&repo.root, &args, vec![]).map_err(|e| {
        if e.code == "TIMEOUT" {
            Error::new(
                "OUTCOME_UNKNOWN",
                "The worktree operation timed out. Inspect the saved outcome.",
            )
        } else {
            e
        }
    })
}
pub(super) fn apply(repo: &Repo, action: &super::super::protocol::Action) -> Result<Value, Error> {
    use super::super::protocol::Action;
    use std::ffi::OsString;
    let strings = |args: &[&str]| args.iter().map(OsString::from).collect::<Vec<_>>();
    if let Action::WorktreeAdd {
        name,
        path,
        branch,
        expected_oid,
        locked,
        new_branch,
    } = action
    {
        if name.is_empty()
            || name.len() > 255
            || name.contains(['/', '\0'])
            || name == "."
            || name == ".."
        {
            return Err(Error::invalid("Invalid worktree name."));
        }
        let reference = writes::name(repo, branch, "heads")?;
        oid(expected_oid.as_bytes())?;
        if !new_branch {
            let actual = repo.run(&["rev-parse", "--verify", "--end-of-options", &reference])?;
            if trim_line(&actual) != expected_oid.as_bytes() {
                return Err(Error::new("STALE_REFERENCE", "The branch changed."));
            }
        }
        let bytes = path.decode()?;
        let destination = Path::new(OsStr::from_bytes(&bytes));
        if !destination.is_absolute()
            || destination
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(Error::invalid("Choose an absolute worktree destination."));
        }
        if fs::symlink_metadata(destination).is_ok()
            || repo.common.join("worktrees").join(name).exists()
        {
            return Err(Error::new(
                "PATH_EXISTS",
                "The destination or worktree name already exists.",
            ));
        }
        let parent = destination
            .parent()
            .ok_or_else(failure)?
            .canonicalize()
            .map_err(io_error)?;
        let temporary = tempfile::Builder::new()
            .prefix(".newport-cli-worktree-")
            .tempdir_in(&parent)
            .map_err(io_error)?;
        let prepared = temporary.path().join(name);
        let mut args = strings(&["worktree", "add"]);
        if *new_branch {
            args.extend(strings(&["-b", branch]));
        }
        args.push("--".into());
        args.push(prepared.as_os_str().into());
        args.push(if *new_branch { expected_oid } else { branch }.into());
        // A failed add/move may leave recovery metadata. Keep its directory rather
        // than letting TempDir delete a worktree the command may have created.
        if let Err(e) = execute(repo, args) {
            let _ = temporary.keep();
            return Err(e);
        }
        let args = vec![
            "worktree".into(),
            "move".into(),
            "--".into(),
            prepared.as_os_str().into(),
            destination.as_os_str().into(),
        ];
        if let Err(e) = execute(repo, args) {
            let _ = temporary.keep();
            return Err(e);
        }
        if *locked {
            execute(
                repo,
                vec![
                    "worktree".into(),
                    "lock".into(),
                    "--".into(),
                    destination.as_os_str().into(),
                ],
            )?;
        }
        return Ok(json!({"name":name,"path":path,"refreshRequired":true}));
    }
    let name = match action {
        Action::WorktreeRemove { name }
        | Action::WorktreeRepair { name, .. }
        | Action::WorktreePrune { name }
        | Action::WorktreeLock { name, .. }
        | Action::WorktreeUnlock { name } => name,
        _ => return Err(Error::invalid("Expected worktree action.")),
    };
    let (rows, _) = rows(repo)?;
    let selected = rows
        .iter()
        .find(|r| {
            r["kind"] == "linked"
                && r["name"]["bytesB64"] == WirePath::new(name.as_bytes()).bytes_b64
        })
        .ok_or_else(|| {
            Error::new(
                "WORKTREE_NOT_FOUND",
                "The linked worktree no longer exists.",
            )
        })?;
    let wire: WirePath = serde_json::from_value(selected["path"].clone()).map_err(|_| failure())?;
    let bytes = wire.decode()?;
    let path = Path::new(OsStr::from_bytes(&bytes));
    let mut args = strings(&["worktree"]);
    match action {
        Action::WorktreeRemove { .. } => {
            if selected["current"] == true {
                return Err(Error::invalid("Cannot remove the current worktree."));
            }
            args.push("remove".into());
        }
        Action::WorktreePrune { .. } => {
            if path.try_exists().map_err(io_error)? {
                return Err(Error::invalid("Only missing worktrees can be pruned."));
            }
            args.push("remove".into());
        }
        Action::WorktreeRepair { path: new_path, .. } => {
            let bytes = new_path.decode()?;
            let new_path = Path::new(OsStr::from_bytes(&bytes));
            if !new_path.is_absolute() {
                return Err(Error::invalid("Choose an absolute repair path."));
            }
            let pointer = fs::read(new_path.join(".git")).map_err(io_error)?;
            let admin = trim_line(&pointer)
                .strip_prefix(b"gitdir: ")
                .ok_or_else(|| Error::invalid("The selected path is not a linked worktree."))?;
            let target = new_path
                .join(OsStr::from_bytes(admin))
                .canonicalize()
                .map_err(io_error)?;
            if target != repo.common.join("worktrees").join(name) {
                return Err(Error::new(
                    "WORKTREE_MISMATCH",
                    "This checkout belongs to a different worktree.",
                ));
            }
            execute(
                repo,
                vec![
                    "worktree".into(),
                    "repair".into(),
                    "--".into(),
                    new_path.as_os_str().into(),
                ],
            )?;
            return Ok(json!({"refreshRequired":true}));
        }
        Action::WorktreeLock { reason, .. } => {
            args.push("lock".into());
            if let Some(reason) = reason {
                if reason.len() > 4096 || reason.contains('\0') {
                    return Err(Error::invalid("Invalid lock reason."));
                }
                args.extend(strings(&["--reason", reason]));
            }
        }
        Action::WorktreeUnlock { .. } => args.push("unlock".into()),
        _ => unreachable!(),
    }
    args.push("--".into());
    args.push(path.as_os_str().into());
    execute(repo, args)?;
    Ok(json!({"refreshRequired":true}))
}
