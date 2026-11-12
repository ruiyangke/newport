//! Clipboard identities and policy shared by desktop control and agent IPC.
//! This module has no runtime, display or private-product dependencies.
use serde::{Deserialize, Serialize};
use std::{fmt, io, path::PathBuf};

pub mod exports;
pub mod imports;
pub mod service;
pub mod source;

pub const SERVICE_CAPABILITY: &str = "clipboard-service";
/// Every route gets a route result. Reads can be long: while the desktop
/// prepares `FILES_FORMAT` it sends empty not-done reply chunks as
/// keep-alives, and every wait treats one as progress.
pub const SERVICE_VERSIONS: &[u32] = &[1];
pub const FILES_FORMAT: &str = "application/x-newport-files";

/// Issued only by runtime worker admission. Debug output excludes the token.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Access {
    pub path: PathBuf,
    pub runtime: String,
    pub worker: String,
    pub token: String,
}
impl Access {
    pub fn validate(
        &self,
        worker_socket: &std::path::Path,
        runtime: &str,
        worker: &str,
    ) -> io::Result<()> {
        if self.path != worker_socket.with_file_name("clipboard.sock")
            || self.runtime != runtime
            || self.worker != worker
            || uuid::Uuid::parse_str(&self.token).is_err()
        {
            return Err(io::Error::other(
                "Clipboard admission does not match this worker runtime",
            ));
        }
        Ok(())
    }
}
impl fmt::Debug for Access {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClipboardAccess")
            .field("runtime", &self.runtime)
            .field("worker", &self.worker)
            .finish_non_exhaustive()
    }
}
/// A runtime-scoped metadata snapshot. Its revision is independent of the
/// desktop clipboard counter and cannot be reused after permission changes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Offer {
    pub runtime: String,
    pub revision: i64,
    pub origin: Origin,
    pub formats: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Origin {
    Empty,
    Host,
    Native { worker: String, revision: u64 },
}

/// Set only by the owning runtime control connection. Import permission is
/// independent. A native provider cannot authorize its own host exports.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Route {
    pub revision: u64,
    pub worker_id: Option<String>,
    pub exports: bool,
    pub focused: bool,
}
/// Sent only in response to the route operation. Retirement is
/// positive evidence of a previously admitted native worker, never an import
/// or export permission grant and never a classification of arbitrary errors.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RouteReply {
    pub revision: u64,
    pub worker_id: Option<String>,
    pub outcome: RouteOutcome,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RouteOutcome {
    Applied,
    Retired,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderLease {
    pub runtime: String,
    pub worker: String,
    pub epoch: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeRequest {
    pub lease: ProviderLease,
    pub id: u64,
    pub revision: u64,
    pub format: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderPoll {
    pub lease: Option<ProviderLease>,
    pub offer: Offer,
    pub requests: Vec<NativeRequest>,
}
/// Only the integration producer can use this lease. The desktop must also
/// recheck its focus and native clipboard change count before applying bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportOffer {
    pub offer: Offer,
    pub route_revision: u64,
}
/// An acknowledged copy of one native source. Focus authorizes creation; later
/// reads remain valid while the source, export permission and owning integration
/// connection remain current. A copied identifier never renews this authority.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopyLease {
    pub id: String,
    pub offer: ExportOffer,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Chunk {
    #[serde(with = "serde_bytes")]
    pub bytes: Vec<u8>,
    pub eof: bool,
}

pub mod helper;
pub mod provider;
