//! Typed desktop git contracts.
#[derive(Clone, Debug)]
pub struct CollectedDiff {
    pub snapshot: String,
    pub diff: crate::git::Diff,
}

#[derive(Clone, Debug)]
pub enum GitResult {
    Reply(Box<crate::git::Reply>),
    Diff(Box<crate::desktop::git::CollectedDiff>),
}

#[derive(Clone, Debug)]
pub struct Refreshed {
    pub repo_id: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReceiptState {
    Pending,
    Succeeded,
    Failed,
    Rejected,
    NeedsResolution,
    OutcomeUnknown,
    ReviewedUnknown,
}

#[derive(Clone, Debug)]
pub struct Receipt {
    pub operation_id: uuid::Uuid,
    pub server: uuid::Uuid,
    pub action: String,
    pub state: crate::desktop::git::ReceiptState,
}

#[derive(Clone, Debug)]
pub struct Receipts {
    pub receipts: Vec<crate::desktop::git::Receipt>,
}

#[derive(Clone, Debug)]
pub struct RequestArgs {
    pub server: uuid::Uuid,
    pub request: crate::git::Request,
    /// Defaults to false when absent on the wire.
    pub retry_connection: bool,
}

#[derive(Clone, Debug)]
pub struct RefreshRepositoryArgs {
    pub server: uuid::Uuid,
    pub repo_id: String,
}

#[derive(Clone, Debug)]
pub struct PendingOperationsArgs {
    pub server: uuid::Uuid,
}

#[derive(Clone, Debug)]
pub struct AcknowledgeOperationArgs {
    pub server: uuid::Uuid,
    pub operation_id: uuid::Uuid,
}

#[derive(Clone, Debug)]
pub struct ReviewOperationArgs {
    pub server: uuid::Uuid,
    pub operation_id: uuid::Uuid,
}

#[derive(Clone, Debug)]
pub enum Operation {
    Request(Box<crate::desktop::git::RequestArgs>),
    RefreshRepository(Box<crate::desktop::git::RefreshRepositoryArgs>),
    PendingOperations(Box<crate::desktop::git::PendingOperationsArgs>),
    AcknowledgeOperation(Box<crate::desktop::git::AcknowledgeOperationArgs>),
    ReviewOperation(Box<crate::desktop::git::ReviewOperationArgs>),
}

#[derive(Clone, Debug)]
pub enum Reply {
    Request(Box<crate::desktop::git::GitResult>),
    RefreshRepository(Box<crate::desktop::git::Refreshed>),
    PendingOperations(Box<crate::desktop::git::Receipts>),
    AcknowledgeOperation(Box<crate::common::Present>),
    ReviewOperation(Box<crate::common::Present>),
}
