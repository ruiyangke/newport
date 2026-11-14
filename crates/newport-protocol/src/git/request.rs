//! Git operations and their persisted request identities.
use super::Error;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Path {
    pub bytes_b64: String,
    #[serde(default)]
    pub display: String,
}
impl Path {
    pub fn new(bytes: &[u8]) -> Self {
        Self {
            bytes_b64: STANDARD.encode(bytes),
            display: String::from_utf8_lossy(bytes).into_owned(),
        }
    }
    pub fn decode(&self) -> Result<Vec<u8>, Error> {
        if self.bytes_b64.len() > super::MAX_PATH.div_ceil(3) * 4 {
            return Err(Error::invalid("Invalid path length."));
        }
        let bytes = STANDARD
            .decode(&self.bytes_b64)
            .map_err(|_| Error::invalid("Invalid path encoding."))?;
        if bytes.is_empty() || bytes.len() > super::MAX_PATH || bytes.contains(&0) {
            return Err(Error::invalid("Invalid path."));
        }
        Ok(bytes)
    }
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatusFilter {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub group: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    HeadToIndex,
    IndexToWorktree,
    HeadToWorktree,
}
fn page_size() -> usize {
    100
}
fn revision() -> String {
    "HEAD".into()
}
fn context() -> u32 {
    3
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Author {
    pub name: String,
    pub email: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CheckoutTarget {
    Branch {
        name: String,
        #[serde(rename = "expectedOid")]
        expected_oid: String,
    },
    Detached {
        oid: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResetMode {
    Soft,
    Mixed,
    Hard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscardSource {
    Index,
    Head,
}

/// The three index stages of a conflicted path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictSide {
    Base,
    Ours,
    Theirs,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TagAnnotation {
    pub message: String,
    #[serde(default)]
    pub author: Option<Author>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HunkSelection {
    pub ids: Vec<String>,
    /// Server-generated identifiers for individual changed lines inside the
    /// selected hunks. An absent selection applies to each whole hunk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<Vec<String>>,
    pub context_lines: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    #[serde(rename = "conflict.resolve")]
    ConflictResolve {
        #[serde(rename = "entryIds")]
        entry_ids: Vec<String>,
        side: ConflictSide,
        /// The blob that side held in the status this request was built from, or
        /// null when that side deleted the file. Never a client-supplied blob.
        #[serde(rename = "expectedOid")]
        expected_oid: Option<String>,
    },
    Discard {
        #[serde(rename = "entryIds")]
        entry_ids: Vec<String>,
        source: DiscardSource,
        /// Restricts the discard to selected hunks or lines of one file.
        /// An absent selection applies to each whole file.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hunks: Option<HunkSelection>,
    },
    Reset {
        #[serde(rename = "targetOid")]
        target_oid: String,
        #[serde(rename = "expectedOid")]
        expected_oid: String,
        mode: ResetMode,
    },
    Rebase {
        #[serde(rename = "upstreamOid")]
        upstream_oid: String,
        #[serde(rename = "ontoOid", default)]
        onto_oid: Option<String>,
        #[serde(default)]
        committer: Option<Author>,
    },
    #[serde(rename = "worktree.repair")]
    WorktreeRepair {
        name: String,
        path: Path,
    },
    #[serde(rename = "worktree.remove")]
    WorktreeRemove {
        name: String,
    },
    #[serde(rename = "worktree.prune")]
    WorktreePrune {
        name: String,
    },
    #[serde(rename = "worktree.add")]
    WorktreeAdd {
        name: String,
        path: Path,
        branch: String,
        /// The branch's current commit, or -- with `newBranch` -- the commit
        /// the new branch starts at.
        #[serde(rename = "expectedOid")]
        expected_oid: String,
        #[serde(default)]
        locked: bool,
        /// Create `branch` at `expectedOid` for this worktree, as
        /// `git worktree add -b` does, rather than check out an existing one.
        /// Advertised as the `worktree.new_branch` feature.
        #[serde(default, rename = "newBranch")]
        new_branch: bool,
    },
    #[serde(rename = "worktree.lock")]
    WorktreeLock {
        name: String,
        #[serde(default)]
        reason: Option<String>,
    },
    #[serde(rename = "worktree.unlock")]
    WorktreeUnlock {
        name: String,
    },
    #[serde(rename = "integration.skip")]
    IntegrationSkip {},
    #[serde(rename = "branch.set_upstream")]
    BranchSetUpstream {
        name: String,
        #[serde(rename = "expectedOid")]
        expected_oid: String,
        #[serde(rename = "expectedToken")]
        expected_token: String,
        upstream: Option<String>,
    },
    CherryPick {
        #[serde(rename = "targetOid")]
        target_oid: String,
        #[serde(default)]
        mainline: u32,
        #[serde(default)]
        author: Option<Author>,
    },
    Revert {
        #[serde(rename = "targetOid")]
        target_oid: String,
        #[serde(default)]
        mainline: u32,
        #[serde(default)]
        author: Option<Author>,
    },
    #[serde(rename = "integration.continue")]
    IntegrationContinue {
        #[serde(default)]
        message: Option<String>,
        #[serde(default)]
        author: Option<Author>,
    },
    #[serde(rename = "integration.abort")]
    IntegrationAbort {},
    #[serde(rename = "tag.delete_remote")]
    TagDeleteRemote {
        remote: String,
        #[serde(rename = "expectedToken")]
        expected_token: String,
        name: String,
        #[serde(rename = "expectedOid")]
        expected_oid: String,
    },
    #[serde(rename = "tag.push")]
    TagPush {
        remote: String,
        #[serde(rename = "expectedToken")]
        expected_token: String,
        name: String,
        #[serde(rename = "expectedOid")]
        expected_oid: String,
    },
    #[serde(rename = "tag.create")]
    TagCreate {
        name: String,
        #[serde(rename = "targetOid")]
        target_oid: String,
        #[serde(default)]
        annotation: Option<TagAnnotation>,
    },
    #[serde(rename = "tag.delete")]
    TagDelete {
        name: String,
        #[serde(rename = "expectedOid")]
        expected_oid: String,
    },
    #[serde(rename = "stash.save")]
    StashSave {
        #[serde(default)]
        message: String,
        #[serde(rename = "includeUntracked", default)]
        include_untracked: bool,
        #[serde(rename = "keepIndex", default)]
        keep_index: bool,
        #[serde(default)]
        author: Option<Author>,
    },
    #[serde(rename = "stash.apply")]
    StashApply {
        oid: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<usize>,
        #[serde(rename = "expectedToken")]
        expected_token: String,
        #[serde(rename = "reinstateIndex", default)]
        reinstate_index: bool,
    },
    #[serde(rename = "stash.pop")]
    StashPop {
        oid: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<usize>,
        #[serde(rename = "expectedToken")]
        expected_token: String,
        #[serde(rename = "reinstateIndex", default)]
        reinstate_index: bool,
    },
    #[serde(rename = "stash.drop")]
    StashDrop {
        oid: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<usize>,
        #[serde(rename = "expectedToken")]
        expected_token: String,
    },
    #[serde(rename = "merge.abort")]
    MergeAbort {},
    Merge {
        #[serde(rename = "targetOid")]
        target_oid: String,
    },
    #[serde(rename = "pull.fast_forward")]
    PullFastForward {
        remote: String,
        #[serde(rename = "expectedToken")]
        expected_token: String,
        #[serde(rename = "remoteBranch")]
        remote_branch: String,
    },
    #[serde(rename = "merge.fast_forward")]
    FastForward {
        #[serde(rename = "targetOid")]
        target_oid: String,
    },
    #[serde(rename = "remote.add")]
    RemoteAdd {
        name: String,
        url: String,
    },
    #[serde(rename = "remote.rename")]
    RemoteRename {
        name: String,
        #[serde(rename = "newName")]
        new_name: String,
        #[serde(rename = "expectedToken")]
        expected_token: String,
    },
    #[serde(rename = "remote.set_url")]
    RemoteSetUrl {
        name: String,
        url: String,
        #[serde(rename = "expectedToken")]
        expected_token: String,
    },
    #[serde(rename = "remote.remove")]
    RemoteRemove {
        name: String,
        #[serde(rename = "expectedToken")]
        expected_token: String,
    },
    /// Repository settings: prune-on-fetch default and commit identity.
    /// Only the provided fields change; at least one is required.
    #[serde(rename = "config.set")]
    ConfigSet {
        #[serde(rename = "expectedToken")]
        expected_token: String,
        #[serde(default, rename = "fetchPrune")]
        fetch_prune: Option<bool>,
        #[serde(default, rename = "userName")]
        user_name: Option<String>,
        #[serde(default, rename = "userEmail")]
        user_email: Option<String>,
    },
    Fetch {
        remote: String,
        #[serde(rename = "expectedToken")]
        expected_token: String,
        #[serde(default)]
        prune: bool,
    },
    #[serde(rename = "branch.delete_remote")]
    BranchDeleteRemote {
        remote: String,
        #[serde(rename = "expectedToken")]
        expected_token: String,
        branch: String,
        #[serde(rename = "expectedOid")]
        expected_oid: String,
    },
    #[serde(rename = "push.with_lease")]
    PushWithLease {
        remote: String,
        #[serde(rename = "expectedToken")]
        expected_token: String,
        branch: String,
        #[serde(rename = "expectedOid")]
        expected_oid: String,
        #[serde(rename = "destinationBranch")]
        destination_branch: String,
        #[serde(rename = "expectedRemoteOid")]
        expected_remote_oid: String,
    },
    Push {
        remote: String,
        #[serde(rename = "expectedToken")]
        expected_token: String,
        branch: String,
        #[serde(rename = "expectedOid")]
        expected_oid: String,
        #[serde(rename = "destinationBranch")]
        destination_branch: String,
    },
    Checkout {
        target: CheckoutTarget,
    },
    #[serde(rename = "branch.create")]
    BranchCreate {
        name: String,
        #[serde(rename = "startOid")]
        start_oid: String,
    },
    #[serde(rename = "branch.rename")]
    BranchRename {
        name: String,
        #[serde(rename = "newName")]
        new_name: String,
        #[serde(rename = "expectedOid")]
        expected_oid: String,
    },
    #[serde(rename = "branch.delete")]
    BranchDelete {
        name: String,
        #[serde(rename = "expectedOid")]
        expected_oid: String,
        #[serde(default)]
        force: bool,
    },
    #[serde(rename = "commit.amend")]
    Amend {
        #[serde(rename = "expectedOid")]
        expected_oid: String,
        message: String,
        #[serde(default)]
        committer: Option<Author>,
        #[serde(default)]
        author: Option<Author>,
    },
    Commit {
        message: String,
        #[serde(default)]
        author: Option<Author>,
    },
    Stage {
        #[serde(rename = "entryIds")]
        entry_ids: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hunks: Option<HunkSelection>,
    },
    Unstage {
        #[serde(rename = "entryIds")]
        entry_ids: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hunks: Option<HunkSelection>,
    },
}

/// A compact representation the agent can page diff line fragments in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiffLineEncoding {
    #[serde(rename = "tuple_v1")]
    TupleV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", deny_unknown_fields)]
pub enum Request {
    #[serde(rename = "repo.clone")]
    Clone {
        #[serde(rename = "operationId")]
        operation_id: String,
        url: String,
        path: Path,
        #[serde(default)]
        branch: Option<String>,
        #[serde(default)]
        bare: bool,
    },
    #[serde(rename = "repo.init")]
    Init {
        #[serde(rename = "operationId")]
        operation_id: String,
        path: Path,
        #[serde(rename = "initialBranch")]
        initial_branch: String,
    },
    #[serde(rename = "repo.tag")]
    Tag {
        #[serde(rename = "repoId")]
        repo_id: String,
        oid: String,
    },
    #[serde(rename = "repo.tags")]
    Tags {
        #[serde(rename = "repoId")]
        repo_id: String,
        #[serde(rename = "pageSize", default = "page_size")]
        page_size: usize,
        #[serde(default)]
        cursor: Option<String>,
        #[serde(
            default,
            rename = "messageBytes",
            skip_serializing_if = "Option::is_none"
        )]
        message_bytes: Option<usize>,
    },
    #[serde(rename = "repo.stashes")]
    Stashes {
        #[serde(rename = "repoId")]
        repo_id: String,
        #[serde(rename = "pageSize", default = "page_size")]
        page_size: usize,
        #[serde(default)]
        cursor: Option<String>,
    },
    #[serde(rename = "repo.blob_page")]
    BlobPage {
        #[serde(rename = "repoId")]
        repo_id: String,
        oid: String,
        #[serde(default, rename = "maxBytes")]
        max_bytes: Option<usize>,
        #[serde(default)]
        cursor: Option<String>,
    },
    #[serde(rename = "repo.blob")]
    Blob {
        #[serde(rename = "repoId")]
        repo_id: String,
        oid: String,
    },
    #[serde(rename = "repo.remote_refs")]
    RemoteRefs {
        #[serde(rename = "repoId")]
        repo_id: String,
        remote: String,
        #[serde(rename = "expectedToken")]
        expected_token: String,
        #[serde(default, rename = "forPush")]
        for_push: bool,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        filter: String,
        #[serde(default = "page_size", rename = "pageSize")]
        page_size: usize,
        #[serde(default)]
        cursor: Option<String>,
    },
    #[serde(rename = "repo.remote_names")]
    RemoteNames {
        #[serde(rename = "repoId")]
        repo_id: String,
        #[serde(default)]
        filter: String,
        #[serde(default = "page_size", rename = "pageSize")]
        page_size: usize,
        #[serde(default)]
        cursor: Option<String>,
    },
    #[serde(rename = "repo.remote")]
    Remote {
        #[serde(rename = "repoId")]
        repo_id: String,
        name: String,
    },
    #[serde(rename = "repo.remotes")]
    Remotes {
        #[serde(rename = "repoId")]
        repo_id: String,
    },
    #[serde(rename = "operation.start")]
    Start {
        #[serde(rename = "operationId")]
        operation_id: String,
        #[serde(rename = "repoId")]
        repo_id: String,
        #[serde(rename = "expectedSnapshot")]
        expected_snapshot: String,
        action: Action,
    },
    #[serde(rename = "operation.get")]
    Get {
        #[serde(rename = "operationId")]
        operation_id: String,
    },
    #[serde(rename = "operation.review")]
    Review {
        #[serde(rename = "operationId")]
        operation_id: String,
    },
    #[serde(rename = "repo.open")]
    Open { path: Path },
    #[serde(rename = "repo.close")]
    Close {
        #[serde(rename = "repoId")]
        repo_id: String,
    },
    #[serde(rename = "repo.status_summary")]
    StatusSummary {
        #[serde(default, rename = "repoId", skip_serializing_if = "Option::is_none")]
        repo_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<Path>,
    },
    #[serde(rename = "repo.status")]
    Status {
        #[serde(rename = "repoId")]
        repo_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filter: Option<StatusFilter>,
        #[serde(default = "page_size", rename = "pageSize")]
        page_size: usize,
        #[serde(default)]
        cursor: Option<String>,
    },
    #[serde(rename = "repo.worktrees")]
    Worktrees {
        #[serde(rename = "repoId")]
        repo_id: String,
        #[serde(default = "page_size", rename = "pageSize")]
        page_size: usize,
        #[serde(default)]
        cursor: Option<String>,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        filter: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        branch: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(
            default,
            rename = "atSnapshot",
            skip_serializing_if = "Option::is_none"
        )]
        at_snapshot: Option<String>,
    },
    #[serde(rename = "repo.branches")]
    Branches {
        #[serde(rename = "repoId")]
        repo_id: String,
        #[serde(default = "page_size", rename = "pageSize")]
        page_size: usize,
        #[serde(default)]
        cursor: Option<String>,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        filter: String,
        #[serde(
            default,
            rename = "branchKind",
            skip_serializing_if = "Option::is_none"
        )]
        branch_kind: Option<String>,
    },
    #[serde(rename = "repo.history")]
    History {
        #[serde(rename = "repoId")]
        repo_id: String,
        #[serde(default = "page_size", rename = "pageSize")]
        page_size: usize,
        #[serde(default)]
        cursor: Option<String>,
        #[serde(default = "revision")]
        revision: String,
        #[serde(
            default,
            rename = "messageBytes",
            skip_serializing_if = "Option::is_none"
        )]
        message_bytes: Option<usize>,
    },
    #[serde(rename = "repo.commit")]
    Commit {
        #[serde(rename = "repoId")]
        repo_id: String,
        #[serde(rename = "commitOid")]
        commit_oid: String,
    },
    #[serde(rename = "repo.commit_files")]
    CommitFiles {
        #[serde(rename = "repoId")]
        repo_id: String,
        #[serde(rename = "commitOid")]
        commit_oid: String,
        #[serde(default, rename = "parentIndex")]
        parent_index: usize,
        #[serde(default = "page_size", rename = "pageSize")]
        page_size: usize,
        #[serde(default)]
        cursor: Option<String>,
    },
    #[serde(rename = "repo.commit_diff")]
    CommitDiff {
        #[serde(default)]
        path: Option<Path>,
        #[serde(rename = "repoId")]
        repo_id: String,
        #[serde(rename = "commitOid")]
        commit_oid: String,
        #[serde(default, rename = "parentIndex")]
        parent_index: usize,
        #[serde(default = "context", rename = "contextLines")]
        context_lines: u32,
    },
    #[serde(rename = "repo.commit_diff_page")]
    CommitDiffPage {
        #[serde(rename = "repoId")]
        repo_id: String,
        #[serde(rename = "commitOid")]
        commit_oid: String,
        path: Path,
        #[serde(default, rename = "parentIndex")]
        parent_index: usize,
        #[serde(default = "context", rename = "contextLines")]
        context_lines: u32,
        #[serde(default = "page_size", rename = "pageSize")]
        page_size: usize,
        #[serde(default, rename = "maxBytes")]
        max_bytes: Option<usize>,
        #[serde(default)]
        cursor: Option<String>,
        #[serde(
            default,
            rename = "lineEncoding",
            skip_serializing_if = "Option::is_none"
        )]
        line_encoding: Option<DiffLineEncoding>,
    },
    #[serde(rename = "repo.diff_page")]
    DiffPage {
        #[serde(rename = "repoId")]
        repo_id: String,
        snapshot: String,
        #[serde(rename = "entryId")]
        entry_id: String,
        side: Side,
        #[serde(default = "context", rename = "contextLines")]
        context_lines: u32,
        #[serde(default = "page_size", rename = "pageSize")]
        page_size: usize,
        #[serde(default, rename = "maxBytes")]
        max_bytes: Option<usize>,
        #[serde(default)]
        cursor: Option<String>,
        #[serde(
            default,
            rename = "lineEncoding",
            skip_serializing_if = "Option::is_none"
        )]
        line_encoding: Option<DiffLineEncoding>,
    },
    #[serde(rename = "repo.diff")]
    Diff {
        #[serde(rename = "repoId")]
        repo_id: String,
        snapshot: String,
        #[serde(rename = "entryId")]
        entry_id: String,
        side: Side,
        #[serde(default = "context", rename = "contextLines")]
        context_lines: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandLog {
    pub command: String,
    pub duration_ms: u64,
    pub exit_code: Option<i32>,
    pub output: String,
    pub interrupted: bool,
}

impl Request {
    pub fn method(&self) -> &'static str {
        match self {
            Self::Clone { .. } => "repo.clone",
            Self::Init { .. } => "repo.init",
            Self::Tag { .. } => "repo.tag",
            Self::Tags { .. } => "repo.tags",
            Self::Stashes { .. } => "repo.stashes",
            Self::BlobPage { .. } => "repo.blob_page",
            Self::Blob { .. } => "repo.blob",
            Self::RemoteRefs { .. } => "repo.remote_refs",
            Self::RemoteNames { .. } => "repo.remote_names",
            Self::Remote { .. } => "repo.remote",
            Self::Remotes { .. } => "repo.remotes",
            Self::Start { .. } => "operation.start",
            Self::Get { .. } => "operation.get",
            Self::Review { .. } => "operation.review",
            Self::Open { .. } => "repo.open",
            Self::Close { .. } => "repo.close",
            Self::StatusSummary { .. } => "repo.status_summary",
            Self::Status { .. } => "repo.status",
            Self::Worktrees { .. } => "repo.worktrees",
            Self::Branches { .. } => "repo.branches",
            Self::History { .. } => "repo.history",
            Self::Commit { .. } => "repo.commit",
            Self::CommitFiles { .. } => "repo.commit_files",
            Self::CommitDiff { .. } => "repo.commit_diff",
            Self::CommitDiffPage { .. } => "repo.commit_diff_page",
            Self::DiffPage { .. } => "repo.diff_page",
            Self::Diff { .. } => "repo.diff",
        }
    }
    pub fn is_mutation(&self) -> bool {
        matches!(
            self,
            Self::Start { .. } | Self::Init { .. } | Self::Clone { .. } | Self::Review { .. }
        )
    }
}

