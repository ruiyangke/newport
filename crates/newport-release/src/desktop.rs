//! Machine-readable identity of the agent payloads a packaged desktop embeds.
//! `newport --agent-identity` prints it; release tools compare it with the
//! exact signed export before Developer ID signing and notarization.
use crate::agent_product::{Product, Variant};
use serde::{Deserialize, Serialize};

pub const REPORT_SCHEMA: u32 = 1;
pub const ARGUMENT: &str = "--agent-identity";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentReport {
    pub schema: u32,
    /// The compiled variant of the bundled payload pair.
    pub variant: Variant,
    pub payloads: Payloads,
    /// Absent for development builds, which embed no signed release metadata.
    pub envelope: Option<Envelope>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Payloads {
    pub x86_64: Payload,
    pub aarch64: Payload,
}

/// The uncompressed payload the desktop accepts from its bundled resource.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Payload {
    pub sha256: String,
    pub size: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Envelope {
    /// SHA-256 of the exact embedded `needle-release.json` bytes.
    pub sha256: String,
    pub size: u64,
    /// The signature-verified release version bound to both payloads.
    pub version: String,
    /// The signed schema-2 Product; absent for schema-1 core releases.
    pub product: Option<Product>,
}
