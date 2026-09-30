//! Independent Git CLI adapter. No fallback into the libgit2 adapter.
//! Engine-neutral RPC envelopes, bounded pages, and the shared operation
//! journal surround Git processes. The desktop defaults to git2.
mod command;
mod create;
mod diff;
mod history;
mod objects;
mod pages;
mod refs;
mod stashes;
mod status;
mod tags;
mod worktrees;
mod writes;
use super::{
    backend::Output,
    protocol::{Error, Path as WirePath, Request},
    tokens::RepoRef,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsStr,
    fs,
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::{Path, PathBuf},
};

pub(super) const METHODS: &[&str] = &[
    "repo.open",
    "repo.init",
    "repo.clone",
    "repo.remote_refs",
    "repo.close",
    "repo.status_summary",
    "repo.status",
    "repo.history",
    "repo.commit",
    "repo.blob",
    "repo.blob_page",
    "repo.branches",
    "repo.remotes",
    "repo.remote",
    "repo.remote_names",
    "repo.tags",
    "repo.tag",
    "repo.stashes",
    "repo.worktrees",
    "repo.diff",
    "repo.diff_page",
    "repo.commit_diff",
    "repo.commit_diff_page",
    "repo.commit_files",
    "operation.start",
    "operation.get",
    "operation.review",
];
pub(super) const FEATURES: &[&str] = &[
    "status_summary.path",
    "status.filter",
    "remote_refs.filter",
    "branches.filter",
    "tags.summary",
    "stash.entry_index",
    "worktrees.filter",
    "worktree.new_branch",
    "worktrees.snapshot_filter",
    "paths.bytes",
    "history.snapshot_pagination",
    "history.summary",
    "diff.streaming",
    "diff.tuple_v1",
    "index.hunks",
    "index.lines",
    "discard.hunks",
];
#[derive(Default)]
pub(super) struct Service {
    journal: Option<super::journal::Journal>,
}
impl Service {
    pub(super) fn with_journal(journal: Option<super::journal::Journal>) -> Self {
        Self { journal }
    }
    pub(super) fn writable(&self) -> bool {
        self.journal.is_some()
    }
}
pub(super) const ACTIONS: &[&str] = &[
    "stage",
    "unstage",
    "commit",
    "commit.amend",
    "branch.create",
    "branch.rename",
    "branch.delete",
    "branch.set_upstream",
    "checkout",
    "remote.add",
    "remote.rename",
    "remote.set_url",
    "remote.remove",
    "fetch",
    "pull.fast_forward",
    "push",
    "push.with_lease",
    "branch.delete_remote",
    "tag.create",
    "tag.delete",
    "tag.push",
    "tag.delete_remote",
    "merge.fast_forward",
    "reset",
    "discard",
    "stash.save",
    "stash.apply",
    "stash.pop",
    "stash.drop",
    "merge",
    "merge.abort",
    "cherry_pick",
    "revert",
    "rebase",
    "integration.continue",
    "integration.abort",
    "integration.skip",
    "conflict.resolve",
    "worktree.add",
    "worktree.remove",
    "worktree.repair",
    "worktree.prune",
    "worktree.lock",
    "worktree.unlock",
];
fn failure() -> Error {
    Error::new("GIT_ERROR", "The Git CLI could not read this repository.")
}
fn io_error(_: std::io::Error) -> Error {
    Error::new("IO_ERROR", "A repository file could not be read.")
}
fn trim_line(value: &[u8]) -> &[u8] {
    value.strip_suffix(b"\n").unwrap_or(value)
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn oid(bytes: &[u8]) -> Result<Value, Error> {
    if ![40, 64].contains(&bytes.len()) || !bytes.iter().all(u8::is_ascii_hexdigit) {
        return Err(failure());
    }
    Ok(
        json!({"algorithm":if bytes.len()==40 {"sha1"} else {"sha256"},"hex":String::from_utf8_lossy(bytes)}),
    )
}
struct Repo {
    root: PathBuf,
    git_dir: PathBuf,
    common: PathBuf,
    id: RepoRef,
    bare: bool,
}
impl Repo {
    fn discover(path: WirePath) -> Result<Self, Error> {
        let bytes = path.decode()?;
        let root = Path::new(OsStr::from_bytes(&bytes));
        if !root.is_absolute() {
            return Err(Error::invalid("An absolute repository path is required."));
        }
        let git_dir =
            command::run(root, &["rev-parse", "--absolute-git-dir"])?.ok_or_else(|| {
                Error::new("REPO_NOT_FOUND", "The repository is no longer available.")
            })?;
        let git_dir = Path::new(OsStr::from_bytes(trim_line(&git_dir)))
            .canonicalize()
            .map_err(io_error)?;
        let bare = command::run(root, &["rev-parse", "--is-bare-repository"])?
            .ok_or_else(failure)?
            == b"true\n";
        let top = if bare {
            git_dir.clone()
        } else {
            let value =
                command::run(root, &["rev-parse", "--show-toplevel"])?.ok_or_else(failure)?;
            Path::new(OsStr::from_bytes(trim_line(&value)))
                .canonicalize()
                .map_err(io_error)?
        };
        let common = match fs::read(git_dir.join("commondir")) {
            Ok(data) => git_dir
                .join(OsStr::from_bytes(trim_line(&data)))
                .canonicalize()
                .map_err(io_error)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => git_dir.clone(),
            Err(e) => return Err(io_error(e)),
        };
        let metadata = fs::metadata(&git_dir).map_err(io_error)?;
        let id = RepoRef::new(&git_dir, metadata.dev(), metadata.ino());
        Ok(Self {
            root: top,
            git_dir,
            common,
            id,
            bare,
        })
    }
    fn from_id(value: &str) -> Result<Self, Error> {
        let id = RepoRef::decode(value)?;
        let dir = id.checked_path()?;
        let root = if dir.file_name() == Some(OsStr::new(".git")) {
            dir.parent().ok_or_else(failure)?.to_owned()
        } else if dir.join("gitdir").is_file() {
            let data = fs::read(dir.join("gitdir")).map_err(io_error)?;
            Path::new(OsStr::from_bytes(trim_line(&data)))
                .parent()
                .ok_or_else(failure)?
                .to_owned()
        } else {
            // Bare and separate-git-dir repositories may define core.worktree.
            match command::run(&dir, &["config", "--path", "--get", "core.worktree"])? {
                Some(value) => dir.join(OsStr::from_bytes(trim_line(&value))),
                None => dir.clone(),
            }
        };
        let repo = Self::discover(WirePath::new(root.as_os_str().as_bytes()))?;
        if repo.id != id {
            return Err(Error::new(
                "REPO_REPLACED",
                "The repository directory was replaced.",
            ));
        }
        Ok(repo)
    }
    fn run(&self, args: &[&str]) -> Result<Vec<u8>, Error> {
        command::run(&self.root, args)?.ok_or_else(failure)
    }
    fn head(&self) -> Result<Value, Error> {
        let reference = command::run(&self.root, &["symbolic-ref", "-q", "HEAD"])?;
        let target = command::run(&self.root, &["rev-parse", "--verify", "HEAD"])?;
        if target.is_none() && reference.is_none() {
            return Err(failure());
        }
        Ok(
            json!({"oid":target.as_deref().map(trim_line).map(oid).transpose()?,
            "name":reference.as_deref().map(trim_line).map(WirePath::new).or_else(||target.as_ref().map(|_|WirePath::new(b"HEAD"))),
            "detached":reference.is_none(),"unborn":target.is_none()}),
        )
    }
    fn state(&self) -> Result<&'static str, Error> {
        for (marker, state) in [
            ("rebase-merge", "RebaseMerge"),
            ("rebase-apply", "Rebase"),
            ("MERGE_HEAD", "Merge"),
            ("CHERRY_PICK_HEAD", "CherryPick"),
            ("REVERT_HEAD", "Revert"),
            ("BISECT_LOG", "Bisect"),
        ] {
            if self.git_dir.join(marker).try_exists().map_err(io_error)? {
                return Ok(state);
            }
        }
        Ok("Clean")
    }
    fn integration(&self) -> Result<Value, Error> {
        let kind = match self.state()? {
            "Merge" => "merge",
            "CherryPick" => "cherry_pick",
            "Revert" => "revert",
            "Rebase" | "RebaseMerge" => "rebase",
            _ => return Ok(Value::Null),
        };
        let managed = self
            .git_dir
            .join("newport-cli-integration")
            .try_exists()
            .map_err(io_error)?;
        Ok(
            json!({"kind":kind,"managed":managed,"canContinue":managed,"canAbort":managed,"canSkip":managed&&kind!="merge"}),
        )
    }
    fn open(&self) -> Result<Value, Error> {
        let format = self.run(&["rev-parse", "--show-object-format"])?;
        Ok(
            json!({"repoId":self.id.encode(),"commonRepoId":hex(&Sha256::digest(self.common.as_os_str().as_bytes())),"root":WirePath::new(self.root.as_os_str().as_bytes()),"bare":self.bare,"objectFormat":String::from_utf8_lossy(trim_line(&format)),"head":self.head()?,"operationState":self.state()?,"integration":self.integration()?,"capabilities":{"readOnly":true,"workingTree":!self.bare}}),
        )
    }
    fn summary(&self) -> Result<Value, Error> {
        if self.bare {
            return Err(Error::new(
                "UNSUPPORTED_CAPABILITY",
                "Bare repositories have no working tree.",
            ));
        }
        let state = self.state()?;
        let bytes = self.run(&[
            "-c",
            "status.renames=true",
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=normal",
            "--ignore-submodules=none",
        ])?;
        let mut records = bytes.split(|b| *b == 0).filter(|r| !r.is_empty());
        let (mut total, mut staged, mut unstaged, mut untracked, mut conflicted) = (0, 0, 0, 0, 0);
        while let Some(row) = records.next() {
            if row.len() < 4 || row[2] != b' ' {
                return Err(failure());
            }
            total += 1;
            let xy = &row[..2];
            if [b"DD", b"AU", b"UD", b"UA", b"DU", b"AA", b"UU"]
                .contains(&xy.try_into().map_err(|_| failure())?)
            {
                conflicted += 1;
            } else if xy == b"??" {
                untracked += 1;
            } else {
                staged += usize::from(b"MADRTC".contains(&xy[0]));
                unstaged += usize::from(b"MDRTC".contains(&xy[1]));
            }
            if xy.contains(&b'R') || xy.contains(&b'C') {
                records.next().ok_or_else(failure)?;
            }
        }
        let upstream = command::run(
            &self.root,
            &[
                "rev-parse",
                "--symbolic-full-name",
                "--verify",
                "@{upstream}",
            ],
        )?;
        let counts = if upstream.is_some() {
            command::run(
                &self.root,
                &["rev-list", "--left-right", "--count", "HEAD...@{upstream}"],
            )?
            .map(|v| {
                String::from_utf8_lossy(&v)
                    .split_whitespace()
                    .map(str::parse::<usize>)
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()
            .map_err(|_| failure())?
        } else {
            None
        };
        Ok(
            json!({"head":self.head()?,"operationState":state,"integration":self.integration()?,"ahead":counts.as_ref().and_then(|v|v.first()),"behind":counts.as_ref().and_then(|v|v.get(1)),"basis":"stored_refs","upstreamRef":upstream.as_deref().map(trim_line).map(WirePath::new),"truncated":false,"totalEntries":total,"groupCounts":{"staged":staged,"unstaged":unstaged,"untracked":untracked,"conflicted":conflicted}}),
        )
    }
}
impl Service {
    pub(super) fn request(&mut self, request: Request) -> Result<Output, Error> {
        let result = match request {
            Request::Open { path } => {
                let mut result = Repo::discover(path)?.open()?;
                result["capabilities"]["readOnly"] = json!(!self.writable());
                Ok(result)
            }
            Request::Init {
                operation_id,
                path,
                initial_branch,
            } => create::create(self, &operation_id, path, Some(initial_branch), None),
            Request::Clone {
                operation_id,
                url,
                path,
                branch,
                bare,
            } => create::create(self, &operation_id, path, branch, Some((url, bare))),
            Request::RemoteRefs {
                repo_id,
                remote,
                expected_token,
                for_push,
                filter,
                page_size,
                cursor,
            } => refs::remote_refs(
                &Repo::from_id(&repo_id)?,
                &remote,
                &expected_token,
                for_push,
                filter,
                page_size,
                cursor,
            ),
            Request::Close { repo_id } => {
                RepoRef::decode(&repo_id)?;
                Ok(json!({"closed":true}))
            }
            Request::StatusSummary { repo_id, path } => {
                let repo = match (repo_id, path) {
                    (Some(id), None) => Repo::from_id(&id)?,
                    (None, Some(path)) => Repo::discover(path)?,
                    _ => return Err(Error::invalid("Specify exactly one of repoId or path.")),
                };
                repo.summary()
            }
            Request::Start {
                operation_id,
                repo_id,
                expected_snapshot,
                action,
            } => writes::start(self, &operation_id, &repo_id, &expected_snapshot, action),
            Request::Get { operation_id } => {
                serde_json::to_value(writes::journal(self)?.get(&operation_id)?)
                    .map_err(|_| failure())
            }
            Request::Review { operation_id } => {
                serde_json::to_value(writes::journal(self)?.review(&operation_id)?)
                    .map_err(|_| failure())
            }
            Request::CommitFiles {
                repo_id,
                commit_oid,
                parent_index,
                page_size,
                cursor,
            } => diff::commit_files(
                &Repo::from_id(&repo_id)?,
                &commit_oid,
                parent_index,
                page_size,
                cursor,
            ),
            Request::CommitDiff {
                repo_id,
                commit_oid,
                parent_index,
                path,
                context_lines,
            } => {
                let value = diff::history(
                    &Repo::from_id(&repo_id)?,
                    &commit_oid,
                    parent_index,
                    path,
                    context_lines,
                )?;
                let bytes = serde_json::to_vec(&value).map_err(|_| failure())?;
                if bytes.len() > super::protocol::MAX_DIFF {
                    return Err(Error::new(
                        "LIMIT_EXCEEDED",
                        "Diff exceeds the stream limit. Use paginated reads.",
                    ));
                }
                return Ok(Output::Diff {
                    snapshot: commit_oid,
                    bytes,
                });
            }
            Request::Diff {
                repo_id,
                snapshot,
                entry_id,
                side,
                context_lines,
            } => {
                let value = diff::working(
                    &Repo::from_id(&repo_id)?,
                    &snapshot,
                    &entry_id,
                    side,
                    context_lines,
                )?;
                let bytes = serde_json::to_vec(&value).map_err(|_| failure())?;
                if bytes.len() > super::protocol::MAX_DIFF {
                    return Err(Error::new(
                        "LIMIT_EXCEEDED",
                        "Diff exceeds the stream limit. Use paginated reads.",
                    ));
                }
                return Ok(Output::Diff { snapshot, bytes });
            }
            Request::CommitDiffPage {
                repo_id,
                commit_oid,
                parent_index,
                path,
                context_lines,
                page_size,
                max_bytes,
                cursor,
                line_encoding,
            } => {
                let repo = Repo::from_id(&repo_id)?;
                let query = format!(
                    "cli.commit_diff:{}",
                    json!([commit_oid, parent_index, path, context_lines, line_encoding])
                );
                let value =
                    diff::history(&repo, &commit_oid, parent_index, Some(path), context_lines)?;
                diff::page(
                    &repo,
                    value,
                    query,
                    page_size,
                    max_bytes,
                    cursor,
                    line_encoding,
                )
            }
            Request::DiffPage {
                repo_id,
                snapshot,
                entry_id,
                side,
                context_lines,
                page_size,
                max_bytes,
                cursor,
                line_encoding,
            } => {
                let repo = Repo::from_id(&repo_id)?;
                let query = format!(
                    "cli.diff:{}",
                    json!([snapshot, entry_id, side, context_lines, line_encoding])
                );
                let value = diff::working(&repo, &snapshot, &entry_id, side, context_lines)?;
                diff::page(
                    &repo,
                    value,
                    query,
                    page_size,
                    max_bytes,
                    cursor,
                    line_encoding,
                )
            }
            Request::Worktrees {
                repo_id,
                page_size,
                cursor,
                filter,
                branch,
                name,
                at_snapshot,
            } => worktrees::page(
                &Repo::from_id(&repo_id)?,
                page_size,
                cursor,
                filter,
                branch,
                name,
                at_snapshot,
            ),
            Request::Tags {
                repo_id,
                page_size,
                cursor,
                message_bytes,
            } => tags::page(&Repo::from_id(&repo_id)?, page_size, cursor, message_bytes),
            Request::Tag { repo_id, oid } => tags::detail(&Repo::from_id(&repo_id)?, &oid, 16384),
            Request::Stashes {
                repo_id,
                page_size,
                cursor,
            } => stashes::page(&Repo::from_id(&repo_id)?, page_size, cursor),
            Request::Branches {
                repo_id,
                page_size,
                cursor,
                filter,
                branch_kind,
            } => refs::branches(
                &Repo::from_id(&repo_id)?,
                filter,
                branch_kind,
                page_size,
                cursor,
            ),
            Request::Remotes { repo_id } => refs::remotes(&Repo::from_id(&repo_id)?),
            Request::Remote { repo_id, name } => refs::remote(&Repo::from_id(&repo_id)?, &name),
            Request::RemoteNames {
                repo_id,
                page_size,
                cursor,
                filter,
            } => refs::remote_names(&Repo::from_id(&repo_id)?, filter, page_size, cursor),
            Request::Blob { repo_id, oid } => {
                objects::blob(&Repo::from_id(&repo_id)?, &oid, None, None, false)
            }
            Request::BlobPage {
                repo_id,
                oid,
                max_bytes,
                cursor,
            } => objects::blob(&Repo::from_id(&repo_id)?, &oid, max_bytes, cursor, true),
            Request::Status {
                repo_id,
                filter,
                page_size,
                cursor,
            } => status::page(&Repo::from_id(&repo_id)?, filter, page_size, cursor),
            Request::History {
                repo_id,
                page_size,
                cursor,
                revision,
                message_bytes,
            } => history::page(
                Repo::from_id(&repo_id)?,
                page_size,
                cursor,
                revision,
                message_bytes,
            ),
            Request::Commit {
                repo_id,
                commit_oid,
            } => history::commit(&Repo::from_id(&repo_id)?, &commit_oid, 16384),
        }?;
        Ok(Output::Json(result))
    }
}

#[cfg(test)]
mod tests;

/// Historical attributes must come from the pinned tree, not today's worktree.
pub(super) fn check_runtime() -> Result<(), Error> {
    let root = std::env::current_dir().map_err(io_error)?;
    if command::run(&root, &["--attr-source=HEAD", "--version"])?.is_none() {
        return Err(Error::new(
            "UNSUPPORTED_GIT_VERSION",
            "The CLI backend requires Git with --attr-source support.",
        ));
    }
    Ok(())
}
