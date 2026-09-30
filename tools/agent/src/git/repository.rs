//! Read-only libgit2 adapter.
//!
//! Stateless: repository ids, snapshots, entry ids and cursors are
//! self-describing tokens (see `tokens`), so no request depends on anything an
//! earlier request left in this process. The only state kept here is a small
//! cache of recently computed listings, keyed by their complete snapshot token;
//! a page found there is served exactly as the captured listing was, and a miss
//! recomputes the listing and serves it only if its fingerprint still matches.
use super::protocol::{Action, Error, Path as WirePath, Request, Side, MAX_DIFF, MAX_FRAME};
use super::{
    journal::Journal,
    operations,
    tokens::{CursorRef, EntryRef, RepoRef, SnapshotRef},
};
use base64::{engine::general_purpose::STANDARD, Engine};
use git2::{DiffOptions, Repository, StatusOptions};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    ffi::OsStr,
    fs,
    io::Read,
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::Path,
    sync::Arc,
};
#[path = "blob_pages.rs"]
mod blob_pages;
#[path = "branch_listing.rs"]
mod branch_listing;
#[path = "commit_files.rs"]
mod commit_files;
#[path = "diff_pages.rs"]
mod diff_pages;
#[path = "history.rs"]
mod history;
#[path = "paged_rows.rs"]
mod paged_rows;
#[path = "remote_listing.rs"]
mod remote_listing;
#[path = "remote_names.rs"]
mod remote_names;
#[path = "status.rs"]
mod status;
#[path = "tag_listing.rs"]
mod tag_listing;
#[path = "worktree_listing.rs"]
pub(super) mod worktree_listing;

const MAX_ENTRIES: usize = 10_000;
const CACHE_BYTES: usize = 32 * 1024 * 1024;
/// The index is read in full for every fingerprint; this bounds that read.
const MAX_INDEX_BYTES: u64 = 256 * 1024 * 1024;
/// How much of a file is read when its timestamps are too coarse to trust.
const MAX_COARSE_READ: u64 = 8 * 1024 * 1024;
/// Listings kept for paging. Bounded by count and by `CACHE_BYTES`.
const CACHE_ENTRIES: usize = 8;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn engine(error: git2::Error) -> Error {
    let code = match error.code() {
        git2::ErrorCode::NotFound | git2::ErrorCode::UnbornBranch => "NOT_FOUND",
        git2::ErrorCode::Locked => "REPOSITORY_BUSY",
        _ => "GIT_ERROR",
    };
    // libgit2 diagnostics can contain remote URLs; keep them off the RPC boundary.
    Error::new(code, "The Git engine could not read this repository.")
}
fn io_error(_: std::io::Error) -> Error {
    Error::new("IO_ERROR", "A repository file could not be read.")
}
fn limit() -> Error {
    Error::new(
        "LIMIT_EXCEEDED",
        "Repository read exceeds the configured resource limit.",
    )
}
/// Report the identifier's own hash algorithm; never assume one.
fn oid(oid: git2::Oid) -> Value {
    json!({"algorithm":oid.object_format().str(),"hex":oid.to_string()})
}
fn wire_path(path: &Path) -> WirePath {
    WirePath::new(path.as_os_str().as_bytes())
}
fn head(repo: &Repository) -> Result<Value, Error> {
    match repo.head() {
        Ok(h) => Ok(
            json!({"oid":h.target().map(oid),"name":WirePath::new(h.name_bytes()),"detached":!h.is_branch(),"unborn":false}),
        ),
        Err(e) if e.code() == git2::ErrorCode::UnbornBranch => Ok(
            json!({"oid":null,"name":repo.find_reference("HEAD").ok().and_then(|h| h.symbolic_target_bytes().map(WirePath::new)),"detached":false,"unborn":true}),
        ),
        Err(e) => Err(engine(e)),
    }
}

#[derive(Default)]
pub struct Service {
    journal: Option<Journal>,
    remote_cache: remote_listing::Cache,
    remote_names_cache: remote_names::Cache,
    commit_files_cache: commit_files::Cache,
    diff_cache: diff_pages::Cache,
    blob_cache: blob_pages::Cache,
    history_cache: history::Cache,
    worktree_cache: worktree_listing::Cache,
    branch_cache: branch_listing::Cache,
    status_cache: status::Cache,
    tag_cache: tag_listing::Cache,
    stash_cache: super::stash::Cache,
}
pub use super::protocol::Output;
impl Service {
    pub fn with_journal(journal: Journal) -> Self {
        Self {
            journal: Some(journal),
            ..Self::default()
        }
    }
    pub fn writable(&self) -> bool {
        self.journal.is_some()
    }

    pub fn request(&mut self, request: Request) -> Result<Output, Error> {
        match request {
            Request::Clone {
                operation_id,
                url,
                path,
                branch,
                bare,
            } => {
                let journal = self.journal.as_ref().ok_or_else(|| {
                    Error::new(
                        "JOURNAL_UNAVAILABLE",
                        "Cloning requires an operation journal.",
                    )
                })?;
                super::cloning::clone(journal, &operation_id, &url, &path, branch.as_deref(), bare)
                    .map(Output::Json)
            }
            Request::Init {
                operation_id,
                path,
                initial_branch,
            } => {
                let journal = self.journal.as_ref().ok_or_else(|| {
                    Error::new(
                        "JOURNAL_UNAVAILABLE",
                        "Repository creation requires an operation journal.",
                    )
                })?;
                super::bootstrap::init(journal, &operation_id, &path, &initial_branch)
                    .map(Output::Json)
            }
            Request::Worktrees {
                repo_id,
                page_size,
                cursor,
                filter,
                branch,
                name,
                at_snapshot,
            } => {
                let query = worktree_listing::query(filter, branch, name)?;
                if let Some(at_snapshot) = at_snapshot {
                    if cursor.is_some() {
                        return Err(Error::invalid("atSnapshot and cursor cannot be combined."));
                    }
                    worktree_listing::anchored_page(
                        &mut self.worktree_cache,
                        RepoRef::decode(&repo_id)?,
                        query,
                        page_size,
                        &at_snapshot,
                    )
                    .map(Output::Json)
                } else {
                    self.page(repo_id, query, page_size, cursor)
                        .map(Output::Json)
                }
            }
            Request::Tags {
                repo_id,
                page_size,
                cursor,
                message_bytes,
            } => {
                let query = tag_listing::query(message_bytes)?;
                self.page(repo_id, query, page_size, cursor)
                    .map(Output::Json)
            }
            Request::Tag { repo_id, oid } => {
                let repo = RepoRef::decode(&repo_id)?.open()?;
                super::tags::detail(&repo, &oid).map(Output::Json)
            }
            Request::Stashes {
                repo_id,
                page_size,
                cursor,
            } => self
                .page(repo_id, "stashes".into(), page_size, cursor)
                .map(Output::Json),
            Request::BlobPage {
                repo_id,
                oid,
                max_bytes,
                cursor,
            } => blob_pages::read(
                &mut self.blob_cache,
                RepoRef::decode(&repo_id)?,
                &oid,
                max_bytes,
                cursor,
            )
            .map(Output::Json),
            Request::Blob {
                repo_id,
                oid: object_id,
            } => {
                let repo = self.repo(&repo_id)?;
                let object_id = super::branches::oid(&object_id)?;
                let (size, kind) = repo
                    .odb()
                    .map_err(engine)?
                    .read_header(object_id)
                    .map_err(engine)?;
                if kind != git2::ObjectType::Blob {
                    return Err(Error::invalid("The object is not a file blob."));
                }
                let content = if size <= 512 * 1024 {
                    Some(STANDARD.encode(repo.find_blob(object_id).map_err(engine)?.content()))
                } else {
                    None
                };
                Ok(Output::Json(
                    json!({"oid":oid(object_id),"size":size,"bytesB64":content,"truncated":content.is_none()}),
                ))
            }

            Request::RemoteRefs {
                repo_id,
                remote,
                expected_token,
                for_push,
                filter,
                page_size,
                cursor,
            } => {
                let query = remote_listing::query(&remote, &expected_token, for_push, &filter)?;
                self.page(repo_id, query, page_size, cursor)
                    .map(Output::Json)
            }
            Request::RemoteNames {
                repo_id,
                filter,
                page_size,
                cursor,
            } => self
                .page(repo_id, remote_names::query(&filter)?, page_size, cursor)
                .map(Output::Json),
            Request::Remote { repo_id, name } => {
                super::remotes::selected(&self.repo(&repo_id)?, &name).map(Output::Json)
            }
            Request::Remotes { repo_id } => {
                super::remotes::list(&self.repo(&repo_id)?).map(Output::Json)
            }
            Request::Start {
                operation_id,
                repo_id,
                expected_snapshot,
                action,
            } => self
                .start_operation(operation_id, repo_id, expected_snapshot, action)
                .map(Output::Json),
            Request::Get { operation_id } => self
                .journal
                .as_ref()
                .ok_or_else(|| {
                    Error::new("JOURNAL_UNAVAILABLE", "Operation journal is unavailable.")
                })?
                .get(&operation_id)
                .and_then(|record| serde_json::to_value(record).map_err(|_| limit()))
                .map(Output::Json),
            Request::Review { operation_id } => self
                .journal
                .as_ref()
                .ok_or_else(|| {
                    Error::new("JOURNAL_UNAVAILABLE", "Operation journal is unavailable.")
                })?
                .review(&operation_id)
                .and_then(|record| serde_json::to_value(record).map_err(|_| limit()))
                .map(Output::Json),
            Request::Open { path } => self.open(path).map(Output::Json),
            // Nothing is held open, so closing releases nothing. It is still a
            // method of the protocol, so it is still validated.
            Request::Close { repo_id } => {
                RepoRef::decode(&repo_id)?;
                Ok(Output::Json(json!({"closed":true})))
            }
            Request::StatusSummary { repo_id, path } => {
                let repo = match (repo_id, path) {
                    (Some(id), None) => self.repo(&id)?,
                    (None, Some(path)) => discover(path)?,
                    _ => return Err(Error::invalid("Specify exactly one repository ID or path.")),
                };
                status::summary(&repo).map(Output::Json)
            }
            Request::Status {
                filter,
                repo_id,
                page_size,
                cursor,
            } => self
                .page(repo_id, status::query(filter.as_ref())?, page_size, cursor)
                .map(Output::Json),
            Request::Branches {
                repo_id,
                page_size,
                cursor,
                filter,
                branch_kind,
            } => {
                let query = branch_listing::query(&filter, branch_kind.as_deref())?;
                self.page(repo_id, query, page_size, cursor)
                    .map(Output::Json)
            }
            Request::History {
                repo_id,
                page_size,
                cursor,
                revision,
                message_bytes,
            } => {
                let query = history::query(&revision, message_bytes)?;
                self.page(repo_id, query, page_size, cursor)
                    .map(Output::Json)
            }
            Request::Commit {
                repo_id,
                commit_oid,
            } => history::commit(RepoRef::decode(&repo_id)?, &commit_oid).map(Output::Json),
            Request::CommitFiles {
                repo_id,
                commit_oid,
                parent_index,
                page_size,
                cursor,
            } => {
                let commit = super::branches::oid(&commit_oid)?;
                self.page(
                    repo_id,
                    format!("commit_files:{commit}:{parent_index}"),
                    page_size,
                    cursor,
                )
                .map(Output::Json)
            }
            Request::CommitDiffPage {
                repo_id,
                commit_oid,
                path,
                parent_index,
                context_lines,
                page_size,
                max_bytes,
                cursor,
                line_encoding,
            } => diff_pages::read_cached(
                &mut self.diff_cache,
                line_encoding.is_some(),
                RepoRef::decode(&repo_id)?,
                &commit_oid,
                path,
                parent_index,
                context_lines,
                page_size,
                max_bytes,
                cursor,
            )
            .map(Output::Json),
            Request::CommitDiff {
                path,
                repo_id,
                commit_oid,
                parent_index,
                context_lines,
            } => {
                let repo = self.repo(&repo_id)?;
                let decoded = path.as_ref().map(WirePath::decode).transpose()?;
                let mut value = commit_diff(
                    &repo,
                    &commit_oid,
                    parent_index,
                    context_lines,
                    decoded.as_deref(),
                )?;
                value["readOnly"] = true.into();
                let bytes = serde_json::to_vec(&value).map_err(|_| limit())?;
                if bytes.len() > MAX_DIFF {
                    return Err(limit());
                }
                Ok(Output::Diff {
                    snapshot: commit_oid,
                    bytes,
                })
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
            } => diff_pages::read_working_cached(
                &mut self.diff_cache,
                line_encoding.is_some(),
                RepoRef::decode(&repo_id)?,
                SnapshotRef::decode(&snapshot)?,
                &entry_id,
                side,
                context_lines,
                page_size,
                max_bytes,
                cursor,
            )
            .map(Output::Json),
            Request::Diff {
                repo_id,
                snapshot,
                entry_id,
                side,
                context_lines,
            } => self.diff(&repo_id, &snapshot, &entry_id, side, context_lines),
        }
    }
    fn start_operation(
        &mut self,
        operation_id: String,
        repo_id: String,
        expected_snapshot: String,
        action: Action,
    ) -> Result<Value, Error> {
        let journal = self.journal.clone().ok_or_else(|| {
            Error::new(
                "JOURNAL_UNAVAILABLE",
                "No write can run without an operation journal.",
            )
        })?;
        let repo = self.repo(&repo_id)?;
        let common = repo.commondir().canonicalize().map_err(io_error)?;
        let metadata = fs::metadata(&common).map_err(io_error)?;
        let identity = super::journal::hash(
            &[
                common.as_os_str().as_bytes(),
                &metadata.dev().to_be_bytes(),
                &metadata.ino().to_be_bytes(),
            ]
            .concat(),
        );
        let hash=super::journal::hash(&serde_json::to_vec(&json!({"repository":identity,"worktree":wire_path(repo.path()),"snapshot":expected_snapshot,"action":action})).map_err(|_|limit())?);
        if journal.existing(&operation_id, &hash)?.is_some() {
            return serde_json::to_value(journal.get(&operation_id)?).map_err(|_| limit());
        }
        let _lock = journal.lock_repository(&identity)?;
        if let Some(record) = journal.existing(&operation_id, &hash)? {
            return serde_json::to_value(record).map_err(|_| limit());
        }
        let worktree_action = matches!(
            action,
            Action::WorktreeRepair { .. }
                | Action::WorktreeRemove { .. }
                | Action::WorktreePrune { .. }
                | Action::WorktreeAdd { .. }
                | Action::WorktreeLock { .. }
                | Action::WorktreeUnlock { .. }
        );
        let expected_query = if worktree_action {
            "worktrees"
        } else {
            "status"
        };
        // The snapshot names the listing the user acted on. It must be of the
        // kind this action requires and belong to this repository.
        let snapshot = SnapshotRef::decode(&expected_snapshot)
            .ok()
            .filter(|s| RepoRef::decode(&repo_id).is_ok_and(|r| s.r == r) && s.q == expected_query)
            .ok_or_else(|| {
                Error::new(
                    "STALE_SNAPSHOT",
                    "Read the required listing before starting this operation.",
                )
            })?;
        let ids = match &action {
            Action::Stage { entry_ids, .. }
            | Action::Unstage { entry_ids, .. }
            | Action::ConflictResolve { entry_ids, .. }
            | Action::Discard { entry_ids, .. } => entry_ids.as_slice(),
            _ => &[],
        };
        // Share this scan between snapshot validation and entry resolution.
        // Native mutation guards still rescan under their lock and before
        // publication; this only removes the adjacent duplicate preflight.
        let selected_scan = if ids.is_empty() {
            None
        } else {
            Some(statuses(&repo)?)
        };
        let current_fingerprint = if worktree_action {
            super::worktrees::token(&repo)?
        } else {
            fingerprint_scan(&repo, &repo.path().join("index"), selected_scan.as_ref())?
        };
        if current_fingerprint != snapshot.f {
            return Err(Error::new(
                "STALE_SNAPSHOT",
                "The repository changed. Refresh before applying this operation.",
            ));
        }
        if (matches!(
            action,
            Action::Stage { .. }
                | Action::Unstage { .. }
                | Action::ConflictResolve { .. }
                | Action::Discard { .. }
        ) && ids.is_empty())
            || ids.len() > MAX_ENTRIES
        {
            return Err(Error::invalid(
                "Select at least one file within the supported entry limit.",
            ));
        }
        let paths = match selected_scan.as_ref() {
            Some(scan) => Self::selected_paths_from_scan(scan, ids)?,
            None => Vec::new(),
        };
        drop(selected_scan);
        let mut record = journal.begin(&operation_id, hash, identity)?;
        match operations::apply(&repo, &action, &paths, &snapshot.f) {
            Ok(result) => {
                record.state = if result["needsResolution"] == true {
                    "needs_resolution"
                } else {
                    "succeeded"
                }
                .into();
                record.result = Some(result);
            }
            Err(error) => {
                record.state = if error.code == "OUTCOME_UNKNOWN" {
                    "outcome_unknown"
                } else {
                    "failed"
                }
                .into();
                record.error = Some(error);
            }
        }
        record.seq += 1;
        if journal.save(&record).is_err() {
            return Err(Error::new("OUTCOME_UNKNOWN",format!("Operation {operation_id} may have completed, but its final record could not be saved. Query its state before any retry.")));
        }
        serde_json::to_value(record).map_err(|_| limit())
    }
    fn open(&self, path: WirePath) -> Result<Value, Error> {
        let repo = discover(path)?;
        let git_dir = repo.path().canonicalize().map_err(io_error)?;
        let root = repo
            .workdir()
            .unwrap_or(repo.path())
            .canonicalize()
            .map_err(io_error)?;
        let common_dir = match fs::read(git_dir.join("commondir")) {
            Ok(data) => git_dir
                .join(OsStr::from_bytes(data.strip_suffix(b"\n").unwrap_or(&data)))
                .canonicalize()
                .map_err(io_error)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => git_dir.clone(),
            Err(e) => return Err(io_error(e)),
        };
        let metadata = fs::metadata(&git_dir).map_err(io_error)?;
        let repo_id = RepoRef::new(&git_dir, metadata.dev(), metadata.ino()).encode();
        let common_repo_id = hex(&Sha256::digest(common_dir.as_os_str().as_bytes()));
        Ok(
            json!({"repoId":repo_id,"commonRepoId":common_repo_id,"root":wire_path(&root),"bare":repo.is_bare(),"objectFormat":repo.object_format().str(),"head":head(&repo)?,"operationState":format!("{:?}",repo.state()),"integration":super::integration::status(&repo),"capabilities":{"readOnly":!self.writable(),"workingTree":!repo.is_bare()}}),
        )
    }
    fn repo(&self, repo_id: &str) -> Result<Repository, Error> {
        RepoRef::decode(repo_id)?.open()
    }
    fn page(
        &mut self,
        repo_id: String,
        query: String,
        count: usize,
        cursor: Option<String>,
    ) -> Result<Value, Error> {
        if !(1..=200).contains(&count) {
            return Err(Error::invalid("pageSize must be between 1 and 200."));
        }
        let repository = RepoRef::decode(&repo_id)?;
        let (snapshot, offset, key) = match cursor {
            Some(cursor) => {
                let cursor = CursorRef::decode(&cursor)?;
                if cursor.s.r != repository || cursor.s.q != query {
                    return Err(Error::invalid("Cursor does not match this query."));
                }
                if cursor.s.p.is_some()
                    && !query.starts_with("history:")
                    && !query.starts_with("history_summary:")
                {
                    return Err(Error::invalid(
                        "Pinned revisions are only valid for history cursors.",
                    ));
                }
                (Some(cursor.s), cursor.o, cursor.k)
            }
            None => (None, 0, None),
        };
        if query == "tags" || query.starts_with("tags_summary:") {
            return tag_listing::page(
                &mut self.tag_cache,
                repository,
                query,
                count,
                snapshot,
                offset,
                key,
            );
        }
        if key.is_some() {
            return Err(Error::invalid(
                "Cursor key is not supported for this query.",
            ));
        }
        if query == "worktrees" || query.starts_with("worktrees:") {
            return worktree_listing::page(
                &mut self.worktree_cache,
                repository,
                query,
                count,
                snapshot,
                offset,
            );
        }
        if query == "stashes" {
            return super::stash::page(&mut self.stash_cache, repository, count, snapshot, offset);
        }
        if query == "status" || query.starts_with("status:") {
            return status::page(
                &mut self.status_cache,
                repository,
                query,
                count,
                snapshot,
                offset,
            );
        }
        if query.starts_with("history:") || query.starts_with("history_summary:") {
            return history::page(
                &mut self.history_cache,
                repository,
                query,
                count,
                snapshot,
                offset,
            );
        }
        if query.starts_with("branches:") {
            return branch_listing::page(
                &mut self.branch_cache,
                repository,
                query,
                count,
                snapshot,
                offset,
            );
        }
        if query.starts_with("commit_files:") {
            return commit_files::page(
                &mut self.commit_files_cache,
                repository,
                query,
                count,
                snapshot,
                offset,
            );
        }
        if query.starts_with("remote_names:") {
            return remote_names::page(
                &mut self.remote_names_cache,
                repository,
                query,
                count,
                snapshot,
                offset,
            );
        }
        if query.starts_with("remote_refs:") {
            return remote_listing::page(
                &mut self.remote_cache,
                repository,
                query,
                count,
                snapshot,
                offset,
            );
        }
        Err(Error::invalid("Unsupported listing query."))
    }
    /// Confirms paths name exactly one entry of the repository's current
    /// status. Called only after the snapshot's fingerprint has been matched,
    /// so the current status is the status the entry was read from.
    #[cfg(test)]
    fn entry_paths_now(repo: &Repository, entry: &str) -> Result<Vec<Vec<u8>>, Error> {
        Self::selected_paths_now(repo, &[entry.to_owned()])
    }

