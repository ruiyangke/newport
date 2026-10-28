//! Clipboard identities and policy shared by desktop control and agent IPC.
//! This module has no runtime, display or private-product dependencies.
use serde::{Deserialize, Serialize};
use std::{fmt, io, path::PathBuf};

pub const SERVICE_CAPABILITY: &str = "clipboard-service";
/// Version 2 adds the route result. Version 3 adds long reads: while the
/// desktop prepares `FILES_FORMAT` it sends empty not-done reply chunks as
/// keep-alives, and every wait treats one as progress.
pub const SERVICE_VERSIONS: &[u32] = &[1, 2, 3];
pub const FILES_FORMAT: &str = "application/x-newport-files";
pub const ROUTE_RESULT_VERSION: u32 = 2;

/// Issued only by runtime worker admission. Debug output excludes the token.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Access {
    pub(crate) path: PathBuf,
    pub(crate) runtime: String,
    pub(crate) worker: String,
    pub(crate) token: String,
}
impl Access {
    pub(crate) fn validate(
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
/// Sent only in response to the negotiated v2 route operation. Retirement is
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
    pub(crate) runtime: String,
    pub(crate) worker: String,
    pub(crate) epoch: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeRequest {
    pub(crate) lease: ProviderLease,
    pub id: u64,
    pub revision: u64,
    pub format: String,
}
#[derive(Debug, Serialize, Deserialize)]
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
    pub(crate) route_revision: u64,
}
/// An acknowledged copy of one native source. Focus authorizes creation; later
/// reads remain valid while the source, export permission and owning integration
/// connection remain current. A copied identifier never renews this authority.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopyLease {
    pub(crate) id: String,
    pub offer: ExportOffer,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Chunk {
    #[serde(with = "serde_bytes")]
    pub bytes: Vec<u8>,
    pub eof: bool,
}
