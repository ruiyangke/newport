//! Processes that outlive the connection that starts them: terminal holders,
//! and the full agent's stable display.
use std::{
    ffi::OsStr,
    fmt, io,
    path::Path,
    process::{Command, ExitStatus, Stdio},
    time::Instant,
};

pub(crate) mod bounded;

/// A launcher completed without confirming that the display started.
#[derive(Debug)]
pub struct LaunchExit {
    unit: String,
    status: ExitStatus,
}
impl LaunchExit {
    /// The command's original exit code or terminating signal.
    pub fn status(&self) -> ExitStatus {
        self.status
    }

    fn error(unit: &str, status: ExitStatus) -> io::Error {
        io::Error::other(Self {
            unit: unit.to_owned(),
            status,
        })
    }
}
impl fmt::Display for LaunchExit {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.status.code().is_some() {
            write!(output, "{} did not start ({}).", self.unit, self.status)
        } else {
            write!(
                output,
                "{} launch ended without confirmation ({}).",
                self.unit, self.status
            )
        }
    }
}
impl std::error::Error for LaunchExit {}

/// A held terminal's scope. One of its processes killed for memory must not
/// stop the rest, as systemd's default for a scope does, and shells, which
/// ignore SIGTERM, are hung up when it stops.
pub const TERMINAL_SCOPE: &[&str] = &["OOMPolicy=continue", "SendSIGHUP=yes"];

/// Starts this build with `args`, outside the caller's process tree. Through
/// a systemd user scope named `unit` with `properties` when there is a user
/// manager, so logind's `KillUserProcesses` doesn't end it with the SSH
/// session, and with the account lingering while it runs (see
/// `crate::linger`), so it outlives the account's last logout; otherwise
/// directly. Either way the process must `detach` itself; this returns once
/// it has.
pub fn launch(unit: &str, properties: &[&str], args: &[String]) -> io::Result<()> {
    launch_with(
        &crate::linger::Linger::system(),
        OsStr::new("systemd-run"),
        unit,
        properties,
        args,
    )
}

/// Starts a display within its admission deadline, including lingering and
/// launcher completion. An uncertain launch is never retried directly.
pub fn launch_until(unit: &str, args: &[String], deadline: Instant) -> io::Result<()> {
    bounded::remaining(deadline)?;
    let executable = std::fs::read_link("/proc/self/exe").or_else(|_| std::env::current_exe())?;
    launch_until_with(
        &crate::linger::Linger::system(),
        OsStr::new("systemd-run"),
        &executable,
        unit,
        args,
        deadline,
    )
}

fn launch_until_with(
    linger: &crate::linger::Linger,
    systemd_run: &OsStr,
    executable: &Path,
    unit: &str,
    args: &[String],
    deadline: Instant,
) -> io::Result<()> {
    // Reconciliation must not turn lingering off before the new scope appears.
    let _held = linger.hold_for_until(unit, deadline)?;
    let scoped = bounded::status(
        Command::new(systemd_run)
            .args(["--user", "--scope", "--quiet", "--collect"])
            .arg(format!("--unit={unit}"))
            .arg("--")
            .arg(executable)
            .args(args),
        deadline,
    );
    match scoped {
        Ok(status) if status.success() => return Ok(()),
        Ok(status) if status.code().is_some() => {}
        Err(bounded::Error::Spawn(_)) => {}
        Err(error) => return Err(error.into()),
        Ok(status) => return Err(LaunchExit::error(unit, status)),
    }
    let status =
        bounded::status(Command::new(executable).args(args), deadline).map_err(io::Error::from)?;
    if status.success() {
        Ok(())
    } else {
        Err(LaunchExit::error(unit, status))
    }
}

fn launch_with(
    linger: &crate::linger::Linger,
    systemd_run: &OsStr,
    unit: &str,
    properties: &[&str],
    args: &[String],
) -> io::Result<()> {
    let executable = std::fs::read_link("/proc/self/exe").or_else(|_| std::env::current_exe())?;
    let quiet = |command: &mut Command| {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
    };
    // Held until the scope runs, so a reconcile can't turn lingering off
    // before the scope counts.
    let _held = linger.hold_for(unit);
    // Older systemd refuses OOMPolicy on a scope.
    let older: Vec<&str> = properties
        .iter()
        .copied()
        .filter(|property| !property.starts_with("OOMPolicy="))
        .collect();
    let mut attempts = vec![properties];
    if older.len() != properties.len() {
        attempts.push(&older);
    }
    for properties in attempts {
        let scoped = quiet(
            Command::new(systemd_run)
                .args(["--user", "--scope", "--quiet", "--collect"])
                .arg(format!("--unit={unit}"))
                .args(
                    properties
                        .iter()
                        .map(|property| format!("--property={property}")),
                )
                .arg("--")
                .arg(&executable)
                .args(args),
        );
        match scoped {
            Ok(status) if status.success() => return Ok(()),
            Ok(_) => {}
            // No systemd-run at all.
            Err(_) => break,
        }
    }
    let status = quiet(Command::new(&executable).args(args))?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("{unit} did not start.")))
    }
}

/// Double fork with `setsid`: not the starter's child, not in its process
/// group, not in the SSH session, and with no controlling terminal. Call it
/// before starting any thread; the starter's `launch` returns once it has.
pub fn detach() -> io::Result<()> {
    // SAFETY: no thread has started, so the forks leave no other thread's state
    // behind; the calls take scalar arguments or a static C string.
    unsafe {
        match libc::fork() {
            -1 => return Err(io::Error::last_os_error()),
            0 => {}
            _ => libc::_exit(0),
        }
        libc::setsid();
        match libc::fork() {
            -1 => libc::_exit(1),
            0 => {}
            _ => libc::_exit(0),
        }
        libc::umask(0o077);
        let null = libc::open(c"/dev/null".as_ptr(), libc::O_RDWR);
        for fd in 0..3 {
            libc::dup2(null, fd);
        }
        if null > 2 {
            libc::close(null);
        }
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
    }
    if let Some(home) = std::env::var_os("HOME") {
        let _ = std::env::set_current_dir(home);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