    /// Validate the entire selection against one status scan. Match complete
    /// entries, including both rename paths, before flattening and deduplicating.
    #[cfg(test)]
    fn selected_paths_now(repo: &Repository, entries: &[String]) -> Result<Vec<Vec<u8>>, Error> {
        if entries.is_empty() {
            return Ok(Vec::new());
        }
        Self::selected_paths_from_scan(&statuses(repo)?, entries)
    }

    fn selected_paths_from_scan(
        scan: &StatusScan<'_>,
        entries: &[String],
    ) -> Result<Vec<Vec<u8>>, Error> {
        let requested = entries
            .iter()
            .map(|entry| EntryRef::decode(entry)?.paths())
            .collect::<Result<Vec<_>, _>>()?;
        let mut remaining: HashSet<_> = requested.iter().cloned().collect();
        for entry in scan.iter() {
            remaining.remove(&entry_paths(&entry));
            if remaining.is_empty() {
                break;
            }
        }
        if !remaining.is_empty() {
            return Err(Error::invalid("File entry is not in this snapshot."));
        }
        let mut paths: Vec<_> = requested.into_iter().flatten().collect();
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
    fn diff(
        &self,
        repo_id: &str,
        snapshot_id: &str,
        entry_id: &str,
        side: Side,
        context: u32,
    ) -> Result<Output, Error> {
        let repository = RepoRef::decode(repo_id)?;
        let snapshot = SnapshotRef::decode(snapshot_id)?;
        let value = working_comparison(&repository, &snapshot, entry_id, side, context, |diff| {
            diff_value(diff, None)
        })?;
        let bytes = serde_json::to_vec(&value).map_err(|_| limit())?;
        if bytes.len() > MAX_DIFF {
            return Err(limit());
        }
        Ok(Output::Diff {
            snapshot: snapshot_id.into(),
            bytes,
        })
    }
}

/// Use the same discovery rules for opening an editor and display-only reads.
fn discover(path: WirePath) -> Result<Repository, Error> {
    let bytes = path.decode()?;
    let path = Path::new(OsStr::from_bytes(&bytes));
    if !path.is_absolute() {
        return Err(Error::invalid("Repository path must be absolute."));
    }
    Repository::discover(path).map_err(engine)
}

/// Keep both snapshot checks around rendering. Callers may collect a bounded
/// page or a capture, but must not publish/cache it before this returns.
fn working_comparison(
    repository: &RepoRef,
    snapshot: &SnapshotRef,
    entry_id: &str,
    side: Side,
    context: u32,
    render: impl FnOnce(&git2::Diff<'_>) -> Result<Value, Error>,
) -> Result<Value, Error> {
    if context > 100 {
        return Err(Error::invalid("contextLines exceeds 100."));
    }
    if snapshot.r != *repository || snapshot.q != "status" || snapshot.p.is_some() {
        return Err(Error::invalid(
            "Diff requires a status snapshot from this repository.",
        ));
    }
    let repo = repository.open()?;
    let scan = statuses(&repo)?;
    if fingerprint_scan(&repo, &repo.path().join("index"), Some(&scan))? != snapshot.f {
        return Err(Error::new(
            "STALE_SNAPSHOT",
            "The working tree changed. Refresh changes.",
        ));
    }
    let paths = Service::selected_paths_from_scan(&scan, &[entry_id.to_owned()])?;
    drop(scan);
    let mut options = DiffOptions::new();
    options
        .context_lines(context)
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .show_untracked_content(true)
        .disable_pathspec_match(true);
    for name in &paths {
        options.pathspec(name);
    }
    // Bound libgit2's text diff work; larger/binary files return metadata.
    options.max_size(2 * 1024 * 1024);
    let tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());
    let mut diff = match side {
        Side::HeadToIndex => repo.diff_tree_to_index(tree.as_ref(), None, Some(&mut options)),
        Side::IndexToWorktree => repo.diff_index_to_workdir(None, Some(&mut options)),
        Side::HeadToWorktree => {
            repo.diff_tree_to_workdir_with_index(tree.as_ref(), Some(&mut options))
        }
    }
    .map_err(engine)?;
    diff.find_similar(Some(git2::DiffFindOptions::new().renames(true)))
        .map_err(engine)?;
    let value = render(&diff)?;
    if fingerprint(&repo)? != snapshot.f {
        return Err(Error::new(
            "STALE_SNAPSHOT",
            "The file changed while its diff was being read.",
        ));
    }
    Ok(value)
}

/// History objects are immutable: no working-tree snapshot or index is read.
fn commit_diff(
    repo: &Repository,
    commit_oid: &str,
    parent_index: usize,
    context: u32,
    path: Option<&[u8]>,
) -> Result<Value, Error> {
    commit_comparison_for_path(repo, commit_oid, parent_index, context, path, |diff| {
        diff_value(diff, path)
    })
}
fn commit_comparison(
    repo: &Repository,
    commit_oid: &str,
    parent_index: usize,
    context: u32,
    render: impl FnOnce(&git2::Diff<'_>) -> Result<Value, Error>,
) -> Result<Value, Error> {
    commit_comparison_for_path(repo, commit_oid, parent_index, context, None, render)
}
fn commit_comparison_for_path(
    repo: &Repository,
    commit_oid: &str,
    parent_index: usize,
    context: u32,
    selected: Option<&[u8]>,
    render: impl FnOnce(&git2::Diff<'_>) -> Result<Value, Error>,
) -> Result<Value, Error> {
    if context > 100 {
        return Err(Error::invalid("contextLines exceeds 100."));
    }
    // Use a bare view with an in-memory index from the selected tree so
    // uncommitted .gitattributes cannot change historical binary detection.
    let history = Repository::open_bare(repo.path()).map_err(engine)?;
    let repo = &history;
    let commit = repo
        .find_commit(super::branches::oid(commit_oid)?)
        .map_err(|_| Error::new("COMMIT_NOT_FOUND", "The selected commit is unavailable."))?;
    if (commit.parent_count() == 0 && parent_index != 0)
        || (commit.parent_count() > 0 && parent_index >= commit.parent_count())
    {
        return Err(Error::invalid(
            "Select an existing zero-based parent index; root commits use zero.",
        ));
    }
    let parent = if commit.parent_count() == 0 {
        None
    } else {
        Some(commit.parent(parent_index).map_err(engine)?)
    };
    let before = parent
        .as_ref()
        .map(|p| p.tree())
        .transpose()
        .map_err(engine)?;
    let after = commit.tree().map_err(engine)?;
    let mut attributes = git2::Index::new().map_err(engine)?;
    attributes.read_tree(&after).map_err(engine)?;
    repo.set_index(&mut attributes).map_err(engine)?;
    let mut options = DiffOptions::new();
    options.context_lines(context).max_size(2 * 1024 * 1024);
    // With FIND_RENAMES (without rewrite/copy detection), an existing blob
    // whose type stays the same cannot be a rename source or destination.
    // Restrict only that case: additions/deletions and type changes still
    // need the complete candidate set to preserve rename semantics.
    let exact = selected.filter(|path| same_blob_type(before.as_ref(), &after, path));
    if let Some(path) = exact {
        options.disable_pathspec_match(true).pathspec(path);
    }
    let mut diff = repo
        .diff_tree_to_tree(before.as_ref(), Some(&after), Some(&mut options))
        .map_err(engine)?;
    if exact.is_none() {
        diff.find_similar(Some(
            git2::DiffFindOptions::new()
                .renames(true)
                .rename_limit(1000),
        ))
        .map_err(engine)?;
    }
    let mut value = render(&diff)?;
    value["commitOid"] = oid(commit.id());
    value["parentOid"] = parent.as_ref().map(|p| oid(p.id())).unwrap_or(Value::Null);
    value["parentIndex"] = if parent.is_some() {
        parent_index.into()
    } else {
        Value::Null
    };
    value["parents"] = commit.parent_ids().map(oid).collect::<Vec<_>>().into();
    Ok(value)
}
fn same_blob_type(before: Option<&git2::Tree<'_>>, after: &git2::Tree<'_>, path: &[u8]) -> bool {
    let path = Path::new(OsStr::from_bytes(path));
    let old = before.and_then(|tree| tree.get_path(path).ok());
    let new = after.get_path(path).ok();
    matches!((old, new), (Some(old), Some(new))
        if old.kind() == Some(git2::ObjectType::Blob)
            && new.kind() == Some(git2::ObjectType::Blob)
            && old.filemode() & 0o170000 == new.filemode() & 0o170000)
}

fn delta_metadata(delta: &git2::DiffDelta<'_>) -> Value {
    json!({"oldPath":delta.old_file().path().map(wire_path),"newPath":delta.new_file().path().map(wire_path),"status":format!("{:?}",delta.status()),"oldOid":(!delta.old_file().id().is_zero()).then(||oid(delta.old_file().id())),"newOid":(!delta.new_file().id().is_zero()).then(||oid(delta.new_file().id())),"oldMode":i32::from(delta.old_file().mode()),"newMode":i32::from(delta.new_file().mode())})
}
fn diff_value(diff: &git2::Diff<'_>, path: Option<&[u8]>) -> Result<Value, Error> {
    let mut files = Vec::new();
    let mut size = 0usize;
    let mut truncated = false;
    let mut line_count = 0usize;
    for index in 0..diff.deltas().len() {
        let delta = diff.get_delta(index).ok_or_else(limit)?;
        if path.is_some_and(|p| {
            delta.old_file().path_bytes() != Some(p) && delta.new_file().path_bytes() != Some(p)
        }) {
            continue;
        }
        if files.len() >= MAX_ENTRIES || size > MAX_DIFF / 2 {
            truncated = true;
            break;
        }
        size += 1024;

        let patch = git2::Patch::from_diff(diff, index).map_err(engine)?;
        let delta = diff.get_delta(index).ok_or_else(limit)?;
        let mut hunks = Vec::new();
        let (mut additions, mut deletions) = (0, 0);
        if let Some(patch) = patch {
            let (_, added, deleted) = patch.line_stats().map_err(engine)?;
            additions = added;
            deletions = deleted;
            for h in 0..patch.num_hunks() {
                let (hunk, lines) = patch.hunk(h).map_err(engine)?;
                let hunk_id = super::hunks::id(&patch, h)?;
                let mut output = Vec::new();
                for l in 0..lines {
                    let line = patch.line_in_hunk(h, l).map_err(engine)?;
                    size += line.content().len() * 2 + 256;
                    line_count += 1;
                    if size > MAX_DIFF / 2 || line_count > 100_000 {
                        truncated = true;
                        break;
                    }
                    output.push(json!({"id":super::hunks::line_id(&hunk_id, l, &line),"origin":line.origin().to_string(),"oldLine":line.old_lineno(),"newLine":line.new_lineno(),"content":WirePath::new(line.content())}));
                }
                if truncated {
                    // A truncated hunk is not addressable, and neither are its lines.
                    for line in &mut output {
                        line["id"] = serde_json::Value::Null;
                    }
                }
                hunks.push(json!({"id":if truncated { None } else { Some(hunk_id) },"oldStart":hunk.old_start(),"oldLines":hunk.old_lines(),"newStart":hunk.new_start(),"newLines":hunk.new_lines(),"lines":output}));
                if truncated {
                    break;
                }
            }
        }
        let mut file = delta_metadata(&delta);
        file["binary"] = (delta.old_file().is_binary() || delta.new_file().is_binary()).into();
        file["additions"] = additions.into();
        file["deletions"] = deletions.into();
        file["hunks"] = hunks.into();
        files.push(file);
        if truncated {
            break;
        }
    }
    if path.is_some() && files.is_empty() {
        return Err(Error::new(
            "FILE_NOT_CHANGED",
            "The selected path is not changed in this comparison.",
        ));
    }
    Ok(json!({"files":files,"truncated":truncated,"readOnly":true}))
}

fn entry_paths(entry: &git2::StatusEntry<'_>) -> Vec<Vec<u8>> {
    let mut names = Vec::new();
    for delta in [entry.head_to_index(), entry.index_to_workdir()]
        .into_iter()
        .flatten()
    {
        for file in [delta.old_file(), delta.new_file()] {
            if let Some(path) = file.path() {
                names.push(path.as_os_str().as_bytes().to_vec());
            }
        }
    }
    names.sort();
    names.dedup();
    names
}

/// Native status ordering spans the complete repository; pagination is applied
/// only when serializing rows, never when computing write guards.
struct StatusScan<'repo> {
    entries: git2::Statuses<'repo>,
    total: usize,
}
impl StatusScan<'_> {
    fn iter(&self) -> impl Iterator<Item = git2::StatusEntry<'_>> {
        self.entries.iter()
    }
}
fn statuses(repo: &Repository) -> Result<StatusScan<'_>, Error> {
    let mut options = StatusOptions::new();
    options
        .include_untracked(true)
        // `git status` collapses untracked directories to a single row and so
        // do we: recursing expands one unignored vendor directory
        // (node_modules, target, dist) into tens of thousands of entries that
        // bury the user's real changes and that nothing downstream can use.
        .recurse_untracked_dirs(false)
        .include_ignored(false)
        .update_index(false)
        .renames_head_to_index(true)
        .renames_index_to_workdir(true);
    let entries = repo.statuses(Some(&mut options)).map_err(engine)?;
    let total = entries.len();
    Ok(StatusScan { entries, total })
}
pub(super) fn fingerprint(repo: &Repository) -> Result<String, Error> {
    fingerprint_index(repo, &repo.path().join("index"))
}

