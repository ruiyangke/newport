//! Commands owned by a display launch, bounded by its admission deadline.
use std::{
    io::{self, Read},
    os::{
        fd::OwnedFd,
        unix::{net::UnixStream, process::CommandExt},
    },
    process::{Child, Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};

const OUTPUT_LIMIT: usize = 4096;
const POLL: Duration = Duration::from_millis(5);

/// A failed spawn cannot have launched anything; all later errors are uncertain.
#[derive(Debug)]
pub(crate) enum Error {
    Spawn(io::Error),
    Running(io::Error),
}
impl From<Error> for io::Error {
    fn from(error: Error) -> Self {
        match error {
            Error::Spawn(error) | Error::Running(error) => error,
        }
    }
}

pub(crate) fn remaining(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|left| !left.is_zero())
        .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "Display launch timed out"))
}

struct Running {
    child: Child,
    group: libc::pid_t,
    reaped: bool,
}
impl Drop for Running {
    fn drop(&mut self) {
        if !self.reaped {
            // SAFETY: this still-unreaped child leads the group created by spawn.
            unsafe {
                libc::killpg(self.group, libc::SIGKILL);
            }
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

pub(crate) fn status(command: &mut Command, deadline: Instant) -> Result<ExitStatus, Error> {
    run(command, deadline, false).map(|(status, _)| status)
}

pub(crate) fn output(
    command: &mut Command,
    deadline: Instant,
) -> Result<(ExitStatus, Vec<u8>), Error> {
    run(command, deadline, true)
}

fn run(
    command: &mut Command,
    deadline: Instant,
    capture: bool,
) -> Result<(ExitStatus, Vec<u8>), Error> {
    remaining(deadline).map_err(Error::Running)?;
    command
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0);
    let mut output = if capture {
        let (reader, writer) = UnixStream::pair().map_err(Error::Running)?;
        reader.set_nonblocking(true).map_err(Error::Running)?;
        let writer: OwnedFd = writer.into();
        command.stdout(Stdio::from(writer));
        Some(reader)
    } else {
        command.stdout(Stdio::null());
        None
    };
    let mut child = command.spawn().map_err(Error::Spawn)?;
    let group = match libc::pid_t::try_from(child.id()) {
        Ok(group) => group,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Running(io::Error::other(
                "Invalid launch process identifier",
            )));
        }
    };
    let mut running = Running {
        child,
        group,
        reaped: false,
    };
    let mut bytes = Vec::new();
    loop {
        remaining(deadline).map_err(Error::Running)?;
        drain(&mut output, &mut bytes, deadline).map_err(Error::Running)?;
        if let Some(status) = running.child.try_wait().map_err(Error::Running)? {
            // Mark before fallible output handling: the PID can now be reused.
            running.reaped = true;
            // No EOF wait: a descendant may retain stdout after the command exits.
            drain(&mut output, &mut bytes, deadline).map_err(Error::Running)?;
            return Ok((status, bytes));
        }
        std::thread::sleep(POLL.min(remaining(deadline).map_err(Error::Running)?));
    }
}

fn drain(
    output: &mut Option<UnixStream>,
    bytes: &mut Vec<u8>,
    deadline: Instant,
) -> io::Result<()> {
    let Some(output) = output else { return Ok(()) };
    let mut buffer = [0; 512];
    loop {
        remaining(deadline)?;
        match output.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(count) if count <= OUTPUT_LIMIT - bytes.len() => {
                bytes.extend_from_slice(&buffer[..count])
            }
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Launch command output exceeds its limit",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
}
