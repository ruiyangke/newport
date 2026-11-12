use super::{LIFETIME, io_bound::Nonblocking};
use newport_protocol::{
    common,
    monitor::{
        self,
        ports::discovery::{
            Snapshot,
            helper::{Request, wire},
        },
    },
};
use std::{
    fmt,
    fs::File,
    io,
    os::{fd::OwnedFd, unix::process::CommandExt},
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};

mod inherited;
mod pump;

const REAP_TIME: Duration = Duration::from_millis(500);
/// Where sudo is installed: the usual place, then NixOS's setuid wrapper.
/// Never looked up on `PATH`.
const SUDO: [&str; 2] = ["/usr/bin/sudo", "/run/wrappers/bin/sudo"];
const POLL: Duration = Duration::from_millis(10);

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Wire(wire::Error),
    Remote(common::Error),
    Exited {
        status: ExitStatus,
        diagnostic: String,
    },
    /// sudo declined to run the helper, which never answered.
    Refused {
        diagnostic: String,
    },
    Invalid(&'static str),
    Cancelled,
    TimedOut,
    CleanupPending {
        cause: Option<Box<Error>>,
    },
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(f),
            Self::Wire(error) => error.fmt(f),
            Self::Remote(error) => error.fmt(f),
            Self::Exited { status, diagnostic } => {
                write!(f, "Discovery helper exited ({status}): {diagnostic}")
            }
            Self::Refused { diagnostic } => {
                write!(f, "sudo refused the discovery helper: {diagnostic}")
            }
            Self::Invalid(message) => f.write_str(message),
            Self::Cancelled => f.write_str("Discovery helper cancelled"),
            Self::TimedOut => f.write_str("Discovery helper time limit reached"),
            Self::CleanupPending { cause } => {
                f.write_str("Discovery helper has not finished shutting down")?;
                if let Some(cause) = cause {
                    write!(f, ": {cause}")?;
                }
                Ok(())
            }
        }
    }
}
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Wire(error) => Some(error),
            Self::Remote(error) => Some(error),
            Self::CleanupPending { cause: Some(error) } => Some(error.as_ref()),
            _ => None,
        }
    }
}
impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
impl From<wire::Error> for Error {
    fn from(error: wire::Error) -> Self {
        Self::Wire(error)
    }
}

struct Running {
    child: Child,
    control: Option<File>,
    reaped: bool,
}
impl Running {
    fn signal(&mut self, signal: libc::c_int) {
        if self.reaped {
            return;
        }
        if let Ok(group) = libc::pid_t::try_from(self.child.id()) {
            // SAFETY: the unreaped child reserves the process group created at spawn.
            unsafe {
                libc::killpg(group, signal);
            }
        }
    }

    fn poll(&mut self) -> io::Result<Option<ExitStatus>> {
        let status = self.child.try_wait()?;
        self.reaped |= status.is_some();
        Ok(status)
    }

    fn stop(&mut self) {
        self.control.take();
        if self.reaped {
            return;
        }
        self.signal(libc::SIGKILL);
        let started = Instant::now();
        while started.elapsed() < REAP_TIME {
            if self.poll().ok().flatten().is_some() {
                return;
            }
            std::thread::sleep(POLL);
        }
    }
}
impl Drop for Running {
    fn drop(&mut self) {
        self.control.take();
        self.signal(libc::SIGKILL);
        let _ = self.poll();
    }
}

/// Owns at most one child, including a child still shutting down after cancellation.
#[derive(Default)]
pub struct Client {
    running: Option<Running>,
}