// The caller has attached this prepared index to repo. Capture the intended
// post-operation state before publishing it and releasing the native index lock.
pub(super) fn fingerprint_index(repo: &Repository, index_path: &Path) -> Result<String, Error> {
    fingerprint_scan(repo, index_path, None)
}
fn fingerprint_scan(
    repo: &Repository,
    index_path: &Path,
    captured: Option<&StatusScan<'_>>,
) -> Result<String, Error> {
    let mut hash = Sha256::new();
    hash.update(serde_json::to_vec(&head(repo)?).map_err(|_| limit())?);
    hash.update(format!("{:?}", repo.state()));
    // The index is hashed by content: a prepared index is fingerprinted, then
    // published by rename, and the published one must match -- a rename can
    // change its change-time, so metadata would not. It gets a bound of its
    // own, so the size of the working tree's changes never counts against it.
    let mut budget = MAX_INDEX_BYTES;
    hash_file(index_path, &mut hash, &mut budget).map_err(|e| {
        if e.code == "LIMIT_EXCEEDED" {
            Error::new(
                "LIMIT_EXCEEDED",
                format!(
                    "The repository's index is larger than {} MiB, too large to read safely.",
                    MAX_INDEX_BYTES / (1024 * 1024)
                ),
            )
        } else {
            e
        }
    })?;
    for metadata in [
        "newport-merge.json",
        "newport-rebase.json",
        "MERGE_HEAD",
        "MERGE_MSG",
        "ORIG_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
    ] {
        hash_file(&repo.path().join(metadata), &mut hash, &mut budget)?;
    }
    hash.update(super::rebase::native_hash(repo)?);
    if let Some(root) = repo.workdir() {
        let fresh;
        let scan = match captured {
            Some(scan) => scan,
            None => {
                fresh = statuses(repo)?;
                &fresh
            }
        };
        hash.update((scan.total as u64).to_be_bytes());
        // Version the old truncation marker while retaining identical hashes
        // for formerly complete listings.
        hash.update([0]);
        let canonical_root = root.canonicalize().map_err(io_error)?;
        for entry in scan.iter() {
            hash.update(entry.status().bits().to_be_bytes());
            for name in entry_paths(&entry) {
                hash.update(&name);
                let relative = Path::new(OsStr::from_bytes(&name));
                if relative.is_absolute()
                    || relative
                        .components()
                        .any(|c| matches!(c, std::path::Component::ParentDir))
                {
                    return Err(Error::invalid("Invalid repository file path."));
                }
                let path = root.join(relative);
                // Reject symlink ancestors. The file itself may be a symlink; hash
                // the link's bytes without following it outside the worktree.
                let mut parent = path.parent();
                while let Some(directory) = parent {
                    match directory.canonicalize() {
                        Ok(canonical) => {
                            if !canonical.starts_with(&canonical_root) {
                                return Err(Error::new(
                                    "PERMISSION_DENIED",
                                    "File path leaves the worktree.",
                                ));
                            }
                            break;
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                            parent = directory.parent()
                        }
                        Err(e) => return Err(io_error(e)),
                    }
                }
                hash_metadata(&path, &mut hash)?;
            }
        }
    }
    Ok(hex(&hash.finalize()))
}
/// Identifies a changed working-tree file by what the filesystem records about
/// it -- type, mode, size, device, inode, and modification and change times to
/// the nanosecond -- the way Git itself decides a file may have changed, rather
/// than by reading it. Any write moves the modification or change time, so the
/// fingerprint still changes when the file does; what it no longer does is
/// read every changed and untracked file in full, twice per status read, and
/// fail outright once they add up to more than a fixed budget.
fn hash_metadata(path: &Path, hash: &mut Sha256) -> Result<(), Error> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            hash.update(b"missing");
            return Ok(());
        }
        Err(e) => return Err(io_error(e)),
    };
    for value in [
        u64::from(metadata.mode()),
        metadata.len(),
        metadata.dev(),
        metadata.ino(),
    ] {
        hash.update(value.to_be_bytes());
    }
    for value in [
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.ctime(),
        metadata.ctime_nsec(),
    ] {
        hash.update(value.to_be_bytes());
    }
    if metadata.file_type().is_symlink() {
        hash.update(
            fs::read_link(path)
                .map_err(io_error)?
                .as_os_str()
                .as_bytes(),
        );
    } else if metadata.is_file() {
        // On a filesystem with whole-second timestamps a same-size rewrite
        // within one second leaves every field above as it was. Such a
        // filesystem shows itself in the timestamps -- no nanoseconds -- and
        // there a small file is read as well. The test depends on the file
        // alone, never on the clock: a rule like "written in the last two
        // seconds" changed the fingerprint of an untouched file as time passed,
        // so a snapshot went stale by itself and the next write was refused.
        let coarse = metadata.mtime_nsec() == 0 && metadata.ctime_nsec() == 0;
        if coarse && metadata.len() <= MAX_COARSE_READ {
            let mut budget = MAX_COARSE_READ;
            hash_file(path, hash, &mut budget)?;
        }
    } else if metadata.is_dir() {
        if let Ok(submodule) = Repository::open(path) {
            hash.update(serde_json::to_vec(&head(&submodule)?).map_err(|_| limit())?);
        }
    } else {
        return Err(Error::new(
            "UNSUPPORTED_FILE",
            "Special files cannot be read as Git content.",
        ));
    }
    Ok(())
}
fn hash_file(path: &Path, hash: &mut Sha256, budget: &mut u64) -> Result<(), Error> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            hash.update(b"missing");
            return Ok(());
        }
        Err(e) => return Err(io_error(e)),
    };
    hash.update(metadata.mode().to_be_bytes());
    if metadata.file_type().is_symlink() {
        hash.update(
            fs::read_link(path)
                .map_err(io_error)?
                .as_os_str()
                .as_bytes(),
        );
    } else if metadata.is_file() {
        if metadata.len() > *budget {
            return Err(limit());
        }
        let mut file = fs::File::open(path).map_err(io_error)?.take(*budget + 1);
        let mut buffer = [0; 16384];
        loop {
            let n = file.read(&mut buffer).map_err(io_error)?;
            if n == 0 {
                break;
            }
            if n as u64 > *budget {
                return Err(limit());
            }
            *budget -= n as u64;
            hash.update(&buffer[..n]);
        }
    } else if metadata.is_dir() {
        if let Ok(submodule) = Repository::open(path) {
            hash.update(serde_json::to_vec(&head(&submodule)?).map_err(|_| limit())?);
        }
    } else {
        return Err(Error::new(
            "UNSUPPORTED_FILE",
            "Special files cannot be read as Git content.",
        ));
    }
    Ok(())
}
/// Read small administrative files without following a substituted symlink.
pub(super) fn worktree_metadata(path: &Path) -> Result<Vec<u8>, Error> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(io_error)?;
    let metadata = file.metadata().map_err(io_error)?;
    if !metadata.is_file() || metadata.len() > 16 * 1024 {
        return Err(limit());
    }
    let mut bytes = Vec::new();
    file.take(16 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() > 16 * 1024 {
        return Err(limit());
    }
    Ok(bytes)
}
pub(super) fn worktree_main(repo: &Repository) -> Result<Value, Error> {
    let common = repo.commondir().canonicalize().map_err(io_error)?;
    let current = repo.path().canonicalize().map_err(io_error)?;
    let main = Repository::open(&common).map_err(engine)?;
    let row = json!({"name":null,"kind":if main.is_bare(){"bare"}else{"main"},"path":wire_path(main.workdir().unwrap_or(&common)),"gitDir":wire_path(&common),"current":current==common,"state":"available","head":head(&main)?,"locked":false,"lockReason":null,"prunable":false});
    Ok(row)
}
pub(super) fn worktree_row(
    repo: &Repository,
    common: &Path,
    current: &Path,
    registry: &Path,
    name: &OsStr,
) -> Result<Value, Error> {
    let admin = registry.join(name);
    let mut row = json!({"name":WirePath::new(name.as_bytes()),"kind":"linked","path":null,"gitDir":wire_path(&admin),"current":current==admin,"state":"invalid","head":null,"locked":null,"lockReason":null,"prunable":null});
    // Keep corrupt/incomplete registry entries visible rather than silently
    // dropping them from libgit2's list of valid worktree records.
    if !fs::symlink_metadata(&admin).is_ok_and(|m| m.is_dir()) {
        return Ok(row);
    }
    match fs::symlink_metadata(admin.join("locked")) {
        Ok(_) => {
            row["locked"] = true.into();
            match worktree_metadata(&admin.join("locked")) {
                Ok(reason) => row["lockReason"] = json!(WirePath::new(&reason)),
                Err(_) => row["lockReasonUnavailable"] = true.into(),
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => row["locked"] = false.into(),
        Err(_) => {
            row["state"] = "unreadable".into();
            return Ok(row);
        }
    }
    // Never hand unbounded administrative files to libgit2's parser.
    if ["gitdir", "commondir", "HEAD"]
        .iter()
        .any(|file| worktree_metadata(&admin.join(file)).is_err())
    {
        return Ok(row);
    }
    let Some(name) = name.to_str() else {
        row["state"] = "unsupported".into();
        row["errorCode"] = "UNSUPPORTED_ENCODING".into();
        return Ok(row);
    };
    let worktree = repo.find_worktree(name).ok();
    if let Some(worktree) = worktree {
        row["path"] = json!(wire_path(worktree.path()));
        row["state"] = match fs::metadata(worktree.path()) {
            Ok(_) if worktree.validate().is_ok() => "available",
            Ok(_) => "invalid",
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => "missing",
            Err(_) => "unreadable",
        }
        .into();
        if let Ok(linked) = Repository::open_from_worktree(&worktree) {
            // A substituted checkout must not be represented as this tree.
            if linked.commondir().canonicalize().ok().as_ref() == Some(&common.to_path_buf())
                && linked.path().canonicalize().ok().as_ref() == Some(&admin)
            {
                row["head"] = head(&linked).unwrap_or(Value::Null);
            } else {
                row["state"] = "invalid".into();
            }
        }
    }
    if row["locked"] == true {
        row["prunable"] = false.into();
    } else if row["locked"] == false
        && matches!(row["state"].as_str(), Some("available" | "missing"))
    {
        row["prunable"] = (row["state"] == "missing").into();
    }
    Ok(row)
}
#[cfg(test)]
pub(super) fn worktree_rows(repo: &Repository) -> Result<Vec<Value>, Error> {
    let mut rows = Vec::new();
    worktree_listing::scan(repo, |row, _| {
        rows.push(row);
        Ok(())
    })?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn review_protocol_dispatch_preserves_unknown_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let journal = Journal::open(temp.path().join("journal"), id()).unwrap();
        let operation_id = id();
        let guard = journal.lock_repository("repo").unwrap();
        journal
            .begin(&operation_id, "hash".into(), "repo".into())
            .unwrap();
        drop(guard);
        let unknown = journal.get(&operation_id).unwrap();
        let request = || {
            serde_json::from_value::<Request>(
                json!({"method":"operation.review", "params":{"operationId":operation_id}}),
            )
            .unwrap()
        };
        assert!(crate::git::protocol::METHODS.contains(&"operation.review"));
        assert!(Service::default().request(request()).is_err());
        let mut service = Service::with_journal(journal);
        let reviewed = json(&mut service, request());
        assert_eq!(reviewed["state"], "reviewed_unknown");
        assert_eq!(
            reviewed["error"],
            serde_json::to_value(unknown.error).unwrap()
        );
        assert_eq!(json(&mut service, request()), reviewed);
        assert_eq!(json(&mut service, Request::Get { operation_id }), reviewed);
    }
    fn id() -> String {
        uuid::Uuid::new_v4().to_string()
    }
    pub(super) fn commit(repo: &Repository, text: &str) {
        let mut index = repo.index().unwrap();
        index
            .add_all(["*"], git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = git2::Signature::now("Test", "test@example.test").unwrap();
        let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
        let parents = parent.iter().collect::<Vec<_>>();
        repo.commit(Some("HEAD"), &sig, &sig, text, &tree, &parents)
            .unwrap();
    }
    fn json(service: &mut Service, request: Request) -> Value {
        match service.request(request).unwrap() {
            Output::Json(v) => v,
            _ => panic!("expected JSON"),
        }
    }
    fn open(service: &mut Service, path: &Path) -> String {
        json(
            service,
            Request::Open {
                path: wire_path(path),
            },
        )["repoId"]
            .as_str()
            .unwrap()
            .into()
    }
    #[test]
    fn listing_cursors_reject_history_only_fields_outside_history() {
        let temp = tempfile::tempdir().unwrap();
        Repository::init(temp.path()).unwrap();
        let mut service = Service::default();
        let id = open(&mut service, temp.path());
        for query in ["status", "stashes"] {
            let first = service.page(id.clone(), query.into(), 20, None).unwrap();
            let mut snapshot = SnapshotRef::decode(first["snapshot"].as_str().unwrap()).unwrap();
            snapshot.p = Some("a".repeat(40));
            let cursor = CursorRef {
                s: snapshot,
                o: 0,
                k: None,
            }
            .encode();
            let result = service.page(id.clone(), query.into(), 20, Some(cursor));
            assert!(
                matches!(result, Err(Error {code,..}) if code == "INVALID_REQUEST"),
                "history field accepted for {query}"
            );
        }
    }

    #[test]
    fn bulk_selection_validates_complete_entries_and_deduplicates_paths() {
        let temp = tempfile::tempdir().unwrap();
        let repo = Repository::init(temp.path()).unwrap();
        fs::write(temp.path().join("a"), "a").unwrap();
        fs::write(temp.path().join("b"), "b").unwrap();
        let a = EntryRef::new(&[b"a".to_vec()]).encode();
        let b = EntryRef::new(&[b"b".to_vec()]).encode();
        assert_eq!(
            Service::selected_paths_now(&repo, &[b.clone(), a.clone(), a.clone()]).unwrap(),
            vec![b"a".to_vec(), b"b".to_vec()]
        );
        // Individually valid paths cannot be forged into a rename entry.
        let combined = EntryRef::new(&[b"a".to_vec(), b"b".to_vec()]).encode();
        assert!(Service::selected_paths_now(&repo, &[a.clone(), combined]).is_err());
        fs::remove_file(temp.path().join("b")).unwrap();
        assert!(Service::selected_paths_now(&repo, &[a, b]).is_err());
        assert!(Service::selected_paths_now(&repo, &["invalid".into()]).is_err());
        assert!(Service::selected_paths_now(&repo, &[]).unwrap().is_empty());
    }

    #[test]
    fn working_comparison_rejects_changes_during_render_for_every_side() {
        for side in [
            Side::HeadToIndex,
            Side::IndexToWorktree,
            Side::HeadToWorktree,
        ] {
            let temp = tempfile::tempdir().unwrap();
            let repo = Repository::init(temp.path()).unwrap();
            let file = temp.path().join("a");
            fs::write(&file, "original\n").unwrap();
            commit(&repo, "initial");
            fs::write(&file, "staged\n").unwrap();
            let mut index = repo.index().unwrap();
            index.add_path(Path::new("a")).unwrap();
            index.write().unwrap();
            fs::write(&file, "working\n").unwrap();
            let mut service = Service::default();
            let repository = RepoRef::decode(&open(&mut service, temp.path())).unwrap();
            let snapshot = SnapshotRef {
                r: repository.clone(),
                q: "status".into(),
                f: fingerprint(&repo).unwrap(),
                p: None,
            };
            let entry = EntryRef::new(&[b"a".to_vec()]).encode();
            let index_before = fs::read(repo.path().join("index")).unwrap();
            let mut rendered = false;
            let result =
                working_comparison(&repository, &snapshot, &entry, side.clone(), 3, |diff| {
                    let value = diff_value(diff, None)?;
                    assert!(!value["files"].as_array().unwrap().is_empty());
                    rendered = true;
                    fs::write(&file, "changed while rendering\n").unwrap();
                    Ok(value)
                });
            assert!(rendered);
            assert_eq!(result.unwrap_err().code, "STALE_SNAPSHOT");
            assert_eq!(fs::read(repo.path().join("index")).unwrap(), index_before);
            // A stale initial snapshot must fail before any capture/render work.
            let result = working_comparison(&repository, &snapshot, &entry, side, 3, |_| {
                panic!("stale snapshot reached renderer")
            });
            assert_eq!(result.unwrap_err().code, "STALE_SNAPSHOT");
        }
    }

    #[test]
    fn working_comparison_rejects_foreign_snapshot_scope_before_rendering() {
        let temp = tempfile::tempdir().unwrap();
        let repo = Repository::init(temp.path()).unwrap();
        fs::write(temp.path().join("a"), "untracked\n").unwrap();
        let mut service = Service::default();
        let repository = RepoRef::decode(&open(&mut service, temp.path())).unwrap();
        let entry = EntryRef::new(&[b"a".to_vec()]).encode();
        for (query, parameter) in [("history", None), ("status", Some("unexpected".into()))] {
            let snapshot = SnapshotRef {
                r: repository.clone(),
                q: query.into(),
                f: fingerprint(&repo).unwrap(),
                p: parameter,
            };
            let result = working_comparison(
                &repository,
                &snapshot,
                &entry,
                Side::IndexToWorktree,
                3,
                |_| panic!("invalid snapshot reached renderer"),
            );
            assert_eq!(result.unwrap_err().code, "INVALID_REQUEST");
        }
    }

    #[test]
    fn opening_aliases_returns_one_canonical_project_root() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        Repository::init(&root).unwrap();
        fs::create_dir(root.join("nested")).unwrap();
        let alias = temp.path().join("alias");
        std::os::unix::fs::symlink(&root, &alias).unwrap();
        let mut service =
            Service::with_journal(Journal::open(temp.path().join("journal"), id()).unwrap());
        let expected = serde_json::to_value(wire_path(&root.canonicalize().unwrap())).unwrap();
        for path in [&root, &root.join("nested"), &alias, &alias.join("nested")] {
            let opened = json(
                &mut service,
                Request::Open {
                    path: wire_path(path),
                },
            );
            assert_eq!(opened["root"], expected);
        }
    }
    #[test]
    fn moved_worktree_repair_preserves_files_locks_and_replays() {
        let temp = tempfile::tempdir().unwrap();
        let main = Repository::init(temp.path().join("main")).unwrap();
        fs::write(main.workdir().unwrap().join("a"), "original").unwrap();
        commit(&main, "initial");
        let root = temp.path().join("linked");
        let worktree = main.worktree("linked", &root, None).unwrap();
        worktree.lock(Some("portable checkout")).unwrap();
        let other = temp.path().join("other");
        main.worktree("other", &other, None).unwrap();
        let admin = main.path().join("worktrees/linked");
        let head_before = fs::read(admin.join("HEAD")).unwrap();
        let index_before = fs::read(admin.join("index")).unwrap();
        let checkout_link = fs::read(root.join(".git")).unwrap();
        let mut service =
            Service::with_journal(Journal::open(temp.path().join("journal"), id()).unwrap());
        let repo_id = open(&mut service, main.workdir().unwrap());
        let request = |service: &mut Service, destination: &Path| {
            let listing = json(
                service,
                Request::Worktrees {
                    at_snapshot: None,
                    filter: String::new(),
                    branch: None,
                    name: None,
                    repo_id: repo_id.clone(),
                    page_size: 100,
                    cursor: None,
                },
            );
            Request::Start {
                operation_id: id(),
                repo_id: repo_id.clone(),
                expected_snapshot: listing["snapshot"].as_str().unwrap().into(),
                action: Action::WorktreeRepair {
                    name: "linked".into(),
                    path: wire_path(destination),
                },
            }
        };
        let copied = temp.path().join("copied");
        fs::create_dir(&copied).unwrap();
        fs::write(copied.join(".git"), &checkout_link).unwrap();
        let redirect = request(&mut service, &copied);
        assert_eq!(json(&mut service, redirect)["error"]["code"], "PATH_EXISTS");
        let mismatch = request(&mut service, &other);
        assert_eq!(
            json(&mut service, mismatch)["error"]["code"],
            "WORKTREE_MISMATCH"
        );
        fs::write(root.join("a"), "dirty").unwrap();
        fs::write(root.join("untracked"), "keep").unwrap();
        let moved = temp.path().join("moved");
        fs::rename(&root, &moved).unwrap();
        fs::write(admin.join("gitdir.lock"), "other tool").unwrap();
        let busy = request(&mut service, &moved);
        assert_eq!(json(&mut service, busy)["error"]["code"], "PATH_EXISTS");
        fs::remove_file(admin.join("gitdir.lock")).unwrap();
        let repair = request(&mut service, &moved);
        let repaired = json(&mut service, repair.clone());
        assert_eq!(repaired["state"], "succeeded", "{repaired}");
        assert_eq!(repaired["result"]["changed"], true);
        assert_eq!(
            Repository::open(&moved)
                .unwrap()
                .workdir()
                .unwrap()
                .canonicalize()
                .unwrap(),
            moved.canonicalize().unwrap()
        );
        assert_eq!(fs::read(admin.join("HEAD")).unwrap(), head_before);
        assert_eq!(fs::read(admin.join("index")).unwrap(), index_before);
        assert_eq!(fs::read(moved.join(".git")).unwrap(), checkout_link);
        assert_eq!(fs::read(moved.join("a")).unwrap(), b"dirty");
        assert_eq!(fs::read(moved.join("untracked")).unwrap(), b"keep");
        assert_eq!(
            main.find_worktree("linked").unwrap().is_locked().unwrap(),
            git2::WorktreeLockStatus::Locked(Some("portable checkout".into()))
        );
        let noop = request(&mut service, &moved);
        assert_eq!(json(&mut service, noop)["result"]["changed"], false);
        fs::rename(&moved, temp.path().join("moved-again")).unwrap();
        assert_eq!(json(&mut service, repair), repaired);
        assert!(!moved.exists());
    }
    #[test]
    fn worktree_removal_preserves_user_files_and_pruning_keeps_moved_checkout() {
        fn request(service: &mut Service, repo_id: &str, action: Action) -> Request {
            let listing = json(
                service,
                Request::Worktrees {
                    at_snapshot: None,
                    filter: String::new(),
                    branch: None,
                    name: None,
                    repo_id: repo_id.into(),
                    page_size: 100,
                    cursor: None,
                },
            );
            Request::Start {
                operation_id: id(),
                repo_id: repo_id.into(),
                expected_snapshot: listing["snapshot"].as_str().unwrap().into(),
                action,
            }
        }
        let temp = tempfile::tempdir().unwrap();
        let repo = Repository::init(temp.path().join("main")).unwrap();
        fs::write(repo.workdir().unwrap().join("a"), "original").unwrap();
        fs::write(repo.workdir().unwrap().join(".gitignore"), "ignored.txt\n").unwrap();
        commit(&repo, "initial");
        let root = temp.path().join("linked");
        let tree = repo.worktree("linked", &root, None).unwrap();
        let mut service =
            Service::with_journal(Journal::open(temp.path().join("journal"), id()).unwrap());
        let repo_id = open(&mut service, repo.workdir().unwrap());
        let linked_id = open(&mut service, &root);
        let remove = || Action::WorktreeRemove {
            name: "linked".into(),
        };
        let current = request(&mut service, &linked_id, remove());
        assert_eq!(
            json(&mut service, current)["error"]["code"],
            "CURRENT_WORKTREE"
        );
        tree.lock(None).unwrap();
        let locked = request(&mut service, &repo_id, remove());
        assert_eq!(
            json(&mut service, locked)["error"]["code"],
            "WORKTREE_LOCKED"
        );
        tree.unlock().unwrap();
        for filename in ["untracked.txt", "ignored.txt", "a"] {
            fs::write(root.join(filename), "keep this content").unwrap();
            let dirty = request(&mut service, &repo_id, remove());
            let result = json(&mut service, dirty);
            assert_eq!(result["error"]["code"], "WORKTREE_DIRTY", "{result}");
            assert_eq!(fs::read(root.join(filename)).unwrap(), b"keep this content");
            if filename == "a" {
                fs::write(root.join(filename), "original").unwrap();
            } else {
                fs::remove_file(root.join(filename)).unwrap();
            }
        }
        let linked = Repository::open(&root).unwrap();
        let mut index = linked.index().unwrap();
        let mut entry = index.get_path(Path::new("a"), 0).unwrap();
        entry.flags |= 0x8000;
        index.add(&entry).unwrap();
        index.write().unwrap();
        let hidden = request(&mut service, &repo_id, remove());
        assert_eq!(
            json(&mut service, hidden)["error"]["code"],
            "UNSUPPORTED_INDEX"
        );
        entry.flags &= !0x8000;
        index.add(&entry).unwrap();
        index.write().unwrap();
        fs::write(linked.path().join("index.lock"), "owned by another tool").unwrap();
        let busy = request(&mut service, &repo_id, remove());
        assert_eq!(json(&mut service, busy)["error"]["code"], "REPOSITORY_BUSY");
        fs::remove_file(linked.path().join("index.lock")).unwrap();
        let remove = request(&mut service, &repo_id, remove());
        let result = json(&mut service, remove.clone());
        assert_eq!(result["state"], "succeeded", "{result}");
        assert!(!root.exists());
        assert!(!repo.path().join("worktrees/linked").exists());
        assert!(repo.find_branch("linked", git2::BranchType::Local).is_ok());
        assert_eq!(json(&mut service, remove), result);
        let missing = temp.path().join("missing");
        repo.worktree("missing", &missing, None).unwrap();
        let valid = request(
            &mut service,
            &repo_id,
            Action::WorktreePrune {
                name: "missing".into(),
            },
        );
        assert_eq!(
            json(&mut service, valid)["error"]["code"],
            "INVALID_WORKTREE"
        );
        let moved = temp.path().join("moved");
        fs::rename(&missing, &moved).unwrap();
        let prune = request(
            &mut service,
            &repo_id,
            Action::WorktreePrune {
                name: "missing".into(),
            },
        );
        let pruned = json(&mut service, prune.clone());
        assert_eq!(pruned["state"], "succeeded", "{pruned}");
        assert_eq!(pruned["result"]["registrationOnly"], true);
        assert_eq!(fs::read(moved.join("a")).unwrap(), b"original");
        assert!(repo.find_branch("missing", git2::BranchType::Local).is_ok());
        assert_eq!(json(&mut service, prune), pruned);
    }
    #[test]
    fn worktree_creation_checks_branch_paths_hooks_and_replays() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let repo = Repository::init(temp.path().join("main")).unwrap();
        fs::write(repo.workdir().unwrap().join("a"), "committed").unwrap();
        commit(&repo, "initial");
        let tip = repo.head().unwrap().peel_to_commit().unwrap();
        repo.branch("topic", &tip, false).unwrap();
        repo.branch("blocked", &tip, false).unwrap();
        fs::write(repo.workdir().unwrap().join("a"), "dirty").unwrap();
        let before_index = fs::read(repo.path().join("index")).unwrap();
        let mut service =
            Service::with_journal(Journal::open(temp.path().join("journal"), id()).unwrap());
        let repo_id = open(&mut service, repo.workdir().unwrap());
        let request =
            |service: &mut Service, name: &str, path: &Path, branch: &str, oid: String| {
                let listing = json(
                    service,
                    Request::Worktrees {
                        at_snapshot: None,
                        filter: String::new(),
                        branch: None,
                        name: None,
                        repo_id: repo_id.clone(),
                        page_size: 100,
                        cursor: None,
                    },
                );
                Request::Start {
                    operation_id: id(),
                    repo_id: repo_id.clone(),
                    expected_snapshot: listing["snapshot"].as_str().unwrap().into(),
                    action: Action::WorktreeAdd {
                        name: name.into(),
                        path: wire_path(path),
                        branch: branch.into(),
                        expected_oid: oid,
                        locked: true,
                        new_branch: false,
                    },
                }
            };
        let target = temp.path().join("topic-checkout");
        let stale = request(&mut service, "topic", &target, "topic", "0".repeat(40));
        assert_eq!(
            json(&mut service, stale)["error"]["code"],
            "STALE_REFERENCE"
        );
        fs::create_dir(&target).unwrap();
        fs::write(target.join("keep"), "keep").unwrap();
        let exists = request(
            &mut service,
            "topic",
            &target,
            "topic",
            tip.id().to_string(),
        );
        assert_eq!(json(&mut service, exists)["error"]["code"], "PATH_EXISTS");
        assert_eq!(fs::read(target.join("keep")).unwrap(), b"keep");
        fs::remove_file(target.join("keep")).unwrap();
        fs::remove_dir(&target).unwrap();
        let add = request(
            &mut service,
            "topic",
            &target,
            "topic",
            tip.id().to_string(),
        );
        let result = json(&mut service, add.clone());
        assert_eq!(result["state"], "succeeded", "{result}");
        assert_eq!(fs::read(target.join("a")).unwrap(), b"committed");
        let linked = Repository::open(&target).unwrap();
        assert_eq!(linked.head().unwrap().name().unwrap(), "refs/heads/topic");
        assert_eq!(linked.head().unwrap().target(), Some(tip.id()));
        assert_eq!(
            repo.find_worktree("topic").unwrap().is_locked().unwrap(),
            git2::WorktreeLockStatus::Locked(None)
        );
        let duplicate_branch = request(
            &mut service,
            "another",
            &temp.path().join("another"),
            "topic",
            tip.id().to_string(),
        );
        assert_eq!(
            json(&mut service, duplicate_branch)["error"]["code"],
            "BRANCH_IN_USE"
        );
        assert!(!temp.path().join("another").exists());
        let hook = repo.path().join("hooks/post-checkout");
        fs::write(&hook, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o700)).unwrap();
        let rejected = request(
            &mut service,
            "blocked",
            &temp.path().join("blocked"),
            "blocked",
            tip.id().to_string(),
        );
        assert_eq!(
            json(&mut service, rejected)["error"]["code"],
            "UNSUPPORTED_HOOK"
        );
        assert!(!temp.path().join("blocked").exists());
        assert!(!repo.path().join("worktrees/blocked").exists());
        fs::rename(&target, temp.path().join("moved")).unwrap();
        assert_eq!(json(&mut service, add), result);
        assert!(!target.exists());
        assert_eq!(
            fs::read(repo.workdir().unwrap().join("a")).unwrap(),
            b"dirty"
        );
        assert_eq!(fs::read(repo.path().join("index")).unwrap(), before_index);
    }
    #[test]
    fn a_worktree_can_start_on_a_new_branch_and_refuses_an_existing_one() {
        let temp = tempfile::tempdir().unwrap();
        let repo = Repository::init(temp.path().join("main")).unwrap();
        fs::write(repo.workdir().unwrap().join("a"), "committed").unwrap();
        commit(&repo, "initial");
        let tip = repo.head().unwrap().peel_to_commit().unwrap();
        repo.branch("taken", &tip, false).unwrap();
        let mut service =
            Service::with_journal(Journal::open(temp.path().join("journal"), id()).unwrap());
        let repo_id = open(&mut service, repo.workdir().unwrap());
        let request = |service: &mut Service, name: &str, path: &Path, branch: &str| {
            let listing = json(
                service,
                Request::Worktrees {
                    at_snapshot: None,
                    filter: String::new(),
                    branch: None,
                    name: None,
                    repo_id: repo_id.clone(),
                    page_size: 100,
                    cursor: None,
                },
            );
            Request::Start {
                operation_id: id(),
                repo_id: repo_id.clone(),
                expected_snapshot: listing["snapshot"].as_str().unwrap().into(),
                action: Action::WorktreeAdd {
                    name: name.into(),
                    path: wire_path(path),
                    branch: branch.into(),
                    expected_oid: tip.id().to_string(),
                    locked: false,
                    new_branch: true,
                },
            }
        };
        // An existing name is refused before anything is created.
        let clash = temp.path().join("clash");
        let refused = request(&mut service, "clash", &clash, "taken");
        assert_eq!(
            json(&mut service, refused)["error"]["code"],
            "BRANCH_EXISTS"
        );
        assert!(!clash.exists());
        assert!(!repo.path().join("worktrees/clash").exists());
        // A new branch is created at the start commit and checked out there,
        // leaving the main worktree's HEAD where it was.
        let target = temp.path().join("agent-auth");
        let add = request(&mut service, "agent-auth", &target, "agent/auth-fix");
        let result = json(&mut service, add.clone());
        assert_eq!(result["state"], "succeeded", "{result}");
        assert_eq!(result["result"]["branchCreated"], true);
        let branch = repo
            .find_branch("agent/auth-fix", git2::BranchType::Local)
            .unwrap();
        assert_eq!(branch.get().target(), Some(tip.id()));
        let linked = Repository::open(&target).unwrap();
        assert_eq!(
            linked.head().unwrap().name().unwrap(),
            "refs/heads/agent/auth-fix"
        );
        assert_eq!(fs::read(target.join("a")).unwrap(), b"committed");
        assert_eq!(repo.head().unwrap().name().unwrap(), "refs/heads/master");
        // The journal answers a repeat with the recorded outcome, not a second
        // attempt that would now find the branch taken.
        assert_eq!(json(&mut service, add), result);
    }
    #[test]
    fn journaled_worktree_locks_use_listing_snapshots_and_replay_without_relocking() {
        let temp = tempfile::tempdir().unwrap();
        let repo = Repository::init(temp.path().join("main")).unwrap();
        fs::write(repo.workdir().unwrap().join("a"), "original").unwrap();
        commit(&repo, "initial");
        let linked_path = temp.path().join("linked");
        let linked = repo.worktree("linked", &linked_path, None).unwrap();
        fs::write(repo.workdir().unwrap().join("a"), "dirty").unwrap();
        let index_before = fs::read(repo.path().join("index")).unwrap();
        let journal = Journal::open(temp.path().join("journal"), id()).unwrap();
        let mut service = Service::with_journal(journal);
        let repo_id = open(&mut service, repo.workdir().unwrap());
        let listing = |service: &mut Service| {
            json(
                service,
                Request::Worktrees {
                    at_snapshot: None,
                    filter: String::new(),
                    branch: None,
                    name: None,
                    repo_id: repo_id.clone(),
                    page_size: 100,
                    cursor: None,
                },
            )
        };
        let before = listing(&mut service);
        let request = Request::Start {
            operation_id: id(),
            repo_id: repo_id.clone(),
            expected_snapshot: before["snapshot"].as_str().unwrap().into(),
            action: Action::WorktreeLock {
                name: "linked".into(),
                reason: Some("external drive".into()),
            },
        };
        let locked = json(&mut service, request.clone());
        assert_eq!(locked["state"], "succeeded", "{locked}");
        assert_eq!(
            listing(&mut service)["entries"][1]["lockReason"]["display"],
            "external drive"
        );
        assert!(
            matches!(service.request(Request::Start { operation_id: id(), repo_id: repo_id.clone(), expected_snapshot: before["snapshot"].as_str().unwrap().into(), action: Action::WorktreeUnlock { name: "linked".into() } }), Err(Error { code, .. }) if code == "STALE_SNAPSHOT")
        );
        linked.unlock().unwrap();
        assert_eq!(json(&mut service, request), locked);
        assert_eq!(listing(&mut service)["entries"][1]["locked"], false);
        let fresh = listing(&mut service);
        let lock = json(
            &mut service,
            Request::Start {
                operation_id: id(),
                repo_id: repo_id.clone(),
                expected_snapshot: fresh["snapshot"].as_str().unwrap().into(),
                action: Action::WorktreeLock {
                    name: "linked".into(),
                    reason: None,
                },
            },
        );
        assert_eq!(lock["state"], "succeeded");
        fs::rename(&linked_path, temp.path().join("moved")).unwrap();
        let fresh = listing(&mut service);
        let repeated = json(
            &mut service,
            Request::Start {
                operation_id: id(),
                repo_id: repo_id.clone(),
                expected_snapshot: fresh["snapshot"].as_str().unwrap().into(),
                action: Action::WorktreeLock {
                    name: "linked".into(),
                    reason: Some("replace".into()),
                },
            },
        );
        assert_eq!(repeated["error"]["code"], "WORKTREE_LOCKED");
        let unlock = Request::Start {
            operation_id: id(),
            repo_id: repo_id.clone(),
            expected_snapshot: fresh["snapshot"].as_str().unwrap().into(),
            action: Action::WorktreeUnlock {
                name: "linked".into(),
            },
        };
        let unlocked = json(&mut service, unlock.clone());
        assert_eq!(unlocked["state"], "succeeded");
        assert_eq!(json(&mut service, unlock), unlocked);
        assert_eq!(listing(&mut service)["entries"][1]["locked"], false);
        assert_eq!(fs::read(repo.path().join("index")).unwrap(), index_before);
        assert_eq!(
            fs::read(repo.workdir().unwrap().join("a")).unwrap(),
            b"dirty"
        );
        assert!(temp.path().join("moved/a").exists());
        let bare = git2::build::RepoBuilder::new()
            .bare(true)
            .clone(repo.path().to_str().unwrap(), &temp.path().join("bare.git"))
            .unwrap();
        bare.worktree("bare-linked", &temp.path().join("bare-linked"), None)
            .unwrap();
        let bare_id = open(&mut service, bare.path());
        let listing = json(
            &mut service,
            Request::Worktrees {
                at_snapshot: None,
                filter: String::new(),
                branch: None,
                name: None,
                repo_id: bare_id.clone(),
                page_size: 100,
                cursor: None,
            },
        );
        let result = json(
            &mut service,
            Request::Start {
                operation_id: id(),
                repo_id: bare_id,
                expected_snapshot: listing["snapshot"].as_str().unwrap().into(),
                action: Action::WorktreeLock {
                    name: "bare-linked".into(),
                    reason: None,
                },
            },
        );
        assert_eq!(result["state"], "succeeded", "{result}");
    }
    #[test]
    fn worktree_listing_handles_locks_missing_checkouts_and_broken_records() {
        let temp = tempfile::tempdir().unwrap();
        let main = Repository::init(temp.path().join("main")).unwrap();
        fs::write(main.workdir().unwrap().join("a"), "a").unwrap();
        commit(&main, "initial");
        let linked_path = temp.path().join("linked");
        let linked = main.worktree("linked", &linked_path, None).unwrap();
        linked.lock(Some("keep on external drive")).unwrap();
        let mut service = Service::default();
        let main_id = open(&mut service, main.workdir().unwrap());
        let request = |repo_id: String, cursor| Request::Worktrees {
            at_snapshot: None,
            filter: String::new(),
            branch: None,
            name: None,
            repo_id,
            page_size: 1,
            cursor,
        };
        let first = json(&mut service, request(main_id.clone(), None));
        assert_eq!(first["entries"][0]["kind"], "main");
        assert_eq!(first["entries"][0]["current"], true);
        let page = json(
            &mut service,
            request(
                main_id.clone(),
                Some(first["nextCursor"].as_str().unwrap().into()),
            ),
        );
        assert_eq!(page["entries"][0]["state"], "available");
        assert_eq!(page["entries"][0]["locked"], true);
        assert_eq!(
            page["entries"][0]["lockReason"]["display"],
            "keep on external drive"
        );
        assert_eq!(
            page["entries"][0]["head"]["name"]["display"],
            "refs/heads/linked"
        );
        let linked_id = open(&mut service, &linked_path);
        let from_linked = json(
            &mut service,
            Request::Worktrees {
                at_snapshot: None,
                filter: String::new(),
                branch: None,
                name: None,
                repo_id: linked_id,
                page_size: 100,
                cursor: None,
            },
        );
        assert_eq!(from_linked["entries"][0]["current"], false);
        assert_eq!(from_linked["entries"][1]["current"], true);
        // Read invalid UTF-8 reasons without invoking git2's panicking accessor.
        fs::write(main.path().join("worktrees/linked/locked"), b"reason-\xff").unwrap();
        fs::rename(&linked_path, temp.path().join("moved")).unwrap();
        fs::create_dir(main.path().join("worktrees/broken")).unwrap();
        let missing = json(
            &mut service,
            Request::Worktrees {
                at_snapshot: None,
                filter: String::new(),
                branch: None,
                name: None,
                repo_id: main_id.clone(),
                page_size: 100,
                cursor: None,
            },
        );
        assert_eq!(missing["entries"][1]["name"]["display"], "broken");
        assert_eq!(missing["entries"][1]["state"], "invalid");
        assert_eq!(missing["entries"][2]["state"], "missing");
        assert_eq!(missing["entries"][2]["locked"], true);
        assert_eq!(
            missing["entries"][2]["lockReason"]["bytesB64"],
            STANDARD.encode(b"reason-\xff")
        );
        assert_ne!(
            missing["metadata"]["listToken"],
            first["metadata"]["listToken"]
        );
        let saved = json(
            &mut service,
            request(main_id, Some(first["nextCursor"].as_str().unwrap().into())),
        );
        assert_eq!(saved, page);
        assert!(temp.path().join("moved/a").is_file());
        let locked_path = main.path().join("worktrees/linked/locked");
        fs::remove_file(&locked_path).unwrap();
        let fifo = std::ffi::CString::new(locked_path.as_os_str().as_bytes()).unwrap();
        // A special lock-reason file must not block listing on a native read.
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        let special = worktree_rows(&main).unwrap();
        let special = special
            .iter()
            .find(|r| r["name"]["display"] == "linked")
            .unwrap();
        assert_eq!(special["locked"], true);
        assert_eq!(special["lockReasonUnavailable"], true);
        assert_eq!(special["prunable"], false);
        fs::remove_file(locked_path).unwrap();

        let bare = Repository::init_bare(temp.path().join("bare.git")).unwrap();
        let bare_id = open(&mut service, bare.path());
        let bare = json(&mut service, request(bare_id, None));
        assert_eq!(bare["entries"][0]["kind"], "bare");
        assert_eq!(bare["entries"][0]["head"]["unborn"], true);
    }
    #[test]
    fn remote_reference_pages_are_anchored_and_select_the_push_destination() {
        let temp = tempfile::tempdir().unwrap();
        let source = Repository::init(temp.path().join("source")).unwrap();
        fs::write(source.workdir().unwrap().join("a"), "source").unwrap();
        commit(&source, "source");
        let tip = source.head().unwrap().peel_to_commit().unwrap();
        for i in 0..205 {
            source
                .branch(&format!("topic-{i:03}"), &tip, false)
                .unwrap();
        }
        let other = Repository::init(temp.path().join("push")).unwrap();
        fs::write(other.workdir().unwrap().join("a"), "push").unwrap();
        commit(&other, "push");
        other
            .branch(
                "push-only",
                &other.head().unwrap().peel_to_commit().unwrap(),
                false,
            )
            .unwrap();
        let local = Repository::init(temp.path().join("local")).unwrap();
        local
            .remote("origin", source.path().to_str().unwrap())
            .unwrap();
        local
            .remote_set_pushurl("origin", Some(other.path().to_str().unwrap()))
            .unwrap();
        let config_before = fs::read(local.path().join("config")).unwrap();
        let token = super::super::remotes::list(&local).unwrap()["entries"][0]["token"]
            .as_str()
            .unwrap()
            .to_owned();
        let mut service = Service::default();
        let repo_id = open(&mut service, local.workdir().unwrap());
        let request = |cursor, for_push, expected_token: String| Request::RemoteRefs {
            filter: String::new(),
            repo_id: repo_id.clone(),
            remote: "origin".into(),
            expected_token,
            for_push,
            page_size: 100,
            cursor,
        };
        let first = json(&mut service, request(None, false, token.clone()));
        assert_eq!(first["entries"].as_array().unwrap().len(), 100);
        source
            .find_reference("refs/heads/topic-204")
            .unwrap()
            .delete()
            .unwrap();
        let cursor = first["nextCursor"].as_str().unwrap().to_owned();
        let mut filtered_request = request(Some(cursor.clone()), false, token.clone());
        if let Request::RemoteRefs { filter, .. } = &mut filtered_request {
            *filter = "topic".into();
        }
        assert!(service.request(filtered_request).is_err());
        assert!(service
            .request(request(Some(cursor.clone()), true, token.clone()))
            .is_err());
        assert!(service
            .request(request(Some(cursor.clone()), false, "changed".into()))
            .is_err());
        let second = json(&mut service, request(Some(cursor), false, token.clone()));
        let third = json(
            &mut service,
            request(
                Some(second["nextCursor"].as_str().unwrap().into()),
                false,
                token.clone(),
            ),
        );
        assert_eq!(first["snapshot"], third["snapshot"]);
        assert!(third["nextCursor"].is_null());
        assert!(third["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["reference"]["display"] == "refs/heads/topic-204"));
        let push = json(&mut service, request(None, true, token.clone()));
        assert!(push["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["reference"]["display"] == "refs/heads/push-only"));
        assert!(!push["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["reference"]["display"] == "refs/heads/topic-000"));
        local
            .remote_set_url("origin", other.path().to_str().unwrap())
            .unwrap();
        let err = service.request(request(None, false, token));
        assert!(matches!(err, Err(Error { code, .. }) if code == "STALE_REMOTE"));
        local
            .remote_set_url("origin", source.path().to_str().unwrap())
            .unwrap();
        assert_eq!(
            fs::read(local.path().join("config")).unwrap(),
            config_before
        );
        assert_eq!(local.references().unwrap().count(), 0);
        assert!(local.find_commit(tip.id()).is_err());
    }
    #[test]
    fn index_operations_preserve_files_and_replay_across_sessions() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = Repository::init(tmp.path()).unwrap();
        fs::write(tmp.path().join("a"), "before").unwrap();
        fs::write(tmp.path().join("b"), "before").unwrap();
        commit(&repo, "initial");
        fs::write(tmp.path().join("a"), "after").unwrap();
        fs::write(tmp.path().join("b"), "other").unwrap();
        let records = tempfile::tempdir().unwrap();
        let journal = Journal::open(records.path().join("journal"), id()).unwrap();
        let mut service = Service::with_journal(journal.clone());
        let repo_id = open(&mut service, tmp.path());
        let status = json(
            &mut service,
            Request::Status {
                filter: None,
                repo_id: repo_id.clone(),
                page_size: 100,
                cursor: None,
            },
        );
        let entry = status["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["path"]["display"] == "a")
            .unwrap();
        let operation_id = id();
        let request = Request::Start {
            operation_id: operation_id.clone(),
            repo_id,
            expected_snapshot: status["snapshot"].as_str().unwrap().into(),
            action: Action::Stage {
                hunks: None,
                entry_ids: vec![entry["entryId"].as_str().unwrap().into()],
            },
        };
        let result = json(&mut service, request.clone());
        assert_eq!(result["state"], "succeeded", "{result}");
        let fresh = Repository::open(tmp.path()).unwrap();
        assert!(fresh
            .status_file(Path::new("a"))
            .unwrap()
            .is_index_modified());
        assert!(fresh.status_file(Path::new("b")).unwrap().is_wt_modified());
        let index_before = fs::read(repo.path().join("index")).unwrap();
        let mut reconnected = Service::with_journal(journal);
        let new_id = open(&mut reconnected, tmp.path());
        let mut replay = request;
        if let Request::Start { repo_id, .. } = &mut replay {
            *repo_id = new_id.clone();
        }
        assert_eq!(json(&mut reconnected, replay), result);
        assert_eq!(fs::read(repo.path().join("index")).unwrap(), index_before);
        let status = json(
            &mut reconnected,
            Request::Status {
                filter: None,
                repo_id: new_id.clone(),
                page_size: 100,
                cursor: None,
            },
        );
        let entry = status["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["path"]["display"] == "a")
            .unwrap();
        let unstage = Request::Start {
            operation_id: id(),
            repo_id: new_id,
            expected_snapshot: status["snapshot"].as_str().unwrap().into(),
            action: Action::Unstage {
                hunks: None,
                entry_ids: vec![entry["entryId"].as_str().unwrap().into()],
            },
        };
        assert_eq!(json(&mut reconnected, unstage)["state"], "succeeded");
        assert_eq!(fs::read(tmp.path().join("a")).unwrap(), b"after");
        assert_eq!(fs::read(tmp.path().join("b")).unwrap(), b"other");
        let fresh = Repository::open(tmp.path()).unwrap();
        assert!(!fresh
            .status_file(Path::new("a"))
            .unwrap()
            .is_index_modified());
    }

    #[test]
    fn real_status_diff_stale_snapshot_and_lossless_names() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = Repository::init(tmp.path()).unwrap();
        fs::write(tmp.path().join("tracked.txt"), "before\n").unwrap();
        commit(&repo, "initial");
        fs::write(tmp.path().join("tracked.txt"), "after\n").unwrap();
        #[cfg(target_os = "linux")]
        let unusual: &[u8] = b"odd\n\xff.txt";
        #[cfg(not(target_os = "linux"))]
        let unusual: &[u8] = b"odd\nname.txt";
        fs::write(tmp.path().join(OsStr::from_bytes(unusual)), "new\n").unwrap();
        let mut service = Service::default();
        let repo_id = open(&mut service, tmp.path());
        let index_before = fs::read(repo.path().join("index")).unwrap();
        let status = json(
            &mut service,
            Request::Status {
                filter: None,
                repo_id: repo_id.clone(),
                page_size: 100,
                cursor: None,
            },
        );
        assert_eq!(status["entries"].as_array().unwrap().len(), 2);
        let entries = status["entries"].as_array().unwrap();
        assert!(entries.iter().any(|e| STANDARD
            .decode(e["path"]["bytesB64"].as_str().unwrap())
            .unwrap()
            == unusual));
        let entry = entries
            .iter()
            .find(|e| e["path"]["display"] == "tracked.txt")
            .unwrap();
        let request = Request::Diff {
            repo_id: repo_id.clone(),
            snapshot: status["snapshot"].as_str().unwrap().into(),
            entry_id: entry["entryId"].as_str().unwrap().into(),
            side: Side::IndexToWorktree,
            context_lines: 3,
        };
        let Output::Diff { bytes, .. } = service.request(request.clone()).unwrap() else {
            panic!()
        };
        let diff: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(diff["truncated"], false);
        assert!(String::from_utf8(bytes).unwrap().contains("after"));
        assert_eq!(
            index_before,
            fs::read(repo.path().join("index")).unwrap(),
            "reads never update the index"
        );
        fs::write(tmp.path().join("tracked.txt"), "changed again\n").unwrap();
        assert!(matches!(service.request(request),Err(Error {code,..}) if code=="STALE_SNAPSHOT"));
    }
    #[test]
    fn commit_files_pages_are_anchored_and_reject_other_comparisons() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = Repository::init(tmp.path()).unwrap();
        for i in 0..205 {
            fs::write(
                tmp.path().join(format!("file-{i:03}")),
                format!("contents {i}\n"),
            )
            .unwrap();
        }
        commit(&repo, "many files");
        let selected = repo.head().unwrap().target().unwrap().to_string();
        let mut service = Service::default();
        let repo_id = open(&mut service, tmp.path());
        let request = |cursor| Request::CommitFiles {
            repo_id: repo_id.clone(),
            commit_oid: selected.clone(),
            parent_index: 0,
            page_size: 100,
            cursor,
        };
        let first = json(&mut service, request(None));
        assert_eq!(first["entries"].as_array().unwrap().len(), 100);
        assert_eq!(first["metadata"]["totalFiles"], 205);
        assert_eq!(first["metadata"]["truncated"], false);
        assert!(first["entries"][0].get("hunks").is_none());
        fs::write(tmp.path().join("new"), "new").unwrap();
        commit(&repo, "move HEAD");
        let cursor = first["nextCursor"].as_str().unwrap().to_owned();
        assert_eq!(
            service
                .request(Request::CommitFiles {
                    repo_id: repo_id.clone(),
                    commit_oid: repo.head().unwrap().target().unwrap().to_string(),
                    parent_index: 0,
                    page_size: 100,
                    cursor: Some(cursor.clone())
                })
                .err()
                .unwrap()
                .code,
            "INVALID_REQUEST"
        );
        let second = json(&mut service, request(Some(cursor)));
        let third = json(
            &mut service,
            request(Some(second["nextCursor"].as_str().unwrap().into())),
        );
        assert_eq!(first["snapshot"], second["snapshot"]);
        assert_eq!(second["snapshot"], third["snapshot"]);
        assert_eq!(third["entries"].as_array().unwrap().len(), 5);
        assert!(third["nextCursor"].is_null());
        let paths = [first, second, third]
            .into_iter()
            .flat_map(|p| {
                p["entries"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|e| e["newPath"]["display"].as_str().unwrap().to_owned())
                    .collect::<Vec<_>>()
            })
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(paths.len(), 205);
        assert!(!paths.contains("new"));
    }
    #[test]
    fn commit_files_can_continue_past_old_truncation_limit_after_reconnect() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = Repository::init_bare(tmp.path()).unwrap();
        let mut tree = repo.treebuilder(None).unwrap();
        let blob = repo.blob(b"contents\n").unwrap();
        for index in 0..50_005 {
            tree.insert(format!("file-{index:05}"), blob, 0o100644)
                .unwrap();
        }
        let tree = repo.find_tree(tree.write().unwrap()).unwrap();
        let sig = git2::Signature::now("Test", "test@example.test").unwrap();
        let commit = repo
            .commit(Some("HEAD"), &sig, &sig, "large", &tree, &[])
            .unwrap();
        let mut service = Service::default();
        let repo_id = open(&mut service, tmp.path());
        let request = |cursor| Request::CommitFiles {
            repo_id: repo_id.clone(),
            commit_oid: commit.to_string(),
            parent_index: 0,
            page_size: 100,
            cursor,
        };
        let first = json(&mut service, request(None));
        assert_eq!(first["metadata"]["totalFiles"], 50_005);
        assert_eq!(first["metadata"]["truncated"], false);
        let mut cursor = CursorRef::decode(first["nextCursor"].as_str().unwrap()).unwrap();
        cursor.o = 50_000;
        let last = json(&mut Service::default(), request(Some(cursor.encode())));
        assert_eq!(last["entries"].as_array().unwrap().len(), 5);
        assert_eq!(last["entries"][0]["newPath"]["display"], "file-50000");
        assert_eq!(last["snapshot"], first["snapshot"]);
        assert!(last["nextCursor"].is_null());
    }
    #[test]
    fn commit_files_byte_budget_preserves_all_rows() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = Repository::init_bare(tmp.path()).unwrap();
        let mut tree = repo.treebuilder(None).unwrap();
        let blob = repo.blob(b"contents\n").unwrap();
        for index in 0..205 {
            tree.insert(
                format!("file-{index:03}-{}", "x".repeat(2000)),
                blob,
                0o100644,
            )
            .unwrap();
        }
        let tree = repo.find_tree(tree.write().unwrap()).unwrap();
        let sig = git2::Signature::now("Test", "test@example.test").unwrap();
        let commit = repo
            .commit(Some("HEAD"), &sig, &sig, "wide paths", &tree, &[])
            .unwrap();
        let mut service = Service::default();
        let repo_id = open(&mut service, tmp.path());
        let mut cursor = None;
        let mut seen = std::collections::BTreeSet::new();
        let mut pages = 0;
        loop {
            let page = json(
                &mut service,
                Request::CommitFiles {
                    repo_id: repo_id.clone(),
                    commit_oid: commit.to_string(),
                    parent_index: 0,
                    page_size: 200,
                    cursor,
                },
            );
            assert!(serde_json::to_vec(&page).unwrap().len() < MAX_FRAME);
            let rows = page["entries"].as_array().unwrap();
            assert!(!rows.is_empty());
            for row in rows {
                assert!(seen.insert(row["newPath"]["bytesB64"].as_str().unwrap().to_owned()));
            }
            pages += 1;
            cursor = page["nextCursor"].as_str().map(str::to_owned);
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(seen.len(), 205);
        assert!(pages > 2);
    }
    #[test]
    fn selected_history_file_is_reachable_beyond_whole_diff_file_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = Repository::init_bare(tmp.path()).unwrap();
        let mut builder = repo.treebuilder(None).unwrap();
        let blob = repo.blob(b"content\n").unwrap();
        for i in 0..10_002 {
            builder
                .insert(format!("file-{i:05}"), blob, 0o100644)
                .unwrap();
        }
        let tree = repo.find_tree(builder.write().unwrap()).unwrap();
        let sig = git2::Signature::now("Test", "test@example.test").unwrap();
        let id = repo.commit(None, &sig, &sig, "large", &tree, &[]).unwrap();
        let diff = commit_diff(&repo, &id.to_string(), 0, 3, Some(b"file-10001")).unwrap();
        assert_eq!(diff["files"].as_array().unwrap().len(), 1);
        assert_eq!(diff["files"][0]["newPath"]["display"], "file-10001");
        assert_eq!(diff["truncated"], false);
    }
    #[test]
    fn commit_diff_path_is_exact_and_lossless() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = Repository::init(tmp.path()).unwrap();
        for name in ["[x]*.txt", "x.txt", "other.txt"] {
            fs::write(tmp.path().join(name), name).unwrap();
        }
        commit(&repo, "files");
        let oid = repo.head().unwrap().target().unwrap().to_string();
        let mut service = Service::default();
        let repo_id = open(&mut service, tmp.path());
        let Output::Diff { bytes, .. } = service
            .request(Request::CommitDiff {
                repo_id,
                commit_oid: oid.clone(),
                parent_index: 0,
                context_lines: 3,
                path: Some(WirePath::new(b"[x]*.txt")),
            })
            .unwrap()
        else {
            panic!()
        };
        let diff: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(diff["files"].as_array().unwrap().len(), 1);
        assert_eq!(diff["files"][0]["newPath"]["display"], "[x]*.txt");
        assert_eq!(
            commit_diff(&repo, &oid, 0, 3, Some(b"*.txt"))
                .unwrap_err()
                .code,
            "FILE_NOT_CHANGED"
        );
    }
    #[test]
    fn commit_diff_root_is_immutable_and_does_not_require_status() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = Repository::init(tmp.path()).unwrap();
        fs::write(tmp.path().join("a"), "first\nsecond\n").unwrap();
        commit(&repo, "initial");
        let root = repo.head().unwrap().target().unwrap().to_string();
        let mut service = Service::default();
        let repo_id = open(&mut service, tmp.path());
        let request = Request::CommitDiff {
            path: None,
            repo_id,
            commit_oid: root.clone(),
            parent_index: 0,
            context_lines: 3,
        };
        let Output::Diff { snapshot, bytes } = service.request(request.clone()).unwrap() else {
            panic!()
        };
        assert_eq!(snapshot, root);
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(value["parentOid"].is_null());
        assert!(value["parentIndex"].is_null());
        assert_eq!(value["files"][0]["status"], "Added");
        assert_eq!(value["files"][0]["additions"], 2);
        assert!(value["files"][0]["oldOid"].is_null());
        assert!(value["files"][0]["newOid"]["hex"].is_string());
        fs::write(tmp.path().join("a"), "later\n").unwrap();
        commit(&repo, "later");
        fs::write(tmp.path().join("a"), "uncommitted\n").unwrap();
        fs::write(tmp.path().join(".gitattributes"), "a -diff\n").unwrap();
        let index = fs::read(repo.path().join("index")).unwrap();
        let Output::Diff { bytes: again, .. } = service.request(request).unwrap() else {
            panic!()
        };
        assert_eq!(bytes, again);
        assert_eq!(fs::read(repo.path().join("index")).unwrap(), index);
        assert_eq!(fs::read(tmp.path().join("a")).unwrap(), b"uncommitted\n");
    }
    #[test]
    fn commit_diff_selects_merge_parent_in_bare_repository() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = Repository::init_bare(tmp.path()).unwrap();
        let sig = git2::Signature::now("Test", "test@example.test").unwrap();
        let create = |parents: &[git2::Oid], names: &[&str]| {
            let mut builder = repo.treebuilder(None).unwrap();
            for name in names {
                builder
                    .insert(
                        *name,
                        repo.blob(format!("{name}\n").as_bytes()).unwrap(),
                        0o100644,
                    )
                    .unwrap();
            }
            let tree = repo.find_tree(builder.write().unwrap()).unwrap();
            let parents = parents
                .iter()
                .map(|id| repo.find_commit(*id).unwrap())
                .collect::<Vec<_>>();
            repo.commit(
                None,
                &sig,
                &sig,
                "commit",
                &tree,
                &parents.iter().collect::<Vec<_>>(),
            )
            .unwrap()
        };
        let base = create(&[], &["base"]);
        let left = create(&[base], &["base", "left"]);
        let right = create(&[base], &["base", "right"]);
        let merged = create(&[left, right], &["base", "left", "right"]);
        let left_diff = commit_diff(&repo, &merged.to_string(), 0, 3, None).unwrap();
        let right_diff = commit_diff(&repo, &merged.to_string(), 1, 3, None).unwrap();
        assert_eq!(left_diff["parentOid"]["hex"], left.to_string());
        assert_eq!(right_diff["parentOid"]["hex"], right.to_string());
        assert_eq!(left_diff["files"][0]["newPath"]["display"], "right");
        assert_eq!(right_diff["files"][0]["newPath"]["display"], "left");
        assert_eq!(
            commit_diff(&repo, &merged.to_string(), 2, 3, None)
                .unwrap_err()
                .code,
            "INVALID_REQUEST"
        );
        assert_eq!(
            commit_diff(&repo, &base.to_string(), 1, 3, None)
                .unwrap_err()
                .code,
            "INVALID_REQUEST"
        );
        assert_eq!(
            commit_diff(&repo, &base.to_string(), 0, 101, None)
                .unwrap_err()
                .code,
            "INVALID_REQUEST"
        );
    }
    #[test]
    fn selected_diff_matches_full_comparison_for_paths_types_attributes_and_parents() {
        let temp = tempfile::tempdir().unwrap();
        let repo = Repository::init_bare(temp.path()).unwrap();
        let sig = git2::Signature::now("Test", "test@example.test").unwrap();
        let make = |parents: &[git2::Oid], changed: bool| {
            let mut builder = repo.treebuilder(None).unwrap();
            let content: &[u8] = if changed { b"after\n" } else { b"before\n" };
            for path in [
                b"selected".as_slice(),
                b"[x]*.txt",
                b"nonutf8-\xff",
                b"binary.data",
            ] {
                builder
                    .insert(path, repo.blob(content).unwrap(), 0o100644)
                    .unwrap();
            }
            builder
                .insert(
                    ".gitattributes",
                    repo.blob(b"*.data -diff\n").unwrap(),
                    0o100644,
                )
                .unwrap();
            builder
                .insert(
                    "executable",
                    repo.blob(b"same\n").unwrap(),
                    if changed { 0o100755 } else { 0o100644 },
                )
                .unwrap();
            builder
                .insert("link", repo.blob(content).unwrap(), 0o120000)
                .unwrap();
            builder
                .insert(
                    "type-change",
                    repo.blob(content).unwrap(),
                    if changed { 0o120000 } else { 0o100644 },
                )
                .unwrap();
            builder
                .insert(
                    if changed {
                        "renamed-new"
                    } else {
                        "renamed-old"
                    },
                    repo.blob(b"unchanged rename content\n").unwrap(),
                    0o100644,
                )
                .unwrap();
            let tree = repo.find_tree(builder.write().unwrap()).unwrap();
            let parents = parents
                .iter()
                .map(|id| repo.find_commit(*id).unwrap())
                .collect::<Vec<_>>();
            repo.commit(
                None,
                &sig,
                &sig,
                "comparison",
                &tree,
                &parents.iter().collect::<Vec<_>>(),
            )
            .unwrap()
        };
        let base = make(&[], false);
        let alternate = make(&[base], false);
        let tip = make(&[base, alternate], true);
        for parent in 0..2 {
            for path in [
                b"selected".as_slice(),
                b"[x]*.txt",
                b"nonutf8-\xff",
                b"binary.data",
                b"executable",
                b"link",
                b"type-change",
                b"renamed-new",
                b"renamed-old",
            ] {
                let full = commit_comparison(&repo, &tip.to_string(), parent, 3, |diff| {
                    diff_value(diff, Some(path))
                })
                .unwrap();
                let selected = commit_diff(&repo, &tip.to_string(), parent, 3, Some(path)).unwrap();
                assert_eq!(selected, full, "path {:?}, parent {parent}", path);
            }
        }
        let before = repo.find_commit(base).unwrap().tree().unwrap();
        let after = repo.find_commit(tip).unwrap().tree().unwrap();
        for path in [
            b"selected".as_slice(),
            b"[x]*.txt",
            b"nonutf8-\xff",
            b"executable",
            b"link",
        ] {
            assert!(same_blob_type(Some(&before), &after, path));
        }
        for path in [
            b"type-change".as_slice(),
            b"renamed-new",
            b"renamed-old",
            b"missing",
        ] {
            assert!(!same_blob_type(Some(&before), &after, path));
        }
    }

    #[test]
    fn commit_diff_reports_renames_binary_blobs_and_mode_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = Repository::init(tmp.path()).unwrap();
        fs::write(tmp.path().join("old"), "rename contents\n").unwrap();
        fs::write(tmp.path().join("binary"), b"old\0binary").unwrap();
        fs::write(tmp.path().join("script"), "script\n").unwrap();
        commit(&repo, "base");
        let parent = repo.head().unwrap().peel_to_commit().unwrap();
        let tree = parent.tree().unwrap();
        let mut builder = repo.treebuilder(Some(&tree)).unwrap();
        let renamed = tree.get_name("old").unwrap().id();
        builder.remove("old").unwrap();
        builder.insert("new", renamed, 0o100644).unwrap();
        builder
            .insert("binary", repo.blob(b"new\0binary").unwrap(), 0o100644)
            .unwrap();
        builder
            .insert("script", tree.get_name("script").unwrap().id(), 0o100755)
            .unwrap();
        let tree = repo.find_tree(builder.write().unwrap()).unwrap();
        let sig = git2::Signature::now("Test", "test@example.test").unwrap();
        let id = repo
            .commit(None, &sig, &sig, "changes", &tree, &[&parent])
            .unwrap();
        let value = commit_diff(&repo, &id.to_string(), 0, 3, None).unwrap();
        let files = value["files"].as_array().unwrap();
        let rename = files.iter().find(|f| f["status"] == "Renamed").unwrap();
        assert_eq!(rename["oldPath"]["display"], "old");
        assert_eq!(rename["newPath"]["display"], "new");
        let by_old = commit_diff(&repo, &id.to_string(), 0, 3, Some(b"old")).unwrap();
        let by_new = commit_diff(&repo, &id.to_string(), 0, 3, Some(b"new")).unwrap();
        assert_eq!(by_old, by_new);
        assert_eq!(by_new["files"].as_array().unwrap().len(), 1);
        assert_eq!(by_new["files"][0]["status"], "Renamed");
        let binary = files
            .iter()
            .find(|f| f["newPath"]["display"] == "binary")
            .unwrap();
        assert_eq!(binary["binary"], true);
        assert!(binary["hunks"].as_array().unwrap().is_empty());
        let mode = files
            .iter()
            .find(|f| f["newPath"]["display"] == "script")
            .unwrap();
        assert_eq!(mode["oldMode"], 0o100644);
        assert_eq!(mode["newMode"], 0o100755);
        assert_eq!(
            commit_diff(&repo, &renamed.to_string(), 0, 3, None)
                .unwrap_err()
                .code,
            "COMMIT_NOT_FOUND"
        );
    }
    #[test]
    fn history_pages_obey_byte_budget_and_replay_without_loss() {
        let temp = tempfile::tempdir().unwrap();
        let repo = Repository::init(temp.path()).unwrap();
        fs::write(temp.path().join("a"), "a").unwrap();
        for i in 0..40 {
            commit(&repo, &format!("{i} {}", "m".repeat(16384)));
        }
        let mut service = Service::default();
        let repo_id = open(&mut service, temp.path());
        let request = |cursor| Request::History {
            message_bytes: None,
            repo_id: repo_id.clone(),
            revision: "HEAD".into(),
            page_size: 200,
            cursor,
        };
        let mut cursor = None;
        let mut seen = Vec::new();
        loop {
            // Reconnect on every page to exercise stateless continuation.
            let value = json(&mut Service::default(), request(cursor));
            assert!(serde_json::to_vec(&value).unwrap().len() < MAX_FRAME);
            assert_eq!(value["metadata"]["truncated"], false);
            seen.extend(
                value["entries"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|entry| entry["oid"]["hex"].as_str().unwrap().to_owned()),
            );
            cursor = value["nextCursor"].as_str().map(str::to_owned);
            if cursor.is_none() {
                break;
            }
        }
        let mut walk = repo.revwalk().unwrap();
        walk.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::TIME)
            .unwrap();
        walk.push_head().unwrap();
        assert_eq!(
            seen,
            walk.map(|oid| oid.unwrap().to_string()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn sha256_history_cursor_survives_reconnect() {
        let temp = tempfile::tempdir().unwrap();
        let mut options = git2::RepositoryInitOptions::new();
        options.object_format(git2::ObjectFormat::Sha256);
        let repo = Repository::init_opts(temp.path(), &options).unwrap();
        fs::write(temp.path().join("a"), "a").unwrap();
        commit(&repo, "one");
        let base = repo.head().unwrap().target().unwrap();
        commit(&repo, "two");
        let mut service = Service::default();
        let repo_id = open(&mut service, temp.path());
        let request = |cursor| Request::History {
            message_bytes: None,
            repo_id: repo_id.clone(),
            revision: "HEAD".into(),
            page_size: 1,
            cursor,
        };
        let first = json(&mut service, request(None));
        let last = json(
            &mut Service::default(),
            request(first["nextCursor"].as_str().map(str::to_owned)),
        );
        assert_eq!(last["entries"][0]["oid"]["hex"], base.to_string());
        assert_eq!(last["entries"][0]["oid"]["algorithm"], "sha256");
        assert!(last["nextCursor"].is_null());
    }

    #[test]
    fn history_cursor_rejects_changed_shallow_boundaries() {
        let temp = tempfile::tempdir().unwrap();
        let repo = Repository::init(temp.path()).unwrap();
        fs::write(temp.path().join("a"), "a").unwrap();
        commit(&repo, "one");
        let base = repo.head().unwrap().target().unwrap();
        commit(&repo, "two");
        let mut service = Service::default();
        let repo_id = open(&mut service, temp.path());
        let request = |cursor| Request::History {
            message_bytes: None,
            repo_id: repo_id.clone(),
            revision: "HEAD".into(),
            page_size: 1,
            cursor,
        };
        let value = json(&mut service, request(None));
        fs::write(repo.path().join("shallow"), format!("{base}\n")).unwrap();
        assert_eq!(
            service
                .request(request(value["nextCursor"].as_str().map(str::to_owned)))
                .err()
                .unwrap()
                .code,
            "SNAPSHOT_EXPIRED"
        );
    }

    #[test]
    fn history_pages_remain_anchored_after_new_commits() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = Repository::init(tmp.path()).unwrap();
        for i in 0..3 {
            fs::write(tmp.path().join("a"), i.to_string()).unwrap();
            commit(&repo, &format!("commit {i}"));
        }
        let mut service = Service::default();
        let repo_id = open(&mut service, tmp.path());
        let request = |cursor| Request::History {
            message_bytes: None,
            repo_id: repo_id.clone(),
            page_size: 1,
            cursor,
            revision: "HEAD".into(),
        };
        let first = json(&mut service, request(None));
        let cursor = first["nextCursor"].as_str().unwrap().to_owned();
        commit(&repo, "new commit");
        let second = json(&mut service, request(Some(cursor.clone())));
        assert_eq!(second["entries"][0]["message"]["display"], "commit 1");
        let repeated = json(&mut service, request(Some(cursor.clone())));
        assert_eq!(second, repeated);
        assert!(
            matches!(service.request(Request::Branches { filter: String::new(), branch_kind: None,repo_id:repo_id.clone(),page_size:1,cursor:Some(cursor)}),Err(Error {code,..}) if code=="INVALID_REQUEST")
        );
        // Stateless: a process that never saw the first page -- a reconnect, a
        // second channel -- continues the same cursor from the same anchored
        // commit, rather than finding it expired.
        let mut elsewhere = Service::default();
        let third = json(
            &mut elsewhere,
            request(second["nextCursor"].as_str().map(str::to_owned)),
        );
        assert_eq!(third["entries"][0]["message"]["display"], "commit 0");
        assert!(third["nextCursor"].is_null());
    }
    #[test]
    fn unborn_bare_and_replaced_repositories() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("repo");
        let repo = Repository::init(&path).unwrap();
        let mut service = Service::default();
        let repo_id = open(&mut service, &path);
        assert_eq!(
            json(
                &mut service,
                Request::History {
                    message_bytes: None,
                    repo_id: repo_id.clone(),
                    page_size: 10,
                    cursor: None,
                    revision: "HEAD".into()
                }
            )["entries"],
            json!([])
        );
        let gitdir = repo.path().to_owned();
        drop(repo);
        fs::rename(&gitdir, tmp.path().join("old-git")).unwrap();
        Repository::init(&path).unwrap();
        assert!(
            matches!(service.request(Request::Branches { filter: String::new(), branch_kind: None,repo_id,page_size:10,cursor:None}),Err(Error {code,..}) if code=="REPO_REPLACED")
        );
        let bare_path = tmp.path().join("bare");
        Repository::init_bare(&bare_path).unwrap();
        let bare = open(&mut service, &bare_path);
        assert!(matches!(service.request(Request::Status {
filter: None,repo_id:bare,page_size:10,cursor:None}),Err(Error {code,..}) if code=="UNSUPPORTED_CAPABILITY"));
    }
    #[test]
    fn deleted_parent_and_staged_rename_can_be_inspected() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = Repository::init(tmp.path()).unwrap();
        fs::create_dir(tmp.path().join("folder")).unwrap();
        fs::write(tmp.path().join("folder/file"), "tracked content\n").unwrap();
        commit(&repo, "initial");
        fs::rename(tmp.path().join("folder/file"), tmp.path().join("renamed")).unwrap();
        fs::remove_dir(tmp.path().join("folder")).unwrap();
        let mut service = Service::default();
        let repo_id = open(&mut service, tmp.path());
        let status = json(
            &mut service,
            Request::Status {
                filter: None,
                repo_id: repo_id.clone(),
                page_size: 100,
                cursor: None,
            },
        );
        assert!(!status["entries"].as_array().unwrap().is_empty());
        let mut index = repo.index().unwrap();
        index.remove_path(Path::new("folder/file")).unwrap();
        index.add_path(Path::new("renamed")).unwrap();
        index.write().unwrap();
        let status = json(
            &mut service,
            Request::Status {
                filter: None,
                repo_id: repo_id.clone(),
                page_size: 100,
                cursor: None,
            },
        );
        let entry = &status["entries"][0];
        assert_eq!(entry["staged"], true);
        assert_eq!(entry["path"]["display"], "renamed");
        let output = service
            .request(Request::Diff {
                repo_id,
                snapshot: status["snapshot"].as_str().unwrap().into(),
                entry_id: entry["entryId"].as_str().unwrap().into(),
                side: Side::HeadToIndex,
                context_lines: 3,
            })
            .unwrap();
        let Output::Diff { bytes, .. } = output else {
            panic!()
        };
        let diff: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(diff["files"].as_array().unwrap().len(), 1);
        assert_eq!(diff["files"][0]["status"], "Renamed");
    }

    /// Every identifier the agent reports must describe the repository it came
    /// from, so a SHA-256 project is readable without any SHA-1 assumption.
    #[test]
    fn sha256_repositories_report_their_own_object_format() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        let mut options = git2::RepositoryInitOptions::new();
        options.object_format(git2::ObjectFormat::Sha256);
        let repo = Repository::init_opts(&root, &options).unwrap();
        assert_eq!(repo.object_format(), git2::ObjectFormat::Sha256);
        fs::write(root.join("file"), "one\ntwo\n").unwrap();
        commit(&repo, "initial");
        fs::write(root.join("file"), "one\nchanged\n").unwrap();
        fs::write(root.join("extra"), "new\n").unwrap();

        let mut service =
            Service::with_journal(Journal::open(temp.path().join("journal"), id()).unwrap());
        let repo_id = open(&mut service, &root);
        let opened = json(
            &mut service,
            Request::Open {
                path: wire_path(&root),
            },
        );
        assert_eq!(opened["objectFormat"], "sha256");
        let head = opened["head"]["oid"].clone();
        assert_eq!(head["algorithm"], "sha256");
        assert_eq!(head["hex"].as_str().unwrap().len(), 64);

        // Reads that carry identifiers must agree with the repository.
        let history = json(
            &mut service,
            Request::History {
                message_bytes: None,
                repo_id: repo_id.clone(),
                revision: "HEAD".into(),
                page_size: 10,
                cursor: None,
            },
        );
        assert_eq!(history["entries"][0]["oid"]["algorithm"], "sha256");
        let detail = json(
            &mut service,
            Request::Commit {
                repo_id: repo_id.clone(),
                commit_oid: history["entries"][0]["oid"]["hex"].as_str().unwrap().into(),
            },
        );
        assert_eq!(detail, history["entries"][0]);

        assert_eq!(
            history["entries"][0]["oid"]["hex"].as_str().unwrap().len(),
            64
        );
        let branches = json(
            &mut service,
            Request::Branches {
                filter: String::new(),
                branch_kind: None,
                repo_id: repo_id.clone(),
                page_size: 10,
                cursor: None,
            },
        );
        assert_eq!(branches["entries"][0]["oid"]["algorithm"], "sha256");
        let status = json(
            &mut service,
            Request::Status {
                filter: None,
                repo_id: repo_id.clone(),
                page_size: 10,
                cursor: None,
            },
        );
        let entries = status["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 2, "{status}");
        // Diffs carry blob identifiers of the repository's own width.
        let entry = entries
            .iter()
            .find(|e| e["path"]["display"] == "file")
            .unwrap();
        let Output::Diff { bytes, .. } = service
            .request(Request::Diff {
                repo_id,
                snapshot: status["snapshot"].as_str().unwrap().into(),
                entry_id: entry["entryId"].as_str().unwrap().into(),
                side: Side::IndexToWorktree,
                context_lines: 3,
            })
            .unwrap()
        else {
            panic!("expected a streamed diff")
        };
        let diff: Value = serde_json::from_slice(&bytes).unwrap();
        let file = diff["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["newPath"]["display"] == "file")
            .unwrap_or_else(|| panic!("{diff}"));
        assert_eq!(file["oldOid"]["algorithm"], "sha256");
        assert_eq!(file["oldOid"]["hex"].as_str().unwrap().len(), 64);
        // Hunk identifiers stay 64 hex characters and are a different field, so
        // they are never confused with an object ID of the same width.
        assert_eq!(file["hunks"][0]["id"].as_str().unwrap().len(), 64);
    }

    #[test]
    fn sha1_repositories_still_report_sha1() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        let repo = Repository::init(&root).unwrap();
        fs::write(root.join("file"), "one\n").unwrap();
        commit(&repo, "initial");
        let mut service =
            Service::with_journal(Journal::open(temp.path().join("journal"), id()).unwrap());
        let opened = json(
            &mut service,
            Request::Open {
                path: wire_path(&root),
            },
        );
        assert_eq!(opened["objectFormat"], "sha1");
        assert_eq!(opened["head"]["oid"]["algorithm"], "sha1");
        assert_eq!(opened["head"]["oid"]["hex"].as_str().unwrap().len(), 40);
    }

    #[test]
    fn status_pages_cover_all_entries_and_stay_fingerprintable() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        let repo = Repository::init(&root).unwrap();
        fs::write(root.join("tracked"), "one\n").unwrap();
        commit(&repo, "initial");
        fs::write(root.join("tracked"), "two\n").unwrap();
        // Untracked files at the root, so collapsing cannot hide them: this is a
        // working tree genuinely larger than one read can carry.
        let untracked = 41;
        for i in 0..untracked {
            fs::write(root.join(format!("untracked-{i:05}")), "x").unwrap();
        }
        let total = untracked + 1;
        let scan = statuses(&repo).unwrap();
        assert_eq!(scan.total, total);
        assert_eq!(scan.iter().count(), total);
        // A repository that cannot be fingerprinted cannot be written at all.
        let stable = fingerprint(&repo).unwrap();
        assert_eq!(stable, fingerprint(&repo).unwrap());
        fs::write(root.join("untracked-99999"), "x").unwrap();
        assert_ne!(
            stable,
            fingerprint(&repo).unwrap(),
            "growth past the bound must still invalidate the snapshot"
        );
        fs::remove_file(root.join("untracked-99999")).unwrap();

        let mut service = Service::default();
        let repo_id = open(&mut service, &root);
        let status = json(
            &mut service,
            Request::Status {
                filter: None,
                repo_id: repo_id.clone(),
                page_size: 7,
                cursor: None,
            },
        );
        assert_eq!(status["metadata"]["truncated"], false);
        assert_eq!(status["metadata"]["totalEntries"], total as u64);
        assert!(status["metadata"].get("entryLimit").is_none());
        let mut seen = status["entries"].as_array().unwrap().len();
        let mut cursor = status["nextCursor"].as_str().map(str::to_owned);
        while let Some(next) = cursor {
            let page = json(
                &mut service,
                Request::Status {
                    filter: None,
                    repo_id: repo_id.clone(),
                    page_size: 7,
                    cursor: Some(next),
                },
            );
            assert_eq!(page["metadata"]["truncated"], false);
            assert_eq!(page["metadata"]["totalEntries"], total as u64);
            seen += page["entries"].as_array().unwrap().len();
            cursor = page["nextCursor"].as_str().map(str::to_owned);
        }
        assert_eq!(seen, total);
    }

    #[test]
    fn small_working_tree_reports_a_complete_listing() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        let repo = Repository::init(&root).unwrap();
        fs::write(root.join("tracked"), "one\n").unwrap();
        commit(&repo, "initial");
        fs::write(root.join("tracked"), "two\n").unwrap();
        fs::write(root.join("added"), "new\n").unwrap();
        let mut service = Service::default();
        let repo_id = open(&mut service, &root);
        let status = json(
            &mut service,
            Request::Status {
                filter: None,
                repo_id,
                page_size: 200,
                cursor: None,
            },
        );
        assert_eq!(status["metadata"]["truncated"], false);
        assert_eq!(status["metadata"]["totalEntries"], 2);
        assert_eq!(status["entries"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn untracked_directories_collapse_to_one_row_and_stage_whole() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        let repo = Repository::init(&root).unwrap();
        fs::write(root.join("tracked"), "one\n").unwrap();
        commit(&repo, "initial");
        fs::write(root.join(".gitignore"), "vendor/ignored\n").unwrap();
        fs::create_dir_all(root.join("vendor/deep")).unwrap();
        for i in 0..50 {
            fs::write(root.join("vendor").join(format!("f{i}.js")), "x").unwrap();
        }
        fs::write(root.join("vendor/deep/b.js"), "b").unwrap();
        fs::write(root.join("vendor/ignored"), "skip").unwrap();
        let mut service =
            Service::with_journal(Journal::open(temp.path().join("journal"), id()).unwrap());
        let repo_id = open(&mut service, &root);
        let status = json(
            &mut service,
            Request::Status {
                filter: None,
                repo_id: repo_id.clone(),
                page_size: 200,
                cursor: None,
            },
        );
        // Fifty-one untracked files below one directory are one row, as in
        // `git status`; nothing is truncated.
        assert_eq!(status["metadata"]["truncated"], false);
        assert_eq!(status["metadata"]["totalEntries"], 2);
        let entries = status["entries"].as_array().unwrap();
        let directory = entries
            .iter()
            .find(|e| e["path"]["display"] == "vendor/")
            .unwrap_or_else(|| panic!("{entries:?}"));
        assert_eq!(directory["untracked"], true);
        let staged = json(
            &mut service,
            Request::Start {
                operation_id: id(),
                repo_id,
                expected_snapshot: status["snapshot"].as_str().unwrap().into(),
                action: Action::Stage {
                    hunks: None,
                    entry_ids: vec![directory["entryId"].as_str().unwrap().into()],
                },
            },
        );
        assert_eq!(staged["state"], "succeeded", "{staged}");
        let fresh = Repository::open(&root).unwrap();
        let index = fresh.index().unwrap();
        let paths = index
            .iter()
            .map(|e| String::from_utf8(e.path.clone()).unwrap())
            .collect::<Vec<_>>();
        assert!(paths.contains(&"vendor/f0.js".to_owned()), "{paths:?}");
        assert!(paths.contains(&"vendor/deep/b.js".to_owned()), "{paths:?}");
        assert!(
            !paths.iter().any(|p| p == "vendor/ignored"),
            "ignored files are never staged: {paths:?}"
        );
    }

    #[test]
    fn a_collapsed_directory_diff_lists_the_files_it_holds() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        let repo = Repository::init(&root).unwrap();
        fs::write(root.join("tracked"), "one\n").unwrap();
        commit(&repo, "initial");
        fs::create_dir_all(root.join("vendor/deep")).unwrap();
        fs::write(root.join("vendor/a.js"), "a\n").unwrap();
        fs::write(root.join("vendor/deep/b.js"), "b\n").unwrap();
        let mut service = Service::default();
        let repo_id = open(&mut service, &root);
        let status = json(
            &mut service,
            Request::Status {
                filter: None,
                repo_id: repo_id.clone(),
                page_size: 200,
                cursor: None,
            },
        );
        let entry = status["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["path"]["display"] == "vendor/")
            .unwrap()
            .clone();
        let Output::Diff { bytes, .. } = service
            .request(Request::Diff {
                repo_id,
                snapshot: status["snapshot"].as_str().unwrap().into(),
                entry_id: entry["entryId"].as_str().unwrap().into(),
                side: Side::IndexToWorktree,
                context_lines: 3,
            })
            .unwrap()
        else {
            panic!("expected a diff")
        };
        let diff: Value = serde_json::from_slice(&bytes).unwrap();
        // Collapsing the status row must not hide the content behind it.
        assert_eq!(diff["truncated"], false, "{diff}");
        let shown = diff["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["newPath"]["display"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(shown, ["vendor/a.js", "vendor/deep/b.js"], "{diff}");
    }

    #[test]
    fn nested_repository_directories_keep_the_submodule_message() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        let repo = Repository::init(&root).unwrap();
        fs::write(root.join("tracked"), "one\n").unwrap();
        commit(&repo, "initial");
        let nested = root.join("nested");
        Repository::init(&nested).unwrap();
        fs::write(nested.join("inner"), "x\n").unwrap();
        let mut service =
            Service::with_journal(Journal::open(temp.path().join("journal"), id()).unwrap());
        let repo_id = open(&mut service, &root);
        let status = json(
            &mut service,
            Request::Status {
                filter: None,
                repo_id: repo_id.clone(),
                page_size: 200,
                cursor: None,
            },
        );
        let entries = status["entries"].as_array().unwrap();
        let entry = entries
            .iter()
            .find(|e| e["path"]["display"] == "nested/")
            .unwrap_or_else(|| panic!("{entries:?}"));
        let result = json(
            &mut service,
            Request::Start {
                operation_id: id(),
                repo_id,
                expected_snapshot: status["snapshot"].as_str().unwrap().into(),
                action: Action::Stage {
                    hunks: None,
                    entry_ids: vec![entry["entryId"].as_str().unwrap().into()],
                },
            },
        );
        assert_eq!(result["state"], "failed", "{result}");
        assert_eq!(result["error"]["code"], "UNSUPPORTED_CAPABILITY");
        assert!(result["error"]["message"]
            .as_str()
            .unwrap()
            .contains("submodule"));
    }

    fn status(service: &mut Service, repo_id: &str, cursor: Option<String>, page: usize) -> Value {
        json(
            service,
            Request::Status {
                filter: None,
                repo_id: repo_id.into(),
                page_size: page,
                cursor,
            },
        )
    }
    #[test]
    fn branch_listing_resolves_upstreams_exactly_as_libgit2_does() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = Repository::init(tmp.path().join("main")).unwrap();
        fs::write(repo.workdir().unwrap().join("a"), "x").unwrap();
        commit(&repo, "initial");
        let tip = repo.head().unwrap().peel_to_commit().unwrap();
        let mut config = repo.config().unwrap();
        config
            .set_str("remote.origin.url", "git@example.com:a.git")
            .unwrap();
        config
            .set_str("remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*")
            .unwrap();
        // A remote with a refspec that renames, as `git remote add -t` or a
        // hand edit can leave.
        config
            .set_str("remote.up.url", "git@example.com:b.git")
            .unwrap();
        config
            .set_str("remote.up.fetch", "+refs/heads/*:refs/remotes/renamed/*")
            .unwrap();
        let set = |config: &mut git2::Config, branch: &str, remote: &str, merge: &str| {
            config
                .set_str(&format!("branch.{branch}.remote"), remote)
                .unwrap();
            config
                .set_str(&format!("branch.{branch}.merge"), merge)
                .unwrap();
        };
        for (branch, remote, merge, tracked) in [
            (
                "tracks-origin",
                "origin",
                "refs/heads/tracks-origin",
                Some("refs/remotes/origin/tracks-origin"),
            ),
            (
                "tracks-renamed",
                "up",
                "refs/heads/elsewhere",
                Some("refs/remotes/renamed/elsewhere"),
            ),
            ("tracks-local", ".", "refs/heads/master", None),
            // Configured, but the remote-tracking branch was pruned.
            ("pruned", "origin", "refs/heads/pruned", None),
            // Configured against a remote that does not exist.
            ("no-remote", "gone", "refs/heads/no-remote", None),
        ] {
            repo.branch(branch, &tip, false).unwrap();
            set(&mut config, branch, remote, merge);
            if let Some(tracked) = tracked {
                repo.reference(tracked, tip.id(), false, "").unwrap();
            }
        }
        repo.branch("untracked", &tip, false).unwrap();
        let mut service = Service::default();
        let repo_id = open(&mut service, repo.workdir().unwrap());
        let listing = json(
            &mut service,
            Request::Branches {
                filter: String::new(),
                branch_kind: None,
                repo_id,
                page_size: 100,
                cursor: None,
            },
        );
        let mut compared = 0;
        for row in listing["entries"].as_array().unwrap() {
            let name = row["name"]["display"].as_str().unwrap();
            let kind = if row["remote"] == true {
                git2::BranchType::Remote
            } else {
                git2::BranchType::Local
            };
            let expected = repo
                .find_branch(name, kind)
                .unwrap()
                .upstream()
                .ok()
                .map(|b| b.get().name().unwrap().to_owned());
            let actual = row["upstream"]["display"].as_str().map(str::to_owned);
            assert_eq!(actual, expected, "{name}");
            compared += 1;
        }
        assert!(compared >= 9, "{listing}");
    }
    /// Times the reads the branch picker waits on, on a repository shaped
    /// like a busy team's: `cargo test --release -- --ignored --nocapture
    /// busy_repository_read_timings`.
    #[test]
    #[ignore]
    fn busy_repository_read_timings() {
        use std::time::Instant;
        let tmp = tempfile::tempdir().unwrap();
        let repo = Repository::init(tmp.path().join("main")).unwrap();
        fs::write(repo.workdir().unwrap().join("a"), "x").unwrap();
        commit(&repo, "initial");
        let tip = repo.head().unwrap().peel_to_commit().unwrap();
        let mut config = repo.config().unwrap();
        config
            .set_str("remote.origin.url", "git@example.com:team/app.git")
            .unwrap();
        config
            .set_str("remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*")
            .unwrap();
        for i in 0..400 {
            let name = format!("feature/branch-{i}");
            repo.branch(&name, &tip, false).unwrap();
            repo.reference(&format!("refs/remotes/origin/{name}"), tip.id(), false, "")
                .unwrap();
            config
                .set_str(&format!("branch.{name}.remote"), "origin")
                .unwrap();
            config
                .set_str(
                    &format!("branch.{name}.merge"),
                    &format!("refs/heads/{name}"),
                )
                .unwrap();
        }
        for i in 0..3000 {
            repo.reference(
                &format!("refs/remotes/origin/other/{i}"),
                tip.id(),
                false,
                "",
            )
            .unwrap();
        }
        for i in 0..60 {
            let path = tmp.path().join(format!("wt-{i}"));
            let name = format!("wt-{i}");
            repo.branch(&name, &tip, false).unwrap();
            let reference = repo.find_reference(&format!("refs/heads/{name}")).unwrap();
            let mut options = git2::WorktreeAddOptions::new();
            options.reference(Some(&reference));
            repo.worktree(&name, &path, Some(&options)).unwrap();
        }
        let mut service = Service::default();
        let repo_id = open(&mut service, repo.workdir().unwrap());
        for round in 0..2 {
            let started = Instant::now();
            let branches = json(
                &mut service,
                Request::Branches {
                    filter: String::new(),
                    branch_kind: None,
                    repo_id: repo_id.clone(),
                    page_size: 100,
                    cursor: None,
                },
            );
            let branches_ms = started.elapsed().as_millis();
            let started = Instant::now();
            json(
                &mut service,
                Request::Worktrees {
                    at_snapshot: None,
                    filter: String::new(),
                    branch: None,
                    name: None,
                    repo_id: repo_id.clone(),
                    page_size: 100,
                    cursor: None,
                },
            );
            let worktrees_ms = started.elapsed().as_millis();
            let started = Instant::now();
            status(&mut service, &repo_id, None, 100);
            let status_ms = started.elapsed().as_millis();
            eprintln!(
                "round {round}: branches {branches_ms} ms ({} rows on page), worktrees {worktrees_ms} ms, status {status_ms} ms",
                branches["entries"].as_array().unwrap().len()
            );
            // A fresh process, as a reconnect or another channel would be.
            service = Service::default();
        }
    }
    fn code(result: Result<Output, Error>) -> String {
        match result {
            Err(error) => error.code,
            Ok(_) => "OK".into(),
        }
    }
    #[test]
    fn tokens_outlive_the_process_that_issued_them() {
        // Open on one agent process, read status on a second, stage on a third:
        // what a reconnect, or a shared channel, looks like to the agent.
        let tmp = tempfile::tempdir().unwrap();
        let repo = Repository::init(tmp.path()).unwrap();
        fs::write(tmp.path().join("a"), "before").unwrap();
        commit(&repo, "initial");
        fs::write(tmp.path().join("a"), "after").unwrap();
        let records = tempfile::tempdir().unwrap();
        let journal = Journal::open(records.path().join("journal"), id()).unwrap();
        let repo_id = open(&mut Service::default(), tmp.path());
        let listing = status(&mut Service::default(), &repo_id, None, 100);
        let entry = listing["entries"][0]["entryId"]
            .as_str()
            .unwrap()
            .to_owned();
        let result = json(
            &mut Service::with_journal(journal),
            Request::Start {
                operation_id: id(),
                repo_id,
                expected_snapshot: listing["snapshot"].as_str().unwrap().into(),
                action: Action::Stage {
                    hunks: None,
                    entry_ids: vec![entry],
                },
            },
        );
        assert_eq!(result["state"], "succeeded", "{result}");
        assert!(Repository::open(tmp.path())
            .unwrap()
            .status_file(Path::new("a"))
            .unwrap()
            .is_index_modified());
    }
    #[test]
    fn identical_state_yields_identical_tokens() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = Repository::init(tmp.path()).unwrap();
        fs::write(tmp.path().join("a"), "one").unwrap();
        commit(&repo, "initial");
        fs::write(tmp.path().join("a"), "two").unwrap();
        let first = open(&mut Service::default(), tmp.path());
        assert_eq!(first, open(&mut Service::default(), tmp.path()));
        let a = status(&mut Service::default(), &first, None, 100);
        let b = status(&mut Service::default(), &first, None, 100);
        assert_eq!(a["snapshot"], b["snapshot"]);
        assert_eq!(a["entries"], b["entries"]);
    }
    #[test]
    fn a_cursor_continues_elsewhere_until_the_listing_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = Repository::init(tmp.path()).unwrap();
        fs::write(tmp.path().join("seed"), "x").unwrap();
        commit(&repo, "initial");
        for name in ["a", "b", "c"] {
            fs::write(tmp.path().join(name), name).unwrap();
        }
        let repo_id = open(&mut Service::default(), tmp.path());
        let first = status(&mut Service::default(), &repo_id, None, 1);
        let cursor = first["nextCursor"].as_str().unwrap().to_owned();
        // A process with no memory of the first page serves the second.
        let second = status(&mut Service::default(), &repo_id, Some(cursor.clone()), 1);
        assert_eq!(second["snapshot"], first["snapshot"]);
        assert_ne!(second["entries"], first["entries"]);
        // Once the working tree changes, the listing is not the same one, and a
        // page from it would mix two versions.
        fs::write(tmp.path().join("d"), "d").unwrap();
        let mut elsewhere = Service::default();
        assert_eq!(
            code(elsewhere.request(Request::Status {
                filter: None,
                repo_id: repo_id.clone(),
                page_size: 1,
                cursor: Some(cursor.clone()),
            })),
            "SNAPSHOT_EXPIRED"
        );
        // The process that captured it still serves its pages as captured, as
        // a stored snapshot always did.
        let mut here = Service::default();
        fs::remove_file(tmp.path().join("d")).unwrap();
        let again = status(&mut here, &repo_id, None, 1);
        fs::write(tmp.path().join("d"), "d").unwrap();
        let next = again["nextCursor"].as_str().unwrap().to_owned();
        assert_eq!(
            status(&mut here, &repo_id, Some(next), 1)["snapshot"],
            again["snapshot"]
        );
    }
    #[test]
    fn writes_and_diffs_refuse_what_the_repository_no_longer_is() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = Repository::init(tmp.path()).unwrap();
        fs::write(tmp.path().join("a"), "before").unwrap();
        fs::write(tmp.path().join("b"), "before").unwrap();
        commit(&repo, "initial");
        fs::write(tmp.path().join("a"), "after").unwrap();
        let records = tempfile::tempdir().unwrap();
        let journal = Journal::open(records.path().join("journal"), id()).unwrap();
        let mut service = Service::with_journal(journal);
        let repo_id = open(&mut service, tmp.path());
        let listing = status(&mut service, &repo_id, None, 100);
        let snapshot = listing["snapshot"].as_str().unwrap().to_owned();
        let stage = |entry: String, snapshot: String| Request::Start {
            operation_id: id(),
            repo_id: repo_id.clone(),
            expected_snapshot: snapshot,
            action: Action::Stage {
                hunks: None,
                entry_ids: vec![entry],
            },
        };
        // A well-formed entry for a file that is not in the current status --
        // "b" is unmodified -- is refused even though the snapshot is current.
        let unlisted = EntryRef::new(&[b"b".to_vec()]).encode();
        assert_eq!(
            code(service.request(stage(unlisted, snapshot.clone()))),
            "INVALID_REQUEST"
        );
        assert_eq!(
            code(service.request(Request::Diff {
                repo_id: repo_id.clone(),
                snapshot: snapshot.clone(),
                entry_id: EntryRef::new(&[b"b".to_vec()]).encode(),
                side: Side::IndexToWorktree,
                context_lines: 3,
            })),
            "INVALID_REQUEST"
        );
        // A traversal smuggled into an entry is refused before any work.
        let hostile = EntryRef::new(&[b"../outside".to_vec()]).encode();
        assert_eq!(
            code(service.request(stage(hostile, snapshot.clone()))),
            "INVALID_REQUEST"
        );
        // A snapshot handed over as a repository is a different kind of token.
        assert_eq!(
            code(service.request(Request::Branches {
                filter: String::new(),
                branch_kind: None,
                repo_id: snapshot.clone(),
                page_size: 10,
                cursor: None,
            })),
            "INVALID_REQUEST"
        );
        // Once the working tree moves, the old snapshot is stale for writes.
        let entry = listing["entries"][0]["entryId"]
            .as_str()
            .unwrap()
            .to_owned();
        fs::write(tmp.path().join("a"), "again").unwrap();
        assert_eq!(
            code(service.request(stage(entry, snapshot))),
            "STALE_SNAPSHOT"
        );
    }
    #[test]
    fn large_changes_are_listed_without_being_read() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = Repository::init(tmp.path()).unwrap();
        fs::write(tmp.path().join("seed"), "x").unwrap();
        commit(&repo, "initial");
        // Well past the old 64 MiB content budget; sparse, so it costs no disk.
        for name in ["dump.sql", "model.bin"] {
            fs::File::create(tmp.path().join(name))
                .unwrap()
                .set_len(200 * 1024 * 1024)
                .unwrap();
        }
        let mut service = Service::default();
        let repo_id = open(&mut service, tmp.path());
        let listing = status(&mut service, &repo_id, None, 100);
        assert_eq!(listing["entries"].as_array().unwrap().len(), 2);
        // The snapshot still moves when one of them does.
        let snapshot = listing["snapshot"].clone();
        fs::OpenOptions::new()
            .append(true)
            .open(tmp.path().join("dump.sql"))
            .unwrap()
            .set_len(200 * 1024 * 1024 + 1)
            .unwrap();
        let later = status(&mut Service::default(), &repo_id, None, 100);
        assert_ne!(later["snapshot"], snapshot);
    }
    #[test]
    fn a_snapshot_does_not_go_stale_by_itself() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = Repository::init(tmp.path()).unwrap();
        fs::write(tmp.path().join("a"), "one").unwrap();
        commit(&repo, "initial");
        // Written just before the read, then left alone.
        fs::write(tmp.path().join("a"), "two").unwrap();
        let first = fingerprint(&repo).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2100));
        assert_eq!(fingerprint(&repo).unwrap(), first);
    }
    #[test]
    fn a_same_size_rewrite_moves_the_snapshot() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = Repository::init(tmp.path()).unwrap();
        fs::write(tmp.path().join("a"), "one").unwrap();
        commit(&repo, "initial");
        fs::write(tmp.path().join("a"), "two").unwrap();
        let mut service = Service::default();
        let repo_id = open(&mut service, tmp.path());
        let first = status(&mut service, &repo_id, None, 100)["snapshot"].clone();
        // Same size, and the modification time put back as it was. (The change
        // time still moves here; the content read covers filesystems where it
        // would not.)
        let before = fs::metadata(tmp.path().join("a"))
            .unwrap()
            .modified()
            .unwrap();
        fs::write(tmp.path().join("a"), "six").unwrap();
        fs::File::options()
            .write(true)
            .open(tmp.path().join("a"))
            .unwrap()
            .set_modified(before)
            .unwrap();
        let second = status(&mut Service::default(), &repo_id, None, 100)["snapshot"].clone();
        assert_ne!(first, second);
    }
    #[test]
    fn no_handles_are_held_so_none_can_run_out() {
        let tmp = tempfile::tempdir().unwrap();
        let mut service = Service::default();
        // Well past the old 32-handle cap.
        for n in 0..40 {
            let path = tmp.path().join(n.to_string());
            Repository::init(&path).unwrap();
            let repo_id = open(&mut service, &path);
            assert_eq!(
                json(
                    &mut service,
                    Request::Close {
                        repo_id: repo_id.clone()
                    }
                )["closed"],
                true
            );
            // Closing released nothing, so the id still works.
            json(
                &mut service,
                Request::Branches {
                    filter: String::new(),
                    branch_kind: None,
                    repo_id,
                    page_size: 10,
                    cursor: None,
                },
            );
        }
        assert_eq!(
            code(service.request(Request::Close {
                repo_id: "r.garbage".into()
            })),
            "INVALID_REQUEST"
        );
    }
    #[test]
    fn a_deleted_repository_is_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("gone");
        Repository::init(&path).unwrap();
        let repo_id = open(&mut Service::default(), &path);
        fs::remove_dir_all(&path).unwrap();
        assert_eq!(
            code(Service::default().request(Request::Branches {
                filter: String::new(),
                branch_kind: None,
                repo_id,
                page_size: 10,
                cursor: None
            })),
            "REPO_NOT_FOUND"
        );
    }
}

#[cfg(test)]
#[path = "benchmarks.rs"]
mod benchmarks;
