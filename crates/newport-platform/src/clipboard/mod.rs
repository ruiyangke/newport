//! Native clipboard reads for desktop integration. Call from the host's main
//! thread. Offers enumerate formats without rendering data; payloads are read
//! on demand and checked against the offered revision before publication.
#[cfg(any(windows, test))]
mod html;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::{offer, read_format, revision};
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{offer, read_format, revision};
