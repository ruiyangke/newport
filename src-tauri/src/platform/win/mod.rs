#[path = "win_clipboard.rs"]
pub mod clipboard;
#[path = "win_desktop.rs"]
pub mod desktop;
#[path = "win_events.rs"]
pub mod events;
#[path = "win_filesystem.rs"]
pub mod filesystem;
#[path = "win_secrets.rs"]
pub mod secrets;
#[path = "win_ssh_agent.rs"]
pub mod ssh_agent;

pub(super) fn wide(value: &std::ffi::OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    value.encode_wide().chain(Some(0)).collect()
}
#[path = "win_activation.rs"]
pub mod activation;
