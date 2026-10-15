//! Versioned deployment receipt fields shared with the desktop.
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Removal remains an admission barrier until explicitly restored. Its receipts
/// are separate from deployments: a successful removal is not permission to run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RemovalState {
    Removing,
    Removed,
    Restoring,
    Restored,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemovalReceipt {
    pub operation_id: Uuid,
    pub build: String,
    pub state: RemovalState,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    #[default]
    Deployment,
    Helpers,
}
impl Kind {
    pub fn is_deployment(&self) -> bool {
        *self == Self::Deployment
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Intent {
    #[serde(default, skip_serializing_if = "Kind::is_deployment")]
    pub kind: Kind,
    pub operation_id: Uuid,
    pub candidate: String,
    pub previous: String,
    pub integrations: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum State {
    Prepared,
    Activated,
    Succeeded,
    RollingBack,
    RolledBack,
    NeedsResolution,
}
impl State {
    pub fn unsettled(self) -> bool {
        matches!(
            self,
            Self::Prepared | Self::Activated | Self::RollingBack | Self::NeedsResolution
        )
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Receipt {
    pub intent: Intent,
    pub state: State,
    /// On Succeeded, an optional non-fatal shell setup notice. Other states use
    /// this for errors. Retain the wire field for older receipt readers.
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollback_helpers: Option<bool>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Observation {
    pub receipt: Option<Receipt>,
    pub active_build: String,
    /// Evidence about the current entry point; never permission to rerun helpers.
    pub activation_observed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HelperReview {
    pub operation_id: Uuid,
    pub files: Vec<HelperFile>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HelperFile {
    pub index: usize,
    pub path: String,
    pub status: String,
    pub message: String,
    pub fingerprint: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HelperRemovalPlan {
    pub files: Vec<HelperFile>,
}

/// Read-only evidence, never a desktop recovery intent or installation consent.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HelperHistory {
    pub version: u32,
    pub build_sha256: String,
    pub ownership_recorded: bool,
    pub operations: Vec<StandaloneHelper>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HelperState {
    Prepared,
    Applied,
    RollingBack,
    RolledBack,
    NeedsResolution,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HelperOrigin {
    Standalone,
    Unattributed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StandaloneHelper {
    pub operation_id: Uuid,
    pub origin: HelperOrigin,
    pub state: HelperState,
    pub file_count: usize,
    /// Binds immutable ownership, excluding mutable recovery state and keep decisions.
    pub fingerprint: String,
    pub restore_blocked: Option<String>,
}
