//! Win32 support for Newport's supervised child processes and private pipes:
//! owned handles, wide strings and command lines, job objects, process
//! creation with an exact inherited-handle list, and security descriptors.
//! Everything but command-line quoting exists only on Windows.
pub mod command_line;
#[cfg(windows)]
pub mod handle;
#[cfg(windows)]
pub mod job;
#[cfg(windows)]
pub mod pipe;
#[cfg(windows)]
pub mod process;
#[cfg(windows)]
pub mod security;

#[cfg(windows)]
pub use command_line::{command_line, wide};
