//! Native clipboard access and lazy publication. Sources and payloads are
//! portable; each desktop OS's owner runs its native calls in a supervised
//! helper process.
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
/// Pasteboard items one copy may hold.
pub const MAX_FILE_ITEMS: usize = 10_000;
#[cfg(target_os = "macos")]
pub mod helper;
#[cfg(any(windows, test))]
mod html;
#[cfg(target_os = "macos")]
pub mod macos;
pub mod owner;
pub mod provider;
#[cfg(windows)]
pub mod windows;
#[cfg(any(windows, test))]
mod windows_attachment;
#[cfg(any(windows, test))]
mod windows_bitmap;
#[cfg(any(windows, test))]
mod windows_commit;
#[cfg(any(windows, test))]
mod windows_formats;
#[cfg(any(windows, test))]
mod windows_limits;
