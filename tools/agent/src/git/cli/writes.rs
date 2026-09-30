//! CLI mutations share the existing durable journal, never the git2 executor.
use super::super::{
    journal::Journal,
    protocol::{Action, Author, CheckoutTarget, DiscardSource, ResetMode},
    tokens::{EntryRef, SnapshotRef},
};
use super::*;
pub(super) fn journal(service: &Service) -> Result<&Journal, Error> {
    service.journal.as_ref().ok_or_else(|| {
        Error::new(
            "JOURNAL_UNAVAILABLE",
            "No write can run without an operation journal.",
        )
    })
}
fn stale() -> Error {
    Error::new(
        "STALE_SNAPSHOT",
        "The repository changed. Refresh before applying this operation.",
    )
}
fn unknown() -> Error {
    Error::new("OUTCOME_UNKNOWN","Git did not confirm completion. Inspect the repository and saved outcome before repeating this operation.")
}
fn text(value: &str) -> Result<(), Error> {
    if value.is_empty() || value.len() > 4096 || value.contains(['\0', '\n', '\r']) {
        Err(Error::invalid("Invalid Git argument."))
    } else {
        Ok(())
    }
}
pub(super) fn name(repo: &Repo, value: &str, kind: &str) -> Result<String, Error> {
    text(value)?;
    if value.starts_with('-') {
        return Err(Error::invalid("Git names cannot begin with '-'."));
    }
    let reference = format!("refs/{kind}/{value}");
    if command::run(&repo.root, &["check-ref-format", &reference])?.is_none() {
        return Err(Error::invalid("Invalid reference name."));
    }
    Ok(reference)
}
fn expected(repo: &Repo, reference: &str, value: &str) -> Result<(), Error> {
    oid(value.as_bytes()).map_err(|_| Error::invalid("A complete object ID is required."))?;
    let found = command::run(
        &repo.root,
        &["rev-parse", "--verify", "--end-of-options", reference],
    )?;
    if found.as_deref().map(trim_line) != Some(value.as_bytes()) {
        return Err(Error::new(
            "STALE_REFERENCE",
            "The reference changed. Refresh before continuing.",
        ));
    }
    Ok(())
}
fn target(repo: &Repo, id: &str) -> Result<(), Error> {
    oid(id.as_bytes()).map_err(|_| Error::invalid("A complete object ID is required."))?;
    if command::run(&repo.root, &["cat-file", "-e", &format!("{id}^{{commit}}")])?.is_none() {
        return Err(Error::new("NOT_FOUND", "The commit no longer exists."));
    }
    Ok(())
}
fn remote(repo: &Repo, name: &str, token: &str) -> Result<(), Error> {
    if refs::remote(repo, name)?["token"] != token {
        return Err(Error::new(
            "STALE_REMOTE",
            "Remote configuration changed. Refresh before continuing.",
        ));
    }
    Ok(())
}
fn run(repo: &Repo, args: Vec<String>, input: Vec<u8>) -> Result<(), Error> {
    let args = args
        .iter()
        .map(std::ffi::OsString::from)
        .collect::<Vec<_>>();
    command::write(&repo.root, &args, input).map_err(|e| {
        if matches!(e.code.as_str(), "TIMEOUT" | "IO_ERROR" | "LIMIT_EXCEEDED") {
            unknown()
        } else {
            e
        }
    })
}
fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|s| s.to_string()).collect()
}
fn identity(args: &mut Vec<String>, author: Option<&Author>) -> Result<(), Error> {
    if let Some(a) = author {
        text(&a.name)?;
        text(&a.email)?;
        args.extend([
            "-c".into(),
            format!("user.name={}", a.name),
            "-c".into(),
            format!("user.email={}", a.email),
        ]);
    }
    Ok(())
}
fn message(value: &str) -> Result<(), Error> {
    if value.trim().is_empty() || value.len() > 65536 || value.contains('\0') {
        Err(Error::invalid(
            "Supply a nonempty commit message of at most 64 KiB.",
        ))
    } else {
        Ok(())
    }
}
fn paths(capture: &status::Capture, ids: &[String]) -> Result<Vec<u8>, Error> {
    if ids.is_empty() || ids.len() > 10000 {
        return Err(Error::invalid("Select between 1 and 10000 files."));
    }
    let mut result = std::collections::BTreeSet::new();
    for id in ids {
        if !capture.rows.iter().any(|r| r["entryId"] == *id) {
            return Err(stale());
        }
        for path in EntryRef::decode(id)?.paths()? {
            result.insert(path);
        }
    }
    let mut input = Vec::new();
    for path in result {
        input.extend(path);
        input.push(0);
    }
    Ok(input)
}
pub(super) fn start(
    service: &Service,
    operation_id: &str,
    repo_id: &str,
    snapshot: &str,
    action: Action,
) -> Result<Value, Error> {
    let journal = journal(service)?;
    let repo = Repo::from_id(repo_id)?;
    let metadata = fs::metadata(&repo.common).map_err(io_error)?;
    // Same lock identity as git2: both engines must serialize against each other.
    let identity = super::super::journal::hash(
        &[
            repo.common.as_os_str().as_bytes(),
            &metadata.dev().to_be_bytes(),
            &metadata.ino().to_be_bytes(),
        ]
        .concat(),
    );
    let payload = pages::hash(
        &json!({"engine":"cli","repository":identity,"worktree":WirePath::new(repo.git_dir.as_os_str().as_bytes()),"snapshot":snapshot,"action":action}),
    )?;
    if journal.existing(operation_id, &payload)?.is_some() {
        return serde_json::to_value(journal.get(operation_id)?).map_err(|_| failure());
    }
    let _lock = journal.lock_repository(&identity)?;
    if let Some(record) = journal.existing(operation_id, &payload)? {
        return serde_json::to_value(record).map_err(|_| failure());
    }
    let snapshot = SnapshotRef::decode(snapshot)?;
    if snapshot.r != repo.id
        || snapshot.q
            != if worktrees::is_action(&action) {
                "cli.worktrees"
            } else {
                "cli.status"
            }
        || snapshot.p.is_some()
    {
        return Err(stale());
    }
    let capture = if worktrees::is_action(&action) {
        let (_, fingerprint) = worktrees::rows(&repo)?;
        status::Capture {
            rows: vec![],
            fingerprint,
            metadata: Value::Null,
        }
    } else {
        status::capture(&repo)?
    };
    if capture.fingerprint != snapshot.f {
        return Err(stale());
    }
    // Do not remove another Git client's locks or begin a receipt for a known lock.
    for path in [
        repo.git_dir.join("index.lock"),
        repo.git_dir.join("HEAD.lock"),
        repo.common.join("packed-refs.lock"),
        repo.git_dir.join("FETCH_HEAD.lock"),
    ] {
        if path.try_exists().map_err(io_error)? {
            return Err(Error::new(
                "REPOSITORY_BUSY",
                "Git has an existing lock. No lock was removed.",
            ));
        }
    }
    let mut record = journal.begin(operation_id, payload, identity)?;
    match apply(&repo, &action, &capture, &snapshot.encode()) {
        Ok(value) => {
            record.state = if value["needsResolution"] == true {
                "needs_resolution"
            } else {
                "succeeded"
            }
            .into();
            record.result = Some(value);
        }
        Err(e) => {
            record.state = if e.code == "OUTCOME_UNKNOWN" {
                "outcome_unknown"
            } else {
                "failed"
            }
            .into();
            record.error = Some(e);
        }
    }
    record.seq += 1;
    journal.save(&record).map_err(|_| unknown())?;
    serde_json::to_value(record).map_err(|_| failure())
}
fn apply(
    repo: &Repo,
    action: &Action,
    capture: &status::Capture,
    snapshot: &str,
) -> Result<Value, Error> {
    let refresh = json!({"refreshRequired":true});
    if worktrees::is_action(action) {
        return worktrees::apply(repo, action);
    }
    match action {
        Action::Stage { entry_ids, hunks } | Action::Unstage { entry_ids, hunks } => {
            if let Some(selection) = hunks {
                if entry_ids.len() != 1 {
                    return Err(Error::invalid("Partial staging requires one file."));
                }
                let reverse = matches!(action, Action::Unstage { .. });
                let patch = diff::selection(
                    repo,
                    snapshot,
                    &entry_ids[0],
                    if reverse {
                        super::super::protocol::Side::HeadToIndex
                    } else {
                        super::super::protocol::Side::IndexToWorktree
                    },
                    selection,
                    reverse,
                )?;
                run(
                    repo,
                    args(&["apply", "--cached", "--recount", "--whitespace=nowarn", "-"]),
                    patch,
                )?;
                return Ok(refresh);
            }
            let input = paths(capture, entry_ids)?;
            let command = if matches!(action, Action::Stage { .. }) {
                args(&[
                    "--literal-pathspecs",
                    "add",
                    "--all",
                    "--pathspec-from-file=-",
                    "--pathspec-file-nul",
                ])
            } else if capture.metadata["head"]["unborn"] == true {
                args(&[
                    "--literal-pathspecs",
                    "rm",
                    "--cached",
                    "--ignore-unmatch",
                    "-r",
                    "--pathspec-from-file=-",
                    "--pathspec-file-nul",
                ])
            } else {
                args(&[
                    "--literal-pathspecs",
                    "restore",
                    "--staged",
                    "--pathspec-from-file=-",
                    "--pathspec-file-nul",
                ])
            };
            run(repo, command, input)?;
            Ok(refresh)
        }
        Action::Commit {
            message: msg,
            author,
        } => {
            message(msg)?;
            let mut command = Vec::new();
            identity(&mut command, author.as_ref())?;
            command.extend(args(&["commit", "--file=-"]));
            run(repo, command, msg.as_bytes().to_vec())?;
            let head = repo.head()?;
            Ok(
                json!({"commitOid":head["oid"]["hex"],"parentOid":capture.metadata["head"]["oid"]["hex"],"refreshRequired":true}),
            )
        }
        Action::Amend {
            expected_oid,
            message: msg,
            committer,
            author,
        } => {
            expected(repo, "HEAD", expected_oid)?;
            message(msg)?;
            let mut command = Vec::new();
            identity(&mut command, committer.as_ref())?;
            command.extend(args(&["commit", "--amend", "--file=-"]));
            if let Some(a) = author {
                text(&a.name)?;
                text(&a.email)?;
                command.push(format!("--author={} <{}>", a.name, a.email));
            }
            run(repo, command, msg.as_bytes().to_vec())?;
            Ok(
                json!({"amended":true,"replacedOid":expected_oid,"commitOid":repo.head()?["oid"]["hex"],"refreshRequired":true}),
            )
        }
        Action::BranchCreate { name: n, start_oid } => {
            name(repo, n, "heads")?;
            target(repo, start_oid)?;
            run(repo, args(&["branch", n, start_oid]), vec![])?;
            Ok(refresh)
        }
        Action::BranchRename {
            name: n,
            new_name,
            expected_oid,
        } => {
            let reference = name(repo, n, "heads")?;
            name(repo, new_name, "heads")?;
            expected(repo, &reference, expected_oid)?;
            run(repo, args(&["branch", "-m", n, new_name]), vec![])?;
            Ok(refresh)
        }
        Action::BranchDelete {
            name: n,
            expected_oid,
            force,
        } => {
            let reference = name(repo, n, "heads")?;
            expected(repo, &reference, expected_oid)?;
            run(
                repo,
                args(&["branch", if *force { "-D" } else { "-d" }, n]),
                vec![],
            )?;
            Ok(refresh)
        }
        Action::BranchSetUpstream {
            name: n,
            expected_oid,
            expected_token,
            upstream,
        } => {
            let reference = name(repo, n, "heads")?;
            expected(repo, &reference, expected_oid)?;
            let tracking = refs::tracking(repo, n)?;
            if tracking["token"] != *expected_token {
                return Err(Error::new(
                    "STALE_REFERENCE",
                    "Upstream configuration changed.",
                ));
            }
            if tracking["editable"] != true {
                return Err(Error::new(
                    "UNSUPPORTED_CONFIGURATION",
                    "Inherited upstream configuration is not editable.",
                ));
            }
            let command = if let Some(upstream) = upstream {
                text(upstream)?;
                args(&["branch", &format!("--set-upstream-to={upstream}"), n])
            } else {
                args(&["branch", "--unset-upstream", n])
            };
            run(repo, command, vec![])?;
            Ok(refresh)
        }
        Action::Checkout { target: checkout } => {
            match checkout {
                CheckoutTarget::Branch {
                    name: n,
                    expected_oid,
                } => {
                    let reference = name(repo, n, "heads")?;
                    expected(repo, &reference, expected_oid)?;
                    run(repo, args(&["switch", "--no-guess", n]), vec![])?;
                }
                CheckoutTarget::Detached { oid } => {
                    target(repo, oid)?;
                    run(repo, args(&["switch", "--detach", oid]), vec![])?;
                }
            }
            Ok(refresh)
        }
        Action::RemoteAdd { name: n, url } => {
            name(repo, n, "remotes")?;
            super::super::remotes::validate_url(url)?;
            run(repo, args(&["remote", "add", "--", n, url]), vec![])?;
            Ok(refresh)
        }
        Action::RemoteRename {
            name: n,
            new_name,
            expected_token,
        } => {
            remote(repo, n, expected_token)?;
            name(repo, new_name, "remotes")?;
            run(repo, args(&["remote", "rename", "--", n, new_name]), vec![])?;
            Ok(refresh)
        }
        Action::RemoteSetUrl {
            name: n,
            url,
            expected_token,
        } => {
            remote(repo, n, expected_token)?;
            super::super::remotes::validate_url(url)?;
            run(repo, args(&["remote", "set-url", "--", n, url]), vec![])?;
            Ok(refresh)
        }
        Action::RemoteRemove {
            name: n,
            expected_token,
        } => {
            remote(repo, n, expected_token)?;
            run(repo, args(&["remote", "remove", "--", n]), vec![])?;
            Ok(refresh)
        }
        Action::Fetch {
            remote: n,
            expected_token,
            prune,
        } => {
            remote(repo, n, expected_token)?;
            let mut command = args(&["fetch", "--no-recurse-submodules"]);
            if *prune {
                command.push("--prune".into());
            }
            command.extend(args(&["--", n]));
            run(repo, command, vec![])?;
            Ok(refresh)
        }
        Action::PullFastForward {
            remote: n,
            expected_token,
            remote_branch,
        } => {
            remote(repo, n, expected_token)?;
            let reference = name(repo, remote_branch, "heads")?;
            run(
                repo,
                args(&["fetch", "--no-recurse-submodules", "--", n, &reference]),
                vec![],
            )?;
            run(
                repo,
                args(&["merge", "--ff-only", "--no-edit", "FETCH_HEAD"]),
                vec![],
            )?;
            Ok(refresh)
        }
        Action::FastForward { target_oid } => {
            target(repo, target_oid)?;
            run(
                repo,
                args(&["merge", "--ff-only", "--no-edit", target_oid]),
                vec![],
            )?;
            Ok(refresh)
        }
        Action::Push {
            remote: n,
            expected_token,
            branch,
            expected_oid,
            destination_branch,
        } => {
            remote(repo, n, expected_token)?;
            let reference = name(repo, branch, "heads")?;
            expected(repo, &reference, expected_oid)?;
            let destination = name(repo, destination_branch, "heads")?;
            run(
                repo,
                args(&[
                    "push",
                    "--porcelain",
                    "--",
                    n,
                    &format!("{expected_oid}:{destination}"),
                ]),
                vec![],
            )?;
            Ok(refresh)
        }
        Action::PushWithLease {
            remote: n,
            expected_token,
            branch,
            expected_oid,
            destination_branch,
            expected_remote_oid,
        } => {
            remote(repo, n, expected_token)?;
            let reference = name(repo, branch, "heads")?;
            expected(repo, &reference, expected_oid)?;
            oid(expected_remote_oid.as_bytes())?;
            let destination = name(repo, destination_branch, "heads")?;
            run(
                repo,
                args(&[
                    "push",
                    "--porcelain",
                    &format!("--force-with-lease={destination}:{expected_remote_oid}"),
                    "--",
                    n,
                    &format!("{expected_oid}:{destination}"),
                ]),
                vec![],
            )?;
            Ok(refresh)
        }
        Action::BranchDeleteRemote {
            remote: n,
            expected_token,
            branch,
            expected_oid,
        }
        | Action::TagDeleteRemote {
            remote: n,
            expected_token,
            name: branch,
            expected_oid,
        } => {
            remote(repo, n, expected_token)?;
            oid(expected_oid.as_bytes())?;
            let reference = name(
                repo,
                branch,
                if matches!(action, Action::TagDeleteRemote { .. }) {
                    "tags"
                } else {
                    "heads"
                },
            )?;
            run(
                repo,
                args(&[
                    "push",
                    "--porcelain",
                    &format!("--force-with-lease={reference}:{expected_oid}"),
                    "--",
                    n,
                    &format!(":{reference}"),
                ]),
                vec![],
            )?;
            Ok(refresh)
        }
        Action::TagCreate {
            name: n,
            target_oid,
            annotation,
        } => {
            name(repo, n, "tags")?;
            oid(target_oid.as_bytes())?;
            let mut command = Vec::new();
            if let Some(a) = annotation {
                message(&a.message)?;
                identity(&mut command, a.author.as_ref())?;
                command.extend(args(&["tag", "-a", n, target_oid, "--file=-"]));
                run(repo, command, a.message.as_bytes().to_vec())?;
            } else {
                run(repo, args(&["tag", n, target_oid]), vec![])?;
            }
            Ok(refresh)
        }
        Action::TagDelete {
            name: n,
            expected_oid,
        } => {
            let reference = name(repo, n, "tags")?;
            expected(repo, &reference, expected_oid)?;
            run(
                repo,
                args(&["update-ref", "-d", &reference, expected_oid]),
                vec![],
            )?;
            Ok(refresh)
        }
        Action::TagPush {
            remote: n,
            expected_token,
            name: tag,
            expected_oid,
        } => {
            remote(repo, n, expected_token)?;
            let reference = name(repo, tag, "tags")?;
            expected(repo, &reference, expected_oid)?;
            run(
                repo,
                args(&[
                    "push",
                    "--porcelain",
                    "--",
                    n,
                    &format!("{expected_oid}:{reference}"),
                ]),
                vec![],
            )?;
            Ok(refresh)
        }
        Action::Reset {
            target_oid,
            expected_oid,
            mode,
        } => {
            expected(repo, "HEAD", expected_oid)?;
            target(repo, target_oid)?;
            run(
                repo,
                args(&[
                    "reset",
                    match mode {
                        ResetMode::Soft => "--soft",
                        ResetMode::Mixed => "--mixed",
                        ResetMode::Hard => "--hard",
                    },
                    target_oid,
                ]),
                vec![],
            )?;
            Ok(refresh)
        }
        Action::Discard {
            entry_ids,
            source,
            hunks,
        } => {
            if let Some(selection) = hunks {
                if entry_ids.len() != 1 {
                    return Err(Error::invalid("Partial discard requires one file."));
                }
                let patch = diff::selection(
                    repo,
                    snapshot,
                    &entry_ids[0],
                    if *source == DiscardSource::Head {
                        super::super::protocol::Side::HeadToWorktree
                    } else {
                        super::super::protocol::Side::IndexToWorktree
                    },
                    selection,
                    true,
                )?;
                run(
                    repo,
                    args(&["apply", "--recount", "--whitespace=nowarn", "-"]),
                    patch,
                )?;
                return Ok(refresh);
            }
            if entry_ids.iter().any(|id| {
                capture.rows.iter().any(|r| {
                    r["entryId"] == *id && (r["untracked"] == true || r["conflicted"] == true)
                })
            }) {
                return Err(Error::new(
                    "UNSUPPORTED_CAPABILITY",
                    "Discard requires tracked, resolved files.",
                ));
            }
            let input = paths(capture, entry_ids)?;
            let mut command = args(&[
                "--literal-pathspecs",
                "restore",
                "--worktree",
                "--pathspec-from-file=-",
                "--pathspec-file-nul",
            ]);
            if *source == DiscardSource::Head {
                command.push("--source=HEAD".into());
            }
            run(repo, command, input)?;
            Ok(refresh)
        }
        Action::StashSave {
            message: msg,
            include_untracked,
            keep_index,
            author,
        } => {
            if msg.len() > 65536 || msg.contains('\0') {
                return Err(Error::invalid("Invalid stash message."));
            }
            let mut command = Vec::new();
            identity(&mut command, author.as_ref())?;
            command.extend(args(&["stash", "push", "--message", msg]));
            if *include_untracked {
                command.push("--include-untracked".into());
            }
            if *keep_index {
                command.push("--keep-index".into());
            }
            if capture.rows.is_empty() {
                return Err(Error::new(
                    "NOTHING_TO_STASH",
                    "There are no changes to stash.",
                ));
            }
            run(repo, command, vec![])?;
            let id = repo.run(&["rev-parse", "refs/stash"])?;
            Ok(
                json!({"oid":String::from_utf8_lossy(trim_line(&id)),"saved":true,"refreshRequired":true}),
            )
        }
        Action::StashApply {
            oid,
            index,
            expected_token,
            reinstate_index,
        }
        | Action::StashPop {
            oid,
            index,
            expected_token,
            reinstate_index,
        } => {
            let selected = stash_selection(repo, oid, *index, expected_token)?;
            let pop = matches!(action, Action::StashPop { .. });
            let mut command = args(&["stash", if pop { "pop" } else { "apply" }]);
            if *reinstate_index {
                command.push("--index".into());
            }
            command.push(selected);
            let result = run(repo, command, vec![]);
            let conflicts = repo.summary()?["groupCounts"]["conflicted"]
                .as_u64()
                .unwrap_or(0)
                > 0;
            if !conflicts {
                result?;
            }
            Ok(
                json!({"oid":oid,"applied":true,"dropped":pop&&!conflicts,"needsResolution":conflicts,"refreshRequired":true}),
            )
        }
        Action::StashDrop {
            oid,
            index,
            expected_token,
        } => {
            let selected = stash_selection(repo, oid, *index, expected_token)?;
            run(repo, args(&["stash", "drop", &selected]), vec![])?;
            Ok(json!({"oid":oid,"dropped":true,"refreshRequired":true}))
        }
        Action::Merge { target_oid } => {
            target(repo, target_oid)?;
            integrate(repo, args(&["merge", "--no-edit", target_oid]), vec![])
        }
        Action::CherryPick {
            target_oid,
            mainline,
            author,
        }
        | Action::Revert {
            target_oid,
            mainline,
            author,
        } => {
            target(repo, target_oid)?;
            let mut command = Vec::new();
            identity(&mut command, author.as_ref())?;
            command.push(
                if matches!(action, Action::Revert { .. }) {
                    "revert"
                } else {
                    "cherry-pick"
                }
                .into(),
            );
            command.push("--no-edit".into());
            if *mainline > 0 {
                command.extend(["-m".into(), mainline.to_string()]);
            }
            command.push(target_oid.clone());
            integrate(repo, command, vec![])
        }
        Action::Rebase {
            upstream_oid,
            onto_oid,
            committer,
        } => {
            target(repo, upstream_oid)?;
            let mut command = Vec::new();
            identity(&mut command, committer.as_ref())?;
            command.extend(args(&["rebase", "--no-autostash"]));
            if let Some(onto) = onto_oid {
                target(repo, onto)?;
                command.extend(args(&["--onto", onto]));
            }
            command.push(upstream_oid.clone());
            integrate(repo, command, vec![])
        }
        Action::MergeAbort {}
        | Action::IntegrationAbort {}
        | Action::IntegrationSkip {}
        | Action::IntegrationContinue { .. } => {
            let info = repo.integration()?;
            if info["managed"] != true {
                return Err(Error::new(
                    "UNMANAGED_INTEGRATION",
                    "This integration was not started by the CLI backend.",
                ));
            }
            let kind = match info["kind"].as_str() {
                Some("merge") => "merge",
                Some("rebase") => "rebase",
                Some("cherry_pick") => "cherry-pick",
                Some("revert") => "revert",
                _ => {
                    return Err(Error::new(
                        "NO_INTEGRATION",
                        "There is no integration to continue.",
                    ))
                }
            };
            let mut command = Vec::new();
            let mut input = vec![];
            match action {
                Action::IntegrationContinue {
                    message: msg,
                    author,
                } => {
                    identity(&mut command, author.as_ref())?;
                    if kind == "merge" {
                        command.push("commit".into());
                        if let Some(msg) = msg {
                            message(msg)?;
                            command.push("--file=-".into());
                            input = msg.as_bytes().to_vec();
                        } else {
                            command.push("--no-edit".into());
                        }
                    } else {
                        command.extend(args(&[kind, "--continue"]));
                    }
                }
                Action::IntegrationSkip {} => {
                    if kind == "merge" {
                        return Err(Error::invalid("Merge cannot be skipped."));
                    }
                    command.extend(args(&[kind, "--skip"]));
                }
                _ => command.extend(args(&[kind, "--abort"])),
            }
            finish_integration(repo, run(repo, command, input))
        }
        Action::ConflictResolve {
            entry_ids,
            side,
            expected_oid,
        } => {
            if entry_ids.len() != 1 {
                return Err(Error::invalid("Resolve one conflict at a time."));
            }
            let row = capture
                .rows
                .iter()
                .find(|r| r["entryId"] == entry_ids[0] && r["conflicted"] == true)
                .ok_or_else(stale)?;
            let side = match side {
                super::super::protocol::ConflictSide::Base => "base",
                super::super::protocol::ConflictSide::Ours => "ours",
                super::super::protocol::ConflictSide::Theirs => "theirs",
            };
            let selected = &row["conflict"][side];
            if selected["oid"]["hex"].as_str() != expected_oid.as_deref() {
                return Err(stale());
            }
            let input = paths(capture, entry_ids)?;
            if selected.is_null() {
                run(
                    repo,
                    args(&[
                        "--literal-pathspecs",
                        "rm",
                        "-f",
                        "--ignore-unmatch",
                        "--pathspec-from-file=-",
                        "--pathspec-file-nul",
                    ]),
                    input,
                )?;
                return Ok(refresh);
            }
            let stage = match side {
                "base" => "--stage=1",
                "ours" => "--stage=2",
                _ => "--stage=3",
            };
            run(
                repo,
                args(&["checkout-index", "--force", stage, "-z", "--stdin"]),
                input.clone(),
            )?;
            run(
                repo,
                args(&[
                    "--literal-pathspecs",
                    "add",
                    "--pathspec-from-file=-",
                    "--pathspec-file-nul",
                ]),
                input,
            )?;
            Ok(refresh)
        }
        Action::WorktreeAdd { .. }
        | Action::WorktreeRemove { .. }
        | Action::WorktreeRepair { .. }
        | Action::WorktreePrune { .. }
        | Action::WorktreeLock { .. }
        | Action::WorktreeUnlock { .. } => {
            unreachable!("worktree actions dispatched before the index workflow")
        }
    }
}

