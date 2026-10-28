//! Native clipboard access for desktop integration. The read functions below
//! run on the host's main thread. Offers enumerate formats without rendering
//! data; payloads are read on demand and checked against the offered revision.
//! Lazy publication has a separate helper-process contract in [`provider`].
/// Offered when a copy holds files; reading it transfers them. Served by the
/// backend, never by the native clipboard.
pub const FILES_FORMAT: &str = "application/x-newport-files";
/// Marks a secret copy. Remote password managers (KDE `x-kde-passwordManagerHint`
/// = `secret`) map to the pasteboard's `org.nspasteboard.ConcealedType`, and
/// back. Its data is always `secret`.
pub const CONCEALED_FORMAT: &str = "application/x-newport-concealed";
/// nspasteboard.org: clipboard managers do not record a concealed copy.
pub const CONCEALED_TYPE: &str = "org.nspasteboard.ConcealedType";
/// nspasteboard.org: a short-lived copy that managers need not record.
pub const TRANSIENT_TYPE: &str = "org.nspasteboard.TransientType";
/// The local paths of the files on the pasteboard. Never leaves this machine.
pub const LOCAL_PATHS_FORMAT: &str = "application/x-newport-local-paths";
#[cfg(unix)]
pub mod files;
#[cfg(target_os = "macos")]
pub mod helper;
#[cfg(any(windows, test))]
mod html;
#[cfg(target_os = "macos")]
mod macos;
pub mod provider;
#[cfg(target_os = "macos")]
pub use macos::{offer, read_format, revision};
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{offer, read_format, revision};
