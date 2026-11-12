//! Typed Git results. Serde is the desktop and durable-record projection.
use super::{Author, Error, Path, Side};
use serde::{Deserialize, Serialize};

/// Binary data whose JSON projection is base64.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Binary(pub Vec<u8>);
impl std::fmt::Debug for Binary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Binary")
            .field("len", &self.0.len())
            .finish()
    }
}
impl Serialize for Binary {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use base64::Engine;
        serializer.serialize_str(&base64::engine::general_purpose::STANDARD.encode(&self.0))
    }
}
impl<'de> Deserialize<'de> for Binary {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use base64::Engine;
        let encoded = String::deserialize(deserializer)?;
        base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map(Self)
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Page<T, M> {
    pub snapshot: String,
    pub entries: Vec<T>,
    pub next_cursor: Option<String>,
    pub metadata: M,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationState {
    Running,
    Succeeded,
    Failed,
    NeedsResolution,
    OutcomeUnknown,
    ReviewedUnknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObjectId {
    pub algorithm: String,
    pub hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TagObjectId {
    pub format: String,
    pub hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Head {
    pub oid: Option<ObjectId>,
    pub name: Option<Path>,
    pub detached: bool,
    pub unborn: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IntegrationState {
    pub kind: String,
    pub managed: bool,
    pub can_continue: bool,
    pub can_abort: bool,
    pub can_skip: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RepositoryCapabilities {
    pub read_only: bool,
    pub working_tree: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Repository {
    pub repo_id: String,
    pub common_repo_id: String,
    pub root: Path,
    pub bare: bool,
    pub object_format: String,
    pub head: Head,
    pub operation_state: String,
    pub integration: Option<IntegrationState>,
    pub capabilities: RepositoryCapabilities,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GroupCounts {
    pub staged: u64,
    pub unstaged: u64,
    pub untracked: u64,
    pub conflicted: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Upstream {
    pub remote: Path,
    pub reference: Path,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StatusSummary {
    pub head: Head,
    pub operation_state: String,
    pub integration: Option<IntegrationState>,
    pub ahead: Option<u64>,
    pub behind: Option<u64>,
    pub basis: String,
    pub upstream_ref: Option<Path>,
    pub upstream: Option<Upstream>,
    pub truncated: bool,
    pub total_entries: u64,
    pub group_counts: GroupCounts,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched_entries: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConflictEntry {
    pub mode: u32,
    pub oid: ObjectId,
    pub path: Path,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Conflict {
    pub base: Option<ConflictEntry>,
    pub ours: Option<ConflictEntry>,
    pub theirs: Option<ConflictEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StatusEntry {
    pub entry_id: String,
    pub path: Path,
    pub old_path: Option<Path>,
    pub flags: u32,
    pub staged: bool,
    pub unstaged: bool,
    pub untracked: bool,
    pub conflicted: bool,
    pub conflict: Option<Conflict>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationEntry {
    pub value: Option<Path>,
    pub level: String,
    pub include_depth: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrackingConfiguration {
    pub remote: Vec<ConfigurationEntry>,
    pub merge: Vec<ConfigurationEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Tracking {
    pub token: String,
    pub editable: bool,
    pub configuration: TrackingConfiguration,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Branch {
    pub name: Path,
    pub reference: Path,
    pub oid: ObjectId,
    pub remote: bool,
    pub current: bool,
    pub upstream: Option<Path>,
    pub tracking: Option<Tracking>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TotalEntries {
    pub total_entries: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Worktree {
    pub name: Option<Path>,
    pub kind: String,
    pub path: Path,
    pub git_dir: Path,
    pub current: bool,
    pub state: String,
    pub head: Head,
    pub locked: bool,
    pub lock_reason: Option<Path>,
    pub prunable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorktreeMetadata {
    pub list_token: String,
    pub total_entries: u64,
    pub matching_entries: u64,
    pub current: Option<Worktree>,
    pub main: Option<Worktree>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Remote {
    pub name: String,
    pub url: Option<String>,
    pub push_url: Option<String>,
    pub token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteAuthentication {
    pub ssh: String,
    pub https: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RepositoryConfiguration {
    pub fetch_prune: bool,
    pub user_name: Option<String>,
    pub user_email: Option<String>,
    pub token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Remotes {
    pub entries: Vec<Remote>,
    pub authentication: RemoteAuthentication,
    pub configuration: RepositoryConfiguration,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteName {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteReference {
    pub reference: Path,
    pub kind: String,
    pub oid: ObjectId,
    pub symbolic_target: Option<Path>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteReferenceMetadata {
    pub remote: String,
    pub remote_token: String,
    pub for_push: bool,
    pub basis: String,
    pub truncated: bool,
    pub total_entries: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Blob {
    pub oid: ObjectId,
    pub size: u64,
    pub bytes_b64: Option<Binary>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BlobFragment {
    pub offset: u64,
    pub bytes_b64: Binary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BlobMetadata {
    pub oid: ObjectId,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Stash {
    pub index: u64,
    pub oid: String,
    pub previous_oid: String,
    pub message: String,
    pub message_truncated: bool,
    pub time: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StashMetadata {
    pub list_token: String,
    pub total_entries: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Commit {
    pub oid: ObjectId,
    pub parents: Vec<ObjectId>,
    pub message: Path,
    pub message_truncated: bool,
    pub author: Author,
    pub time: i64,
    pub offset_minutes: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoryMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_revision: Option<ObjectId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Tagger {
    pub name: String,
    pub email: String,
    pub time: i64,
    pub offset_minutes: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Tag {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<Path>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<Path>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbolic_target: Option<Path>,
    pub oid: TagObjectId,
    pub annotated: bool,
    pub details_omitted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_oid: Option<TagObjectId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peeled_oid: Option<TagObjectId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peeled_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<Path>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_truncated: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tagger: Option<Tagger>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Delta {
    pub old_path: Path,
    pub new_path: Path,
    pub old_oid: Option<ObjectId>,
    pub new_oid: Option<ObjectId>,
    pub old_mode: u32,
    pub new_mode: u32,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiffMetadata {
    pub read_only: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_oid: Option<ObjectId>,
    #[serde(default)]
    pub parent_oid: Option<ObjectId>,
    #[serde(default)]
    pub parent_index: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parents: Option<Vec<ObjectId>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_snapshot: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side: Option<Side>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_lines: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_path: Option<Path>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_files: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_units: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_omissions: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiffLine {
    pub id: Option<String>,
    pub origin: String,
    pub old_line: Option<u64>,
    pub new_line: Option<u64>,
    pub content: Path,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiffHunk {
    pub id: Option<String>,
    pub old_start: u64,
    pub old_lines: u64,
    pub new_start: u64,
    pub new_lines: u64,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiffFile {
    pub old_path: Path,
    pub new_path: Path,
    pub old_oid: Option<ObjectId>,
    pub new_oid: Option<ObjectId>,
    pub old_mode: u32,
    pub new_mode: u32,
    pub status: String,
    pub binary: bool,
    pub additions: u64,
    pub deletions: u64,
    pub hunks: Vec<DiffHunk>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Diff {
    pub files: Vec<DiffFile>,
    pub read_only: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_oid: Option<ObjectId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_oid: Option<ObjectId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_index: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parents: Option<Vec<ObjectId>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_snapshot: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side: Option<Side>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_lines: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_path: Option<Path>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_files: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_units: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_omissions: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", from = "LineRepresentation")]
pub struct LineFragment {
    pub line_index: u64,
    pub byte_offset: u64,
    pub line_complete: bool,
    pub origin: String,
    pub old_line: Option<u64>,
    pub new_line: Option<u64>,
    pub content_bytes_b64: Binary,
    pub id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HunkFragment {
    pub id: Option<String>,
    pub index: u64,
    pub old_start: u64,
    pub old_lines: u64,
    pub new_start: u64,
    pub new_lines: u64,
    pub total_lines: u64,
    pub lines: Vec<LineFragment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileFragment {
    pub old_path: Path,
    pub new_path: Path,
    pub old_oid: Option<ObjectId>,
    pub new_oid: Option<ObjectId>,
    pub old_mode: u32,
    pub new_mode: u32,
    pub status: String,
    pub binary: bool,
    pub additions: u64,
    pub deletions: u64,
    pub file_index: u64,
    pub omission_reason: Option<String>,
    pub hunks: Vec<HunkFragment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Operation {
    pub operation_id: String,
    pub repository: String,
    pub payload_hash: String,
    pub state: OperationState,
    pub seq: u32,
    pub result: Option<OperationResult>,
    pub error: Option<Error>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Closed {
    pub closed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StreamComplete {
    pub last_seq: u32,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Refresh {
    pub refresh_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Committed {
    pub merge_completed: bool,
    pub commit_oid: String,
    pub parent_oid: Option<String>,
    pub refresh_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Amended {
    pub amended: bool,
    pub replaced_oid: String,
    pub commit_oid: String,
    pub refresh_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrackingChanged {
    pub tracking: Tracking,
    pub upstream: Option<String>,
    pub refresh_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CheckedOut {
    pub detached: bool,
    pub oid: Option<String>,
    pub refresh_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteRenamed {
    pub renamed_references: u64,
    pub token: String,
    pub refresh_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FastForwarded {
    pub fast_forwarded: bool,
    pub oid: String,
    pub refresh_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LeaseMatched {
    pub lease_matched: bool,
    pub refresh_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteDeleted {
    pub deleted: bool,
    pub lease_matched: bool,
    pub oid: Option<String>,
    pub destination_tag: Option<String>,
    pub refresh_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TagCreated {
    pub oid: String,
    pub target_oid: String,
    pub refresh_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Reset {
    pub previous_oid: String,
    pub oid: String,
    pub refresh_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Discarded {
    pub index_changed: bool,
    pub refresh_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StashSaved {
    pub oid: String,
    pub saved: bool,
    pub refresh_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StashApplied {
    pub oid: String,
    pub applied: bool,
    pub dropped: bool,
    pub needs_resolution: bool,
    pub refresh_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StashDropped {
    pub oid: String,
    pub dropped: bool,
    pub refresh_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NeedsResolution {
    pub needs_resolution: bool,
    pub refresh_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IntegrationCompleted {
    pub needs_resolution: bool,
    pub aborted: bool,
    pub merge_completed: bool,
    pub integration_completed: Option<String>,
    pub commit_oid: Option<String>,
    pub oid: Option<String>,
    pub refresh_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorktreeAdded {
    pub name: String,
    pub path: Path,
    pub open_required: bool,
    pub branch_created: bool,
    pub refresh_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorktreeRepaired {
    pub changed: bool,
    pub refresh_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorktreeRegistrationChanged {
    pub registration_only: bool,
    pub refresh_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Initialized {
    pub initialized: bool,
    pub path: Path,
    pub initial_branch: String,
    pub bare: bool,
    pub open_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Cloned {
    pub cloned: bool,
    pub path: Path,
    pub bare: bool,
    pub oid: Option<String>,
    pub open_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OperationResult {
    Refresh(Refresh),
    Committed(Committed),
    Amended(Amended),
    TrackingChanged(TrackingChanged),
    CheckedOut(CheckedOut),
    RemoteRenamed(RemoteRenamed),
    FastForwarded(FastForwarded),
    LeaseMatched(LeaseMatched),
    RemoteDeleted(RemoteDeleted),
    TagCreated(TagCreated),
    Reset(Reset),
    Discarded(Discarded),
    StashSaved(StashSaved),
    StashApplied(StashApplied),
    StashDropped(StashDropped),
    NeedsResolution(NeedsResolution),
    IntegrationCompleted(IntegrationCompleted),
    WorktreeAdded(WorktreeAdded),
    WorktreeRepaired(WorktreeRepaired),
    WorktreeRegistrationChanged(WorktreeRegistrationChanged),
    Initialized(Initialized),
    Cloned(Cloned),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum Reply {
    Open(Box<Repository>),
    Close(Box<Closed>),
    StatusSummary(Box<StatusSummary>),
    Status(Box<Page<StatusEntry, StatusSummary>>),
    Branches(Box<Page<Branch, TotalEntries>>),
    Worktrees(Box<Page<Worktree, WorktreeMetadata>>),
    Remotes(Box<Remotes>),
    Remote(Box<Remote>),
    RemoteNames(Box<Page<RemoteName, TotalEntries>>),
    RemoteRefs(Box<Page<RemoteReference, RemoteReferenceMetadata>>),
    Blob(Box<Blob>),
    BlobPage(Box<Page<BlobFragment, BlobMetadata>>),
    Stashes(Box<Page<Stash, StashMetadata>>),
    Tags(Box<Page<Tag, TotalEntries>>),
    Tag(Box<Tag>),
    History(Box<Page<Commit, HistoryMetadata>>),
    Commit(Box<Commit>),
    CommitFiles(Box<Page<Delta, DiffMetadata>>),
    DiffPage(Box<Page<FileFragment, DiffMetadata>>),
    CommitDiffPage(Box<Page<FileFragment, DiffMetadata>>),
    Operation(Box<Operation>),
    StreamComplete(Box<StreamComplete>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyKind {
    Open,
    Close,
    StatusSummary,
    Status,
    Branches,
    Worktrees,
    Remotes,
    Remote,
    RemoteNames,
    RemoteRefs,
    Blob,
    BlobPage,
    Stashes,
    Tags,
    Tag,
    History,
    Commit,
    CommitFiles,
    DiffPage,
    CommitDiffPage,
    Operation,
    StreamComplete,
}

impl ReplyKind {
    pub fn deserialize_result<'de, D: serde::Deserializer<'de>>(
        self,
        d: D,
    ) -> Result<Reply, D::Error> {
        match self {
            Self::Open => <Repository>::deserialize(d).map(Box::new).map(Reply::Open),
            Self::Close => <Closed>::deserialize(d).map(Box::new).map(Reply::Close),
            Self::StatusSummary => <StatusSummary>::deserialize(d)
                .map(Box::new)
                .map(Reply::StatusSummary),
            Self::Status => <Page<StatusEntry, StatusSummary>>::deserialize(d)
                .map(Box::new)
                .map(Reply::Status),
            Self::Branches => <Page<Branch, TotalEntries>>::deserialize(d)
                .map(Box::new)
                .map(Reply::Branches),
            Self::Worktrees => <Page<Worktree, WorktreeMetadata>>::deserialize(d)
                .map(Box::new)
                .map(Reply::Worktrees),
            Self::Remotes => <Remotes>::deserialize(d).map(Box::new).map(Reply::Remotes),
            Self::Remote => <Remote>::deserialize(d).map(Box::new).map(Reply::Remote),
            Self::RemoteNames => <Page<RemoteName, TotalEntries>>::deserialize(d)
                .map(Box::new)
                .map(Reply::RemoteNames),
            Self::RemoteRefs => <Page<RemoteReference, RemoteReferenceMetadata>>::deserialize(d)
                .map(Box::new)
                .map(Reply::RemoteRefs),
            Self::Blob => <Blob>::deserialize(d).map(Box::new).map(Reply::Blob),
            Self::BlobPage => <Page<BlobFragment, BlobMetadata>>::deserialize(d)
                .map(Box::new)
                .map(Reply::BlobPage),
            Self::Stashes => <Page<Stash, StashMetadata>>::deserialize(d)
                .map(Box::new)
                .map(Reply::Stashes),
            Self::Tags => <Page<Tag, TotalEntries>>::deserialize(d)
                .map(Box::new)
                .map(Reply::Tags),
            Self::Tag => <Tag>::deserialize(d).map(Box::new).map(Reply::Tag),
            Self::History => <Page<Commit, HistoryMetadata>>::deserialize(d)
                .map(Box::new)
                .map(Reply::History),
            Self::Commit => <Commit>::deserialize(d).map(Box::new).map(Reply::Commit),
            Self::CommitFiles => <Page<Delta, DiffMetadata>>::deserialize(d)
                .map(Box::new)
                .map(Reply::CommitFiles),
            Self::DiffPage => <Page<FileFragment, DiffMetadata>>::deserialize(d)
                .map(Box::new)
                .map(Reply::DiffPage),
            Self::CommitDiffPage => <Page<FileFragment, DiffMetadata>>::deserialize(d)
                .map(Box::new)
                .map(Reply::CommitDiffPage),
            Self::Operation => <Operation>::deserialize(d)
                .map(Box::new)
                .map(Reply::Operation),
            Self::StreamComplete => <StreamComplete>::deserialize(d)
                .map(Box::new)
                .map(Reply::StreamComplete),
        }
    }
}

impl Reply {
    pub fn kind(&self) -> ReplyKind {
        match self {
            Self::Open(_) => ReplyKind::Open,
            Self::Close(_) => ReplyKind::Close,
            Self::StatusSummary(_) => ReplyKind::StatusSummary,
            Self::Status(_) => ReplyKind::Status,
            Self::Branches(_) => ReplyKind::Branches,
            Self::Worktrees(_) => ReplyKind::Worktrees,
            Self::Remotes(_) => ReplyKind::Remotes,
            Self::Remote(_) => ReplyKind::Remote,
            Self::RemoteNames(_) => ReplyKind::RemoteNames,
            Self::RemoteRefs(_) => ReplyKind::RemoteRefs,
            Self::Blob(_) => ReplyKind::Blob,
            Self::BlobPage(_) => ReplyKind::BlobPage,
            Self::Stashes(_) => ReplyKind::Stashes,
            Self::Tags(_) => ReplyKind::Tags,
            Self::Tag(_) => ReplyKind::Tag,
            Self::History(_) => ReplyKind::History,
            Self::Commit(_) => ReplyKind::Commit,
            Self::CommitFiles(_) => ReplyKind::CommitFiles,
            Self::DiffPage(_) => ReplyKind::DiffPage,
            Self::CommitDiffPage(_) => ReplyKind::CommitDiffPage,
            Self::Operation(_) => ReplyKind::Operation,
            Self::StreamComplete(_) => ReplyKind::StreamComplete,
        }
    }
}

type WorkingLine = (
    u64,
    u64,
    bool,
    String,
    Option<u64>,
    Option<u64>,
    Binary,
    Option<String>,
);

#[derive(Deserialize)]
#[serde(untagged, deny_unknown_fields)]
enum LineRepresentation {
    #[serde(rename_all = "camelCase")]
    Object {
        line_index: u64,
        byte_offset: u64,
        line_complete: bool,
        origin: String,
        old_line: Option<u64>,
        new_line: Option<u64>,
        content_bytes_b64: Binary,
        #[serde(default)]
        id: Option<String>,
    },
    Working(WorkingLine),
    Historical((u64, u64, bool, String, Option<u64>, Option<u64>, Binary)),
}
impl From<LineRepresentation> for LineFragment {
    fn from(value: LineRepresentation) -> Self {
        match value {
            LineRepresentation::Object {
                line_index,
                byte_offset,
                line_complete,
                origin,
                old_line,
                new_line,
                content_bytes_b64,
                id,
            }
            | LineRepresentation::Working((
                line_index,
                byte_offset,
                line_complete,
                origin,
                old_line,
                new_line,
                content_bytes_b64,
                id,
            )) => Self {
                line_index,
                byte_offset,
                line_complete,
                origin,
                old_line,
                new_line,
                content_bytes_b64,
                id,
            },
            LineRepresentation::Historical((
                line_index,
                byte_offset,
                line_complete,
                origin,
                old_line,
                new_line,
                content_bytes_b64,
            )) => Self {
                line_index,
                byte_offset,
                line_complete,
                origin,
                old_line,
                new_line,
                content_bytes_b64,
                id: None,
            },
        }
    }
}
