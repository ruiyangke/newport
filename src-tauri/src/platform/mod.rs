//! Native mechanisms only. Connection, sync, and retry policy remain shared.
//!
//! Callers use `platform::{clipboard, secrets, events, ssh_agent, filesystem, desktop}`.
//! Platform implementations live under their OS folder with OS-prefixed filenames.
//! Unsupported platforms must not silently
//! use insecure storage, skip permissions, or pretend native observers are installed.
#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "macos")]
pub use mac::{clipboard, desktop, filesystem, secrets, ssh_agent};
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
compile_error!("This desktop platform is not implemented yet");
pub mod events;

#[cfg(target_os = "windows")]
mod win;
#[cfg(target_os = "windows")]
pub use win::activation;
#[cfg(target_os = "windows")]
pub use win::{clipboard, desktop, filesystem, secrets, ssh_agent};
