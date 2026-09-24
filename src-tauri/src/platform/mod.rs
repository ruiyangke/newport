//! Native mechanisms only. Connection, sync, and retry policy remain shared.
//!
//! Callers use `platform::{clipboard, secrets, events, ssh_agent, filesystem, desktop}`.
//! Platform implementations live under their OS folder with OS-prefixed filenames.
//! Add Windows exports here when implemented; unsupported platforms must not silently
//! use insecure storage, skip permissions, or pretend native observers are installed.
#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "macos")]
pub use mac::{clipboard, desktop, filesystem, secrets, ssh_agent};
#[cfg(not(target_os = "macos"))]
compile_error!("This desktop platform is not implemented yet");
pub mod events;
