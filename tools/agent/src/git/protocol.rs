//! Shared by the remote agent and Tauri. No Git engine or UI dependencies.
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, Read, Write};

pub const VERSION: u32 = 1;
pub const MAX_FRAME: usize = 1024 * 1024;
pub const MAX_DIFF: usize = 20 * 1024 * 1024;
pub const CHUNK_SIZE: usize = 64 * 1024;
pub const METHODS: &[&str] = &[
    "repo.open",
    "repo.init",
    "repo.clone",
    "repo.close",
    "repo.status",
    "repo.branches",
    "repo.worktrees",
    "repo.remotes",
    "repo.remote_refs",
    "repo.blob",
    "repo.stashes",
    "repo.tags",
    "repo.history",
    "repo.diff",
    "repo.commit_diff",
    "repo.commit_files",
    "operation.start",
    "operation.get",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
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
        let bytes = STANDARD
            .decode(&self.bytes_b64)
            .map_err(|_| Error::invalid("Invalid path encoding."))?;
        if bytes.is_empty() || bytes.len() > 4096 || bytes.contains(&0) {
            return Err(Error::invalid("Invalid path."));
        }
        Ok(bytes)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Error {
    pub code: String,
    pub message: String,
    pub retry: String,
}
impl Error {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retry: "never".into(),
        }
    }
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new("INVALID_REQUEST", message)
    }
    pub fn transport(message: impl Into<String>) -> Self {
        Self::new("TRANSPORT_ERROR", message)
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for Error {}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Author {
    pub name: String,
    pub email: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TagAnnotation {
    pub message: String,
    #[serde(default)]
    pub author: Option<Author>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HunkSelection {
    pub ids: Vec<String>,
    /// Server-generated identifiers for individual changed lines inside the
    /// selected hunks. Omitted selections keep whole-hunk behavior and the
    /// original payload hashes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<Vec<String>>,
    pub context_lines: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
        /// Omitting it preserves whole-file behavior and legacy payload hashes.
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    #[serde(rename = "repo.tags")]
    Tags {
        #[serde(rename = "repoId")]
        repo_id: String,
        #[serde(rename = "pageSize", default = "page_size")]
        page_size: usize,
        #[serde(default)]
        cursor: Option<String>,
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
        #[serde(default = "page_size", rename = "pageSize")]
        page_size: usize,
        #[serde(default)]
        cursor: Option<String>,
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
    #[serde(rename = "repo.open")]
    Open { path: Path },
    #[serde(rename = "repo.close")]
    Close {
        #[serde(rename = "repoId")]
        repo_id: String,
    },
    #[serde(rename = "repo.status")]
    Status {
        #[serde(rename = "repoId")]
        repo_id: String,
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
    },
    #[serde(rename = "repo.branches")]
    Branches {
        #[serde(rename = "repoId")]
        repo_id: String,
        #[serde(default = "page_size", rename = "pageSize")]
        page_size: usize,
        #[serde(default)]
        cursor: Option<String>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Message {
    Hello {
        protocol: String,
        versions: Vec<u32>,
        #[serde(rename = "instanceId")]
        instance_id: String,
        limits: Value,
    },
    Initialize {
        id: String,
        version: u32,
        #[serde(rename = "clientId")]
        client_id: String,
        #[serde(rename = "clientVersion")]
        client_version: String,
    },
    Ready {
        id: String,
        version: u32,
        capabilities: Value,
    },
    Request {
        id: String,
        method: String,
        params: Value,
    },
    Response {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<Error>,
    },
    Ping {
        nonce: String,
    },
    Pong {
        nonce: String,
    },
    #[serde(rename = "stream.begin")]
    Begin {
        id: String,
        #[serde(rename = "streamId")]
        stream_id: String,
        snapshot: String,
    },
    #[serde(rename = "stream.chunk")]
    Chunk {
        id: String,
        #[serde(rename = "streamId")]
        stream_id: String,
        seq: u32,
        #[serde(rename = "bytesB64")]
        bytes_b64: String,
    },
    #[serde(rename = "stream.ack")]
    Ack {
        #[serde(rename = "streamId")]
        stream_id: String,
        seq: u32,
    },
}
impl Message {
    pub fn success(id: String, result: Value) -> Self {
        Self::Response {
            id,
            result: Some(result),
            error: None,
        }
    }
    pub fn failure(id: String, error: Error) -> Self {
        Self::Response {
            id,
            result: None,
            error: Some(error),
        }
    }
}
fn invalid(error: impl ToString) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}
pub fn payload_length(header: [u8; 5]) -> io::Result<usize> {
    let length = u32::from_be_bytes(header[1..].try_into().unwrap()) as usize;
    if header[0] != b'J' || length == 0 || length > MAX_FRAME {
        return Err(invalid("Invalid Git frame header"));
    }
    Ok(length)
}
pub fn encode(message: &Message) -> io::Result<Vec<u8>> {
    let bytes = serde_json::to_vec(message).map_err(invalid)?;
    if bytes.len() > MAX_FRAME {
        return Err(invalid("Git frame too large"));
    }
    let mut frame = vec![b'J'];
    frame.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    frame.extend(bytes);
    Ok(frame)
}
pub fn decode(bytes: &[u8]) -> io::Result<Message> {
    if bytes.len() > MAX_FRAME {
        return Err(invalid("Git frame too large"));
    }
    let unique: Unique = serde_json::from_slice(bytes).map_err(invalid)?;
    if depth(&unique.0) > 32 {
        return Err(invalid("JSON nesting limit exceeded"));
    }
    let message = serde_json::from_value(unique.0).map_err(invalid)?;
    if let Message::Response { result, error, .. } = &message {
        if result.is_some() == error.is_some() {
            return Err(invalid("Expected exactly one result or error"));
        }
    }
    Ok(message)
}
pub fn read(input: &mut impl Read) -> io::Result<Message> {
    let mut header = [0; 5];
    input.read_exact(&mut header)?;
    let mut bytes = vec![0; payload_length(header)?];
    input.read_exact(&mut bytes)?;
    decode(&bytes)
}
pub fn write(output: &mut impl Write, message: &Message) -> io::Result<()> {
    output.write_all(&encode(message)?)?;
    output.flush()
}
fn depth(value: &Value) -> usize {
    match value {
        Value::Array(a) => 1 + a.iter().map(depth).max().unwrap_or(0),
        Value::Object(o) => 1 + o.values().map(depth).max().unwrap_or(0),
        _ => 0,
    }
}
// Preserve strictness inside untyped parameter maps as well as typed envelopes.
struct Unique(Value);
impl<'de> Deserialize<'de> for Unique {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = Unique;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("JSON with unique keys")
            }
            fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Unique, E> {
                Ok(Unique(v.into()))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Unique, E> {
                Ok(Unique(v.into()))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Unique, E> {
                Ok(Unique(v.into()))
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Unique, E> {
                Ok(Unique(v.into()))
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Unique, E> {
                Ok(Unique(v.into()))
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Unique, E> {
                Ok(Unique(Value::Null))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut a: A) -> Result<Unique, A::Error> {
                let mut values = Vec::new();
                while let Some(Unique(v)) = a.next_element()? {
                    values.push(v);
                }
                Ok(Unique(Value::Array(values)))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(self, mut a: A) -> Result<Unique, A::Error> {
                let mut values = serde_json::Map::new();
                while let Some(k) = a.next_key::<String>()? {
                    if values.contains_key(&k) {
                        return Err(serde::de::Error::custom("Duplicate JSON key"));
                    }
                    let Unique(v) = a.next_value()?;
                    values.insert(k, v);
                }
                Ok(Unique(Value::Object(values)))
            }
        }
        d.deserialize_any(Visitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_bad_frames_and_ambiguous_json() {
        assert!(payload_length([b'J', 255, 255, 255, 255]).is_err());
        assert!(payload_length([b'X', 0, 0, 0, 4]).is_err());
        assert!(read(&mut &b"J\0\0\0\x08{}"[..]).is_err());
        assert!(decode(br#"{"type":"ping","nonce":"a","nonce":"b"}"#).is_err());
        assert!(decode(br#"{"type":"response","id":"a"}"#).is_err());
        assert!(decode(format!("{}0{}", "[".repeat(33), "]".repeat(33)).as_bytes()).is_err());
    }
    #[test]
    fn fragmented_frames_and_lossless_paths() {
        struct Fragments<'a>(&'a [u8]);
        impl Read for Fragments<'_> {
            fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
                let n = b.len().min(1);
                self.0.read(&mut b[..n])
            }
        }
        let data = encode(&Message::Ping { nonce: "a".into() }).unwrap();
        assert!(
            matches!(read(&mut Fragments(&data)).unwrap(), Message::Ping { nonce } if nonce=="a")
        );
        assert_eq!(Path::new(b"a\n\xff").decode().unwrap(), b"a\n\xff");
        assert!(Path::new(b"a\0b").decode().is_err());
    }
}
