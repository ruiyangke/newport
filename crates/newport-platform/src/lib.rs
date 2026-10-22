//! Operating-system adapters shared by the UI and the backend.
#[cfg(feature = "account")]
pub mod account;
pub mod clipboard;
pub mod events;
pub mod filesystem;
pub mod ipc;
#[cfg(feature = "pdf")]
pub mod pdf;
pub mod secrets;
#[cfg(windows)]
mod security;
pub mod ssh_agent;
pub mod startup;
#[cfg(all(feature = "tray", any(target_os = "macos", windows)))]
pub mod tray;
#[cfg(feature = "updates")]
pub mod updates;
#[cfg(windows)]
pub mod window;
