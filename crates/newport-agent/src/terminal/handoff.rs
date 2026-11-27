//! What a holder hands the build it replaces its own program with: the
//! shell's descriptors, kept open across `exec`, and its state, in a private
//! file. A build that can't read the file it is given, as another version's
//! may be, refuses it in its dry run, and the old build keeps the shell. The
//! holder's reaper and clipboard display follow it the same way.
use newport_protocol::terminal::Computer;
use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    os::fd::RawFd,
    path::{Path, PathBuf},
};

/// How a build's dry run says it can't take over, as opposed to failing for
/// now; the old build doesn't try it again.
pub(crate) const REFUSED: i32 = 78;

/// Marks an error as the dry run's refusal.
pub(crate) fn refusal(error: io::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Handoff {
    /// The folder the terminal's files are in.
    pub directory: PathBuf,
    /// The build the holder moves to.
    pub build: PathBuf,
    pub creator: Computer,
    pub started: u64,
    pub name: Option<String>,
    pub size: (u16, u16),
    pub shell: libc::pid_t,
    /// The PTY's controlling side.
    pub master: RawFd,
    /// The reaper's pipe, until it has brought the shell's wait status.
    pub status: Option<RawFd>,
    /// The socket clients attach to.
    pub listener: RawFd,
    /// The lock that makes it the terminal's one holder.
    pub lock: RawFd,
    /// The pipes that tell the reaper and the clipboard display to follow.
    pub reaper: Option<RawFd>,
    pub display: Option<RawFd>,
    pub reporter: Option<libc::pid_t>,
    pub program: Option<libc::pid_t>,
    pub prompted: bool,
    /// The computer that was active, made active again when it attaches.
    pub active: Option<String>,
}

/// The handoff as one JSON line, then the replica's full snapshot.
pub(super) fn write(path: &Path, handoff: &Handoff, snapshot: &[u8]) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(handoff)?;
    bytes.push(b'\n');
    bytes.extend_from_slice(snapshot);
    crate::paths::private_write(path, &bytes)
}

pub(super) fn read(path: &Path) -> io::Result<(Handoff, Vec<u8>)> {
    let mut bytes = fs::read(path)?;
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "Invalid terminal handoff.");
    let end = bytes
        .iter()
        .position(|byte| *byte == b'\n')
        .ok_or_else(invalid)?;
    let handoff = serde_json::from_slice(&bytes[..end]).map_err(|_| invalid())?;
    Ok((handoff, bytes.split_off(end + 1)))
}

/// A child of the holder told to follow it to a build: the holder tells it
/// again from time to time, in case it couldn't. It never moves to the build
/// it runs, nor tries again one whose dry run refused it.
#[derive(Default)]
pub(crate) struct Follow {
    declined: Option<Vec<u8>>,
}

impl Follow {
    /// Calls `move_to`, which returns only if this program keeps running,
    /// with whether the build refused.
    pub(crate) fn to(&mut self, build: &[u8], move_to: impl FnOnce(&[u8]) -> bool) {
        use std::os::unix::ffi::OsStrExt;
        let path = std::path::Path::new(std::ffi::OsStr::from_bytes(build));
        if crate::paths::is_own_program(path) || self.declined.as_deref() == Some(build) {
            return;
        }
        if move_to(build) {
            self.declined = Some(build.to_owned());
        }
    }
}

/// Whether a dry run's exit says the build refused.
pub(crate) fn refused(
    checked: &Result<std::process::ExitStatus, crate::detached::bounded::Error>,
) -> bool {
    checked
        .as_ref()
        .is_ok_and(|status| status.code() == Some(REFUSED))
}