fn stash_selection(
    repo: &Repo,
    id: &str,
    index: Option<usize>,
    token: &str,
) -> Result<String, Error> {
    let (rows, current) = stashes::rows(repo)?;
    if current != token {
        return Err(Error::new("STALE_STASH", "The stash list changed."));
    }
    let matches = rows
        .iter()
        .enumerate()
        .filter(|(i, r)| r["oid"] == id && index.is_none_or(|wanted| wanted == *i))
        .map(|(i, _)| i)
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err(Error::new("STALE_STASH", "Select one current stash entry."));
    }
    Ok(format!("stash@{{{}}}", matches[0]))
}
fn integrate(repo: &Repo, command: Vec<String>, input: Vec<u8>) -> Result<Value, Error> {
    if repo.state()? != "Clean" {
        return Err(Error::new(
            "INTEGRATION_IN_PROGRESS",
            "Finish or abort the active integration first.",
        ));
    }
    fs::write(repo.git_dir.join("newport-cli-integration"), b"1\n").map_err(io_error)?;
    finish_integration(repo, run(repo, command, input))
}
fn finish_integration(repo: &Repo, result: Result<(), Error>) -> Result<Value, Error> {
    if repo.state()? != "Clean" {
        if repo.integration()?["managed"] == true {
            return Ok(json!({"needsResolution":true,"refreshRequired":true}));
        }
        return result.map(|_| json!({"refreshRequired":true}));
    }
    let _ = fs::remove_file(repo.git_dir.join("newport-cli-integration"));
    result?;
    Ok(json!({"needsResolution":false,"refreshRequired":true}))
}