impl Client {
    pub fn poll_cleanup(&mut self) {
        if self
            .running
            .as_mut()
            .is_some_and(|running| running.poll().ok().flatten().is_some())
        {
            self.running = None;
        }
    }
    pub fn projects(
        &mut self,
        agent: &Path,
        snapshot: &Snapshot,
        deadline: Instant,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Snapshot, Error> {
        if !agent.is_absolute() {
            return Err(Error::Invalid(
                "Project helper requires an absolute agent path",
            ));
        }
        let mut command = Command::new(agent);
        command
            .args(["projects", "--stdio"])
            .current_dir("/")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("LC_ALL", "C");
        self.exchange(
            &mut command,
            Request {
                contract_id: Some(monitor::CONTRACT_ID.into()),
                generation: snapshot.generation,
                projects: Some(Box::new(snapshot.clone())),
            },
            deadline,
            cancelled,
        )
    }

    fn exchange(
        &mut self,
        command: &mut Command,
        request: Request,
        deadline: Instant,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Snapshot, Error> {
        check(deadline, cancelled)?;
        if let Some(running) = &mut self.running {
            running.poll()?;
            if !running.reaped {
                return Err(Error::CleanupPending { cause: None });
            }
        }
        self.running = None;
        let frame = wire::encode_request(&request)?;
        let deadline = deadline.min(Instant::now() + LIFETIME);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        // SAFETY: descriptor enumeration uses stack storage and async-signal-safe syscalls.
        // CLOEXEC preserves std's exec-error pipe until exec, while preventing inherited handles.
        unsafe {
            command.pre_exec(inherited::mark);
        }
        self.running = Some(Running {
            child: command.spawn()?,
            control: None,
            reaped: false,
        });
        let running = self
            .running
            .as_mut()
            .ok_or(Error::Invalid("Missing discovery helper process"))?;
        let result = capture(running, &request, &frame, deadline, cancelled);
        running.stop();
        if !running.reaped {
            return Err(Error::CleanupPending {
                cause: result.err().map(Box::new),
            });
        }
        self.running = None;
        result
    }
}

fn capture(
    running: &mut Running,
    request: &Request,
    frame: &[u8],
    deadline: Instant,
    cancelled: &dyn Fn() -> bool,
) -> Result<Snapshot, Error> {
    let input = running
        .child
        .stdin
        .take()
        .ok_or(Error::Invalid("Missing discovery control pipe"))?;
    running.control = Some(File::from(OwnedFd::from(input)));
    let output = File::from(OwnedFd::from(
        running
            .child
            .stdout
            .take()
            .ok_or(Error::Invalid("Missing discovery output pipe"))?,
    ));
    let errors = File::from(OwnedFd::from(
        running
            .child
            .stderr
            .take()
            .ok_or(Error::Invalid("Missing discovery diagnostic pipe"))?,
    ));
    let input = running
        .control
        .as_ref()
        .ok_or(Error::Invalid("Missing discovery control pipe"))?;
    use std::os::fd::AsFd;
    let _input_flags = Nonblocking::pipe(input.as_fd())?;
    let _output_flags = Nonblocking::pipe(output.as_fd())?;
    let _error_flags = Nonblocking::pipe(errors.as_fd())?;
    pump::run(
        &mut running.child,
        &mut running.reaped,
        pump::Pipes {
            input,
            output: &output,
            errors: &errors,
        },
        request,
        frame,
        deadline,
        cancelled,
    )
}

fn check(deadline: Instant, cancelled: &dyn Fn() -> bool) -> Result<(), Error> {
    if cancelled() {
        return Err(Error::Cancelled);
    }
    if Instant::now() >= deadline {
        return Err(Error::TimedOut);
    }
    Ok(())
}

/// The first of `candidates` that is a file.
/// sudo, from the places it is installed.
pub(super) fn sudo() -> Result<&'static Path, Error> {
    installed(&SUDO.map(Path::new)).ok_or_else(|| Error::Io(io::ErrorKind::NotFound.into()))
}

/// Marks this child's inherited descriptors close-on-exec; for `pre_exec`.
pub(super) fn mark_inherited() -> io::Result<()> {
    inherited::mark()
}

fn installed<'a>(candidates: &[&'a Path]) -> Option<&'a Path> {
    candidates.iter().copied().find(|path| path.is_file())
}

#[cfg(test)]
mod tests;
