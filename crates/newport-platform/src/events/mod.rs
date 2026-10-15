//! Process-lifetime OS observers. Install once on the application's main thread.
//! Notifications are recovery hints, never proof of remote connectivity.

#[derive(Clone, Copy, Debug)]
pub enum Event {
    Sleep,
    Wake,
    NetworkChanged,
}

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::install;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::install;

#[cfg(not(any(target_os = "macos", windows)))]
pub fn install(_: fn(Event)) -> anyhow::Result<()> {
    anyhow::bail!("System event observers are not implemented on this platform")
}
