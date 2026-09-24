#[derive(Clone, Copy, Debug)]
pub enum Event {
    Sleep,
    Wake,
    NetworkChanged,
}
#[cfg(target_os = "macos")]
pub use super::mac::events::install;