impl Request {
    pub fn reply_kind(&self) -> super::ReplyKind {
        match self {
            Self::Clone { .. } => super::ReplyKind::Operation,
            Self::Init { .. } => super::ReplyKind::Operation,
            Self::Tag { .. } => super::ReplyKind::Tag,
            Self::Tags { .. } => super::ReplyKind::Tags,
            Self::Stashes { .. } => super::ReplyKind::Stashes,
            Self::BlobPage { .. } => super::ReplyKind::BlobPage,
            Self::Blob { .. } => super::ReplyKind::Blob,
            Self::RemoteRefs { .. } => super::ReplyKind::RemoteRefs,
            Self::RemoteNames { .. } => super::ReplyKind::RemoteNames,
            Self::Remote { .. } => super::ReplyKind::Remote,
            Self::Remotes { .. } => super::ReplyKind::Remotes,
            Self::Start { .. } => super::ReplyKind::Operation,
            Self::Get { .. } => super::ReplyKind::Operation,
            Self::Review { .. } => super::ReplyKind::Operation,
            Self::Open { .. } => super::ReplyKind::Open,
            Self::Close { .. } => super::ReplyKind::Close,
            Self::StatusSummary { .. } => super::ReplyKind::StatusSummary,
            Self::Status { .. } => super::ReplyKind::Status,
            Self::Worktrees { .. } => super::ReplyKind::Worktrees,
            Self::Branches { .. } => super::ReplyKind::Branches,
            Self::History { .. } => super::ReplyKind::History,
            Self::Commit { .. } => super::ReplyKind::Commit,
            Self::CommitFiles { .. } => super::ReplyKind::CommitFiles,
            Self::CommitDiff { .. } => super::ReplyKind::StreamComplete,
            Self::CommitDiffPage { .. } => super::ReplyKind::CommitDiffPage,
            Self::DiffPage { .. } => super::ReplyKind::DiffPage,
            Self::Diff { .. } => super::ReplyKind::StreamComplete,
        }
    }
}

pub const METHODS: &[&str] = &[
    "repo.clone",
    "repo.init",
    "repo.tag",
    "repo.tags",
    "repo.stashes",
    "repo.blob_page",
    "repo.blob",
    "repo.remote_refs",
    "repo.remote_names",
    "repo.remote",
    "repo.remotes",
    "operation.start",
    "operation.get",
    "operation.review",
    "repo.open",
    "repo.close",
    "repo.status_summary",
    "repo.status",
    "repo.worktrees",
    "repo.branches",
    "repo.history",
    "repo.commit",
    "repo.commit_files",
    "repo.commit_diff",
    "repo.commit_diff_page",
    "repo.diff_page",
    "repo.diff",
];
