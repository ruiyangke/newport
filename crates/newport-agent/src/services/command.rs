use super::Output;
use std::{
    io::{self, Read},
    os::unix::process::CommandExt,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

const POLL: Duration = Duration::from_millis(5);

struct Running {
    child: Child,
    group: libc::pid_t,
    reaped: bool,
}

impl Drop for Running {
    fn drop(&mut self) {
        if self.reaped {
            return;
        }
        // SAFETY: killpg takes scalar arguments, and the unreaped child reserves
        // the group identity established by process_group.
        unsafe {
            libc::killpg(self.group, libc::SIGKILL);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub(super) fn run(
    command: &mut Command,
    deadline: Instant,
    limit: usize,
    cancelled: &dyn Fn() -> bool,
) -> io::Result<Output> {
    remaining(deadline, cancelled)?;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()?;
    let group = match libc::pid_t::try_from(child.id()) {
        Ok(group) => group,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::other("Invalid command process identifier"));
        }
    };
    let mut running = Running {
        child,
        group,
        reaped: false,
    };
    let mut stdout = running.child.stdout.take();
    let mut stderr = running.child.stderr.take();
    use std::os::fd::AsFd;
    for descriptor in [
        stdout.as_ref().map(AsFd::as_fd),
        stderr.as_ref().map(AsFd::as_fd),
    ] {
        crate::transport::set_nonblocking(
            descriptor.ok_or_else(|| io::Error::other("Missing command capture pipe"))?,
        )?;
    }
    let mut out = Vec::new();
    let mut err = Vec::new();
    loop {
        remaining(deadline, cancelled)?;
        drain(&mut stdout, &mut out, limit, deadline, cancelled)?;
        drain(&mut stderr, &mut err, limit, deadline, cancelled)?;
        if stdout.is_none()
            && stderr.is_none()
            && let Some(status) = running.child.try_wait()?
        {
            running.reaped = true;
            return Ok(Output {
                success: status.success(),
                stdout: String::from_utf8_lossy(&out).into_owned(),
                stderr: String::from_utf8_lossy(&err).into_owned(),
            });
        }
        std::thread::sleep(POLL.min(remaining(deadline, cancelled)?));
    }
}

fn remaining(deadline: Instant, cancelled: &dyn Fn() -> bool) -> io::Result<Duration> {
    if cancelled() {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Command cancelled",
        ));
    }
    deadline
        .checked_duration_since(Instant::now())
        .filter(|left| !left.is_zero())
        .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "Command time limit reached"))
}

fn drain(
    input: &mut Option<impl Read>,
    bytes: &mut Vec<u8>,
    limit: usize,
    deadline: Instant,
    cancelled: &dyn Fn() -> bool,
) -> io::Result<()> {
    let Some(reader) = input else {
        return Ok(());
    };
    let mut buffer = [0; 8192];
    loop {
        remaining(deadline, cancelled)?;
        match reader.read(&mut buffer) {
            Ok(0) => {
                *input = None;
                return Ok(());
            }
            Ok(count) if count <= limit.saturating_sub(bytes.len()) => {
                bytes.extend_from_slice(&buffer[..count])
            }
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::FileTooLarge,
                    "Command output exceeded its size limit",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell(script: &str) -> Command {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", script]);
        command
    }

    #[test]
    fn captures_both_streams_and_preserves_nonzero_exit() {
        let output = run(
            &mut shell("printf stdout; printf stderr >&2; exit 7"),
            Instant::now() + Duration::from_secs(1),
            100,
            &|| false,
        )
        .unwrap();
        assert!(!output.success);
        assert_eq!(output.stdout, "stdout");
        assert_eq!(output.stderr, "stderr");
    }

    #[test]
    fn output_overflow_is_an_error_instead_of_a_successful_truncated_reply() {
        let error = run(
            &mut shell("printf 123456789"),
            Instant::now() + Duration::from_secs(1),
            8,
            &|| false,
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::FileTooLarge);
    }

    #[test]
    fn cancellation_before_spawn_runs_nothing() {
        let error = run(
            &mut Command::new("/nonexistent/command"),
            Instant::now() + Duration::from_secs(1),
            100,
            &|| true,
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
    }

    #[test]
    fn inherited_capture_pipes_cannot_outlive_the_command_deadline() {
        let start = Instant::now();
        let error = run(
            &mut shell("sleep 30 & exit 0"),
            start + Duration::from_millis(100),
            100,
            &|| false,
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn cancellation_reaps_the_running_child() {
        let directory = tempfile::tempdir().unwrap();
        let pid_file = directory.path().join("pid");
        let mut command = shell("printf '%s' $$ > \"$1\"; exec sleep 30");
        command.arg("command-test").arg(&pid_file);
        let start = Instant::now();
        let error = run(&mut command, start + Duration::from_secs(2), 100, &|| {
            std::fs::read_to_string(&pid_file)
                .ok()
                .is_some_and(|text| text.parse::<libc::pid_t>().is_ok())
        })
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        let pid = std::fs::read_to_string(pid_file)
            .unwrap()
            .parse::<libc::pid_t>()
            .unwrap();
        // The direct child is reaped before returning, so it cannot remain a zombie.
        // SAFETY: signal 0 only checks that the process exists.
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        assert!(start.elapsed() < Duration::from_secs(1));
    }
}
