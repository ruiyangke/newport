//! Native clipboard access for desktop integration. The read functions below
//! run on the host's main thread. Offers enumerate formats without rendering
//! data; payloads are read on demand and checked against the offered revision.
//! Lazy publication has a separate helper-process contract in [`provider`].
pub use newport_clipboard::{
    CONCEALED_FORMAT, CONCEALED_TYPE, FILES_FORMAT, LOCAL_PATHS_FORMAT, TRANSIENT_TYPE, owner,
    provider,
};
pub mod files;
#[cfg(target_os = "macos")]
pub use newport_clipboard::helper;
#[cfg(target_os = "macos")]
pub use newport_clipboard::macos::{offer, read_format, revision};
#[cfg(windows)]
pub use newport_clipboard::windows::{offer, read_format, revision};
