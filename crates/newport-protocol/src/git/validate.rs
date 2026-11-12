use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Invalid(pub &'static str);
impl std::fmt::Display for Invalid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid Git {}", self.0)
    }
}
impl std::error::Error for Invalid {}

pub(super) const MAX_ITEMS: usize = 10_000;
const MAX_FIELDS: usize = 262_144;
struct Budget {
    bytes: usize,
    fields: usize,
}
impl Budget {
    fn new(bytes: usize) -> Self {
        Self {
            bytes,
            fields: MAX_FIELDS,
        }
    }
    fn bytes(&mut self, length: usize) -> Result<(), Invalid> {
        self.bytes = self
            .bytes
            .checked_sub(length)
            .ok_or(Invalid("byte budget"))?;
        Ok(())
    }
    fn fields(&mut self, length: usize) -> Result<(), Invalid> {
        self.fields = self
            .fields
            .checked_sub(length)
            .ok_or(Invalid("field budget"))?;
        Ok(())
    }
}
trait Check {
    fn check(&self, budget: &mut Budget) -> Result<(), Invalid>;
}
impl Check for String {
    fn check(&self, budget: &mut Budget) -> Result<(), Invalid> {
        ensure(self.len() <= MAX_TEXT, "text length")?;
        budget.bytes(self.len())
    }
}
impl<T: Check> Check for Option<T> {
    fn check(&self, budget: &mut Budget) -> Result<(), Invalid> {
        self.as_ref()
            .map(|value| value.check(budget))
            .transpose()
            .map(|_| ())
    }
}
impl<T: Check> Check for Box<T> {
    fn check(&self, budget: &mut Budget) -> Result<(), Invalid> {
        self.as_ref().check(budget)
    }
}
impl<T: Check> Check for Vec<T> {
    fn check(&self, budget: &mut Budget) -> Result<(), Invalid> {
        ensure(self.len() <= MAX_ITEMS, "collection length")?;
        budget.fields(self.len())?;
        self.iter().try_for_each(|value| value.check(budget))
    }
}
impl<T: Check, M: Check> Check for Page<T, M> {
    fn check(&self, budget: &mut Budget) -> Result<(), Invalid> {
        self.snapshot.check(budget)?;
        self.entries.check(budget)?;
        self.next_cursor.check(budget)?;
        self.metadata.check(budget)
    }
}
fn ensure(condition: bool, name: &'static str) -> Result<(), Invalid> {
    condition.then_some(()).ok_or(Invalid(name))
}
fn identity(value: &str) -> Result<(), Invalid> {
    uuid::Uuid::parse_str(value)
        .map(|_| ())
        .map_err(|_| Invalid("identity"))
}
impl Check for Binary {
    fn check(&self, budget: &mut Budget) -> Result<(), Invalid> {
        ensure(self.0.len() <= MAX_DIFF, "binary length")?;
        budget.bytes(self.0.len())
    }
}
impl Check for Path {
    fn check(&self, budget: &mut Budget) -> Result<(), Invalid> {
        use base64::Engine;
        ensure(
            self.bytes_b64.len() <= MAX_DIFF * 4 / 3 + 4 && self.display.len() <= MAX_DIFF,
            "binary text length",
        )?;
        budget.bytes(self.bytes_b64.len())?;
        budget.bytes(self.display.len())?;
        base64::engine::general_purpose::STANDARD
            .decode(&self.bytes_b64)
            .map_err(|_| Invalid("binary text encoding"))?;
        Ok(())
    }
}
impl Check for Side {
    fn check(&self, budget: &mut Budget) -> Result<(), Invalid> {
        budget.fields(1)
    }
}
impl Check for ResetMode {
    fn check(&self, budget: &mut Budget) -> Result<(), Invalid> {
        budget.fields(1)
    }
}
impl Check for DiscardSource {
    fn check(&self, budget: &mut Budget) -> Result<(), Invalid> {
        budget.fields(1)
    }
}
impl Check for ConflictSide {
    fn check(&self, budget: &mut Budget) -> Result<(), Invalid> {
        budget.fields(1)
    }
}
impl Check for DiffLineEncoding {
    fn check(&self, budget: &mut Budget) -> Result<(), Invalid> {
        budget.fields(1)
    }
}
impl Check for OperationState {
    fn check(&self, budget: &mut Budget) -> Result<(), Invalid> {
        budget.fields(1)
    }
}
impl Check for CheckoutTarget {
    fn check(&self, budget: &mut Budget) -> Result<(), Invalid> {
        match self {
            Self::Branch { name, expected_oid } => {
                name.check(budget)?;
                expected_oid.check(budget)?;
            }
            Self::Detached { oid } => {
                oid.check(budget)?;
            }
        }
        Ok(())
    }
}
impl Check for Action {
    fn check(&self, budget: &mut Budget) -> Result<(), Invalid> {
        match self {
            Self::ConflictResolve {
                entry_ids,
                side,
                expected_oid,
            } => {
                entry_ids.check(budget)?;
                side.check(budget)?;
                expected_oid.check(budget)?;
            }
            Self::Discard {
                entry_ids,
                source,
                hunks,
            } => {
                entry_ids.check(budget)?;
                source.check(budget)?;
                hunks.check(budget)?;
            }
            Self::Reset {
                target_oid,
                expected_oid,
                mode,
            } => {
                target_oid.check(budget)?;
                expected_oid.check(budget)?;
                mode.check(budget)?;
            }
            Self::Rebase {
                upstream_oid,
                onto_oid,
                committer,
            } => {
                upstream_oid.check(budget)?;
                onto_oid.check(budget)?;
                committer.check(budget)?;
            }
            Self::WorktreeRepair { name, path } => {
                name.check(budget)?;
                path.decode().map_err(|_| Invalid("path"))?;
                path.check(budget)?;
            }
            Self::WorktreeRemove { name } => {
                name.check(budget)?;
            }
            Self::WorktreePrune { name } => {
                name.check(budget)?;
            }
            Self::WorktreeAdd {
                name,
                path,
                branch,
                expected_oid,
                ..
            } => {
                name.check(budget)?;
                path.decode().map_err(|_| Invalid("path"))?;
                path.check(budget)?;
                branch.check(budget)?;
                expected_oid.check(budget)?;
            }
            Self::WorktreeLock { name, reason } => {
                name.check(budget)?;
                reason.check(budget)?;
            }
            Self::WorktreeUnlock { name } => {
                name.check(budget)?;
            }
            Self::IntegrationSkip {} => {}
            Self::BranchSetUpstream {
                name,
                expected_oid,
                expected_token,
                upstream,
            } => {
                name.check(budget)?;
                expected_oid.check(budget)?;
                expected_token.check(budget)?;
                upstream.check(budget)?;
            }
            Self::CherryPick {
                target_oid, author, ..
            } => {
                target_oid.check(budget)?;
                author.check(budget)?;
            }
            Self::Revert {
                target_oid, author, ..
            } => {
                target_oid.check(budget)?;
                author.check(budget)?;
            }
            Self::IntegrationContinue { message, author } => {
                message.check(budget)?;
                author.check(budget)?;
            }
            Self::IntegrationAbort {} => {}
            Self::TagDeleteRemote {
                remote,
                expected_token,
                name,
                expected_oid,
            } => {
                remote.check(budget)?;
                expected_token.check(budget)?;
                name.check(budget)?;
                expected_oid.check(budget)?;
            }
            Self::TagPush {
                remote,
                expected_token,
                name,
                expected_oid,
            } => {
                remote.check(budget)?;
                expected_token.check(budget)?;
                name.check(budget)?;
                expected_oid.check(budget)?;
            }
            Self::TagCreate {
                name,
                target_oid,
                annotation,
            } => {
                name.check(budget)?;
                target_oid.check(budget)?;
                annotation.check(budget)?;
            }
            Self::TagDelete { name, expected_oid } => {
                name.check(budget)?;
                expected_oid.check(budget)?;
            }
            Self::StashSave {
                message, author, ..
            } => {
                message.check(budget)?;
                author.check(budget)?;
            }
            Self::StashApply {
                oid,
                index,
                expected_token,
                ..
            } => {
                oid.check(budget)?;
                index.check(budget)?;
                expected_token.check(budget)?;
            }
            Self::StashPop {
                oid,
                index,
                expected_token,
                ..
            } => {
                oid.check(budget)?;
                index.check(budget)?;
                expected_token.check(budget)?;
            }
            Self::StashDrop {
                oid,
                index,
                expected_token,
            } => {
                oid.check(budget)?;
                index.check(budget)?;
                expected_token.check(budget)?;
            }
            Self::MergeAbort {} => {}
            Self::Merge { target_oid } => {
                target_oid.check(budget)?;
            }
            Self::PullFastForward {
                remote,
                expected_token,
                remote_branch,
            } => {
                remote.check(budget)?;
                expected_token.check(budget)?;
                remote_branch.check(budget)?;
            }
            Self::FastForward { target_oid } => {
                target_oid.check(budget)?;
            }
            Self::RemoteAdd { name, url } => {
                name.check(budget)?;
                url.check(budget)?;
            }
            Self::RemoteRename {
                name,
                new_name,
                expected_token,
            } => {
                name.check(budget)?;
                new_name.check(budget)?;
                expected_token.check(budget)?;
            }
            Self::RemoteSetUrl {
                name,
                url,
                expected_token,
            } => {
                name.check(budget)?;
                url.check(budget)?;
                expected_token.check(budget)?;
            }
            Self::RemoteRemove {
                name,
                expected_token,
            } => {
                name.check(budget)?;
                expected_token.check(budget)?;
            }
            Self::ConfigSet {
                expected_token,
                fetch_prune,
                user_name,
                user_email,
            } => {
                expected_token.check(budget)?;
                fetch_prune.check(budget)?;
                user_name.check(budget)?;
                user_email.check(budget)?;
            }
            Self::Fetch {
                remote,
                expected_token,
                ..
            } => {
                remote.check(budget)?;
                expected_token.check(budget)?;
            }
            Self::BranchDeleteRemote {
                remote,
                expected_token,
                branch,
                expected_oid,
            } => {
                remote.check(budget)?;
                expected_token.check(budget)?;
                branch.check(budget)?;
                expected_oid.check(budget)?;
            }
            Self::PushWithLease {
                remote,
                expected_token,
                branch,
                expected_oid,
                destination_branch,
                expected_remote_oid,
            } => {
                remote.check(budget)?;
                expected_token.check(budget)?;
                branch.check(budget)?;
                expected_oid.check(budget)?;
                destination_branch.check(budget)?;
                expected_remote_oid.check(budget)?;
            }
            Self::Push {
                remote,
                expected_token,
                branch,
                expected_oid,
                destination_branch,
            } => {
                remote.check(budget)?;
                expected_token.check(budget)?;
                branch.check(budget)?;
                expected_oid.check(budget)?;
                destination_branch.check(budget)?;
            }
            Self::Checkout { target } => {
                target.check(budget)?;
            }
            Self::BranchCreate { name, start_oid } => {
                name.check(budget)?;
                start_oid.check(budget)?;
            }
            Self::BranchRename {
                name,
                new_name,
                expected_oid,
            } => {
                name.check(budget)?;
                new_name.check(budget)?;
                expected_oid.check(budget)?;
            }
            Self::BranchDelete {
                name, expected_oid, ..
            } => {
                name.check(budget)?;
                expected_oid.check(budget)?;
            }
            Self::Amend {
                expected_oid,
                message,
                committer,
                author,
            } => {
                expected_oid.check(budget)?;
                message.check(budget)?;
                committer.check(budget)?;
                author.check(budget)?;
            }
            Self::Commit { message, author } => {
                message.check(budget)?;
                author.check(budget)?;
            }
            Self::Stage { entry_ids, hunks } => {
                entry_ids.check(budget)?;
                hunks.check(budget)?;
            }
            Self::Unstage { entry_ids, hunks } => {
                entry_ids.check(budget)?;
                hunks.check(budget)?;
            }
        }
        Ok(())
    }
}
impl Check for Request {
    fn check(&self, budget: &mut Budget) -> Result<(), Invalid> {
        match self {
            Self::Clone {
                operation_id,
                url,
                path,
                branch,
                ..
            } => {
                identity(operation_id)?;
                operation_id.check(budget)?;
                url.check(budget)?;
                path.decode().map_err(|_| Invalid("path"))?;
                path.check(budget)?;
                branch.check(budget)?;
            }
            Self::Init {
                operation_id,
                path,
                initial_branch,
            } => {
                identity(operation_id)?;
                operation_id.check(budget)?;
                path.decode().map_err(|_| Invalid("path"))?;
                path.check(budget)?;
                initial_branch.check(budget)?;
            }
            Self::Tag { repo_id, oid } => {
                repo_id.check(budget)?;
                oid.check(budget)?;
            }
            Self::Tags {
                repo_id,
                page_size,
                cursor,
                message_bytes,
            } => {
                repo_id.check(budget)?;
                ensure((1..=200).contains(page_size), "page size")?;
                cursor.check(budget)?;
                message_bytes.check(budget)?;
            }
            Self::Stashes {
                repo_id,
                page_size,
                cursor,
            } => {
                repo_id.check(budget)?;
                ensure((1..=200).contains(page_size), "page size")?;
                cursor.check(budget)?;
            }
            Self::BlobPage {
                repo_id,
                oid,
                max_bytes,
                cursor,
            } => {
                repo_id.check(budget)?;
                oid.check(budget)?;
                max_bytes.check(budget)?;
                cursor.check(budget)?;
            }
            Self::Blob { repo_id, oid } => {
                repo_id.check(budget)?;
                oid.check(budget)?;
            }
            Self::RemoteRefs {
                repo_id,
                remote,
                expected_token,
                filter,
                page_size,
                cursor,
                ..
            } => {
                repo_id.check(budget)?;
                remote.check(budget)?;
                expected_token.check(budget)?;
                filter.check(budget)?;
                ensure((1..=200).contains(page_size), "page size")?;
                cursor.check(budget)?;
            }
            Self::RemoteNames {
                repo_id,
                filter,
                page_size,
                cursor,
            } => {
                repo_id.check(budget)?;
                filter.check(budget)?;
                ensure((1..=200).contains(page_size), "page size")?;
                cursor.check(budget)?;
            }
            Self::Remote { repo_id, name } => {
                repo_id.check(budget)?;
                name.check(budget)?;
            }
            Self::Remotes { repo_id } => {
                repo_id.check(budget)?;
            }
            Self::Start {
                operation_id,
                repo_id,
                expected_snapshot,
                action,
            } => {
                identity(operation_id)?;
                operation_id.check(budget)?;
                repo_id.check(budget)?;
                expected_snapshot.check(budget)?;
                action.check(budget)?;
            }
            Self::Get { operation_id } => {
                identity(operation_id)?;
                operation_id.check(budget)?;
            }
            Self::Review { operation_id } => {
                identity(operation_id)?;
                operation_id.check(budget)?;
            }
            Self::Open { path } => {
                path.decode().map_err(|_| Invalid("path"))?;
                path.check(budget)?;
            }
            Self::Close { repo_id } => {
                repo_id.check(budget)?;
            }
            Self::StatusSummary { repo_id, path } => {
                repo_id.check(budget)?;
                if let Some(path) = path {
                    path.decode().map_err(|_| Invalid("path"))?;
                }
                path.check(budget)?;
            }
            Self::Status {
                repo_id,
                filter,
                page_size,
                cursor,
            } => {
                repo_id.check(budget)?;
                filter.check(budget)?;
                ensure((1..=200).contains(page_size), "page size")?;
                cursor.check(budget)?;
            }
            Self::Worktrees {
                repo_id,
                page_size,
                cursor,
                filter,
                branch,
                name,
                at_snapshot,
            } => {
                repo_id.check(budget)?;
                ensure((1..=200).contains(page_size), "page size")?;
                cursor.check(budget)?;
                filter.check(budget)?;
                branch.check(budget)?;
                name.check(budget)?;
                at_snapshot.check(budget)?;
            }
            Self::Branches {
                repo_id,
                page_size,
                cursor,
                filter,
                branch_kind,
            } => {
                repo_id.check(budget)?;
                ensure((1..=200).contains(page_size), "page size")?;
                cursor.check(budget)?;
                filter.check(budget)?;
                branch_kind.check(budget)?;
            }
            Self::History {
                repo_id,
                page_size,
                cursor,
                revision,
                message_bytes,
            } => {
                repo_id.check(budget)?;
                ensure((1..=200).contains(page_size), "page size")?;
                cursor.check(budget)?;
                revision.check(budget)?;
                message_bytes.check(budget)?;
            }
            Self::Commit {
                repo_id,
                commit_oid,
            } => {
                repo_id.check(budget)?;
                commit_oid.check(budget)?;
            }
            Self::CommitFiles {
                repo_id,
                commit_oid,
                page_size,
                cursor,
                ..
            } => {
                repo_id.check(budget)?;
                commit_oid.check(budget)?;
                ensure((1..=200).contains(page_size), "page size")?;
                cursor.check(budget)?;
            }
            Self::CommitDiff {
                path,
                repo_id,
                commit_oid,
                context_lines,
                ..
            } => {
                if let Some(path) = path {
                    path.decode().map_err(|_| Invalid("path"))?;
                }
                path.check(budget)?;
                repo_id.check(budget)?;
                commit_oid.check(budget)?;
                ensure(*context_lines <= 100, "diff context")?;
            }
            Self::CommitDiffPage {
                repo_id,
                commit_oid,
                path,
                context_lines,
                page_size,
                max_bytes,
                cursor,
                line_encoding,
                ..
            } => {
                repo_id.check(budget)?;
                commit_oid.check(budget)?;
                path.decode().map_err(|_| Invalid("path"))?;
                path.check(budget)?;
                ensure(*context_lines <= 100, "diff context")?;
                ensure((1..=5000).contains(page_size), "page size")?;
                max_bytes.check(budget)?;
                cursor.check(budget)?;
                line_encoding.check(budget)?;
            }
            Self::DiffPage {
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
                repo_id.check(budget)?;
                snapshot.check(budget)?;
                entry_id.check(budget)?;
                side.check(budget)?;
                ensure(*context_lines <= 100, "diff context")?;
                ensure((1..=5000).contains(page_size), "page size")?;
                max_bytes.check(budget)?;
                cursor.check(budget)?;
                line_encoding.check(budget)?;
            }
            Self::Diff {
                repo_id,
                snapshot,
                entry_id,
                side,
                context_lines,
            } => {
                repo_id.check(budget)?;
                snapshot.check(budget)?;
                entry_id.check(budget)?;
                side.check(budget)?;
                ensure(*context_lines <= 100, "diff context")?;
            }
        }
        Ok(())
    }
}
impl Check for OperationResult {
    fn check(&self, budget: &mut Budget) -> Result<(), Invalid> {
        match self {
            Self::Refresh(value) => value.check(budget),
            Self::Committed(value) => value.check(budget),
            Self::Amended(value) => value.check(budget),
            Self::TrackingChanged(value) => value.check(budget),
            Self::CheckedOut(value) => value.check(budget),
            Self::RemoteRenamed(value) => value.check(budget),
            Self::FastForwarded(value) => value.check(budget),
            Self::LeaseMatched(value) => value.check(budget),
            Self::RemoteDeleted(value) => value.check(budget),
            Self::TagCreated(value) => value.check(budget),
            Self::Reset(value) => value.check(budget),
            Self::Discarded(value) => value.check(budget),
            Self::StashSaved(value) => value.check(budget),
            Self::StashApplied(value) => value.check(budget),
            Self::StashDropped(value) => value.check(budget),
            Self::NeedsResolution(value) => value.check(budget),
            Self::IntegrationCompleted(value) => value.check(budget),
            Self::WorktreeAdded(value) => value.check(budget),
            Self::WorktreeRepaired(value) => value.check(budget),
            Self::WorktreeRegistrationChanged(value) => value.check(budget),
            Self::Initialized(value) => value.check(budget),
            Self::Cloned(value) => value.check(budget),
        }
    }
}
impl Check for Reply {
    fn check(&self, budget: &mut Budget) -> Result<(), Invalid> {
        match self {
            Self::Open(value) => value.check(budget),
            Self::Close(value) => value.check(budget),
            Self::StatusSummary(value) => value.check(budget),
            Self::Status(value) => value.check(budget),
            Self::Branches(value) => value.check(budget),
            Self::Worktrees(value) => value.check(budget),
            Self::Remotes(value) => value.check(budget),
            Self::Remote(value) => value.check(budget),
            Self::RemoteNames(value) => value.check(budget),
            Self::RemoteRefs(value) => value.check(budget),
            Self::Blob(value) => value.check(budget),
            Self::BlobPage(value) => value.check(budget),
            Self::Stashes(value) => value.check(budget),
            Self::Tags(value) => value.check(budget),
            Self::Tag(value) => value.check(budget),
            Self::History(value) => value.check(budget),
            Self::Commit(value) => value.check(budget),
            Self::CommitFiles(value) => value.check(budget),
            Self::DiffPage(value) => value.check(budget),
            Self::CommitDiffPage(value) => value.check(budget),
            Self::Operation(value) => value.check(budget),
            Self::StreamComplete(value) => value.check(budget),
        }
    }
}
impl Check for Error {
    fn check(&self, budget: &mut Budget) -> Result<(), Invalid> {
        ensure(!self.code.is_empty() && self.code.len() <= 64, "error code")?;
        ensure(self.retry == "never", "retry policy")?;
        self.message.check(budget)
    }
}
impl Request {
    pub fn validate(&self) -> Result<(), Invalid> {
        let budget = &mut Budget::new(MAX_FRAME);
        self.check(budget)?;
        if let Self::StatusSummary { repo_id, path } = self {
            ensure(repo_id.is_some() != path.is_some(), "repository selector")?;
        }
        Ok(())
    }
}
impl Reply {
    pub fn validate(&self) -> Result<(), Invalid> {
        let budget = &mut Budget::new(MAX_FRAME);
        self.check(budget)
    }
}
impl Diff {
    pub fn validate(&self) -> Result<(), Invalid> {
        let budget = &mut Budget::new(MAX_DIFF);
        self.check(budget)
    }
}
impl Call {
    pub fn validate(&self) -> Result<(), Invalid> {
        identity(&self.id)?;
        self.request.validate()
    }
}
impl Response {
    pub fn validate_for(&self, call: &Call) -> Result<(), Invalid> {
        self.validate()?;
        ensure(self.id == call.id, "response identity")?;
        if let Ok(reply) = &self.result {
            ensure(
                reply.kind() == call.request.reply_kind(),
                "response operation",
            )?;
            if let Reply::Operation(record) = reply {
                let expected = match call.request.as_ref() {
                    Request::Start { operation_id, .. }
                    | Request::Get { operation_id }
                    | Request::Review { operation_id }
                    | Request::Init { operation_id, .. }
                    | Request::Clone { operation_id, .. } => operation_id,
                    _ => return Err(Invalid("operation receipt")),
                };
                ensure(
                    record.operation_id == *expected,
                    "operation receipt identity",
                )?;
            }
        }
        Ok(())
    }
    pub fn validate(&self) -> Result<(), Invalid> {
        let budget = &mut Budget::new(MAX_FRAME);
        identity(&self.id)?;
        match &self.result {
            Ok(reply) => reply.check(budget),
            Err(error) => error.check(budget),
        }
    }
}
fn nonce(value: &str) -> Result<(), Invalid> {
    ensure(!value.is_empty() && value.len() <= 128, "keepalive nonce")
}
impl ClientMessage {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Configure(value) => {
                identity(&value.id)?;
                identity(&value.client_id)?;
                ensure(value.client_version.len() <= 64, "client version")
            }
            Self::Call(value) => value.validate(),
            Self::Ping(value) => nonce(value),
            Self::Ack(value) => {
                identity(&value.stream_id)?;
                ensure(value.seq > 0, "acknowledgement sequence")
            }
        }
    }
}
impl ServerMessage {
    pub fn validate(&self) -> Result<(), Invalid> {
        let budget = &mut Budget::new(MAX_FRAME);
        match self {
            Self::Ready(value) => {
                identity(&value.id)?;
                value.capabilities.check(budget)?;
                ensure(value.limits == Limits::default(), "limits")
            }
            Self::Response(value) => value.validate(),
            Self::Pong(value) => nonce(value),
            Self::Begin(value) => {
                identity(&value.id)?;
                identity(&value.stream_id)?;
                value.snapshot.check(budget)
            }
            Self::Chunk(value) => {
                identity(&value.id)?;
                identity(&value.stream_id)?;
                ensure(
                    value.seq > 0 && !value.data.0.is_empty() && value.data.0.len() <= CHUNK_SIZE,
                    "stream chunk",
                )
            }
            Self::Log(value) => {
                identity(&value.id)?;
                ensure(
                    value.entry.command.len() <= 256 && value.entry.output.len() <= 32768,
                    "command log",
                )
            }
            Self::Failed(value) => value.check(budget),
        }
    }
}
macro_rules! record_fields {
    ($($name:ty => [$($field:ident),*];)*) => {$ (
        impl Check for $name {
            fn check(&self, budget: &mut Budget) -> Result<(), Invalid> {
                budget.fields(1)?;
                $(self.$field.check(budget)?;)*
                Ok(())
            }
        }
    )*};
}
record_fields! {
    StatusFilter => [text, group];
    Author => [name, email];
    TagAnnotation => [message, author];
    HunkSelection => [ids, lines];
    CommandLog => [command, exit_code, output];
    ObjectId => [algorithm, hex];
    TagObjectId => [format, hex];
    Head => [oid, name];
    IntegrationState => [kind];
    RepositoryCapabilities => [];
    Repository => [repo_id, common_repo_id, root, object_format, head, operation_state, integration, capabilities];
    GroupCounts => [];
    Upstream => [remote, reference];
    StatusSummary => [head, operation_state, integration, ahead, behind, basis, upstream_ref, upstream, group_counts, matched_entries];
    ConflictEntry => [oid, path];
    Conflict => [base, ours, theirs];
    StatusEntry => [entry_id, path, old_path, conflict];
    ConfigurationEntry => [value, level];
    TrackingConfiguration => [remote, merge];
    Tracking => [token, configuration];
    Branch => [name, reference, oid, upstream, tracking];
    TotalEntries => [];
    Worktree => [name, kind, path, git_dir, state, head, lock_reason];
    WorktreeMetadata => [list_token, current, main];
    Remote => [name, url, push_url, token];
    RemoteAuthentication => [ssh, https];
    RepositoryConfiguration => [user_name, user_email, token];
    Remotes => [entries, authentication, configuration];
    RemoteName => [name];
    RemoteReference => [reference, kind, oid, symbolic_target];
    RemoteReferenceMetadata => [remote, remote_token, basis];
    Blob => [oid, bytes_b64];
    BlobFragment => [bytes_b64];
    BlobMetadata => [oid];
    Stash => [oid, previous_oid, message];
    StashMetadata => [list_token];
    Commit => [oid, parents, message, author];
    HistoryMetadata => [resolved_revision, truncated];
    Tagger => [name, email];
    Tag => [name, reference, symbolic_target, oid, object_type, target_oid, peeled_oid, peeled_type, message, message_truncated, tagger];
    Delta => [old_path, new_path, old_oid, new_oid, status];
    DiffMetadata => [commit_oid, parent_oid, parent_index, parents, source_snapshot, entry_id, side, context_lines, selected_path, truncated, total_files, total_units, has_omissions];
    DiffLine => [id, origin, old_line, new_line, content];
    DiffHunk => [id, lines];
    DiffFile => [old_path, new_path, old_oid, new_oid, status, hunks];
    Diff => [files, commit_oid, parent_oid, parent_index, parents, source_snapshot, entry_id, side, context_lines, selected_path, truncated, total_files, total_units, has_omissions];
    LineFragment => [origin, old_line, new_line, content_bytes_b64, id];
    HunkFragment => [id, lines];
    FileFragment => [old_path, new_path, old_oid, new_oid, status, omission_reason, hunks];
    Operation => [operation_id, repository, payload_hash, state, result, error];
    Closed => [];
    StreamComplete => [];
    Refresh => [];
    Committed => [commit_oid, parent_oid];
    Amended => [replaced_oid, commit_oid];
    TrackingChanged => [tracking, upstream];
    CheckedOut => [oid];
    RemoteRenamed => [token];
    FastForwarded => [oid];
    LeaseMatched => [];
    RemoteDeleted => [oid, destination_tag];
    TagCreated => [oid, target_oid];
    Reset => [previous_oid, oid];
    Discarded => [];
    StashSaved => [oid];
    StashApplied => [oid];
    StashDropped => [oid];
    NeedsResolution => [];
    IntegrationCompleted => [integration_completed, commit_oid, oid];
    WorktreeAdded => [name, path];
    WorktreeRepaired => [];
    WorktreeRegistrationChanged => [];
    Initialized => [path, initial_branch];
    Cloned => [path, oid];
    Limits => [];
    Capabilities => [methods, object_formats, features, actions];
    bool => [];
    u32 => [];
    u64 => [];
    i32 => [];
    i64 => [];
    usize => [];
}
