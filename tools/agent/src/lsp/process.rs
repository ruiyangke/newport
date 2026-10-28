//! Child processes the worker owns: each runs in its own process group so a
//! stop reaches everything it started, and its output is read off-thread.
use std::{
    io::{self, Read},
    process::{Child, ExitStatus},
    sync::mpsc,
    time::{Duration, Instant},
};

pub(super) fn signal_group(group: u32, signal: libc::c_int) {
    // A group whose members all exited reports ESRCH; there is nothing to stop.
    unsafe {
        libc::killpg(group as libc::pid_t, signal);
    }
}

/// Wait until the child exits or the deadline passes.
pub(super) fn wait_until(child: &mut Child, deadline: Instant) -> io::Result<Option<ExitStatus>> {
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Ask the whole group to stop, then force it.
pub(super) fn stop_group(child: &mut Child, grace: Duration) -> Option<ExitStatus> {
    let group = child.id();
    signal_group(group, libc::SIGTERM);
    let status = wait_until(child, Instant::now() + grace).ok().flatten();
    signal_group(group, libc::SIGKILL);
    status.or_else(|| child.wait().ok())
}

/// Bytes read from a pipe on another thread: the first `limit` bytes, or the
/// last `limit` bytes when `tail` is set.
pub(super) struct Collector(mpsc::Receiver<Vec<u8>>);
impl Collector {
    pub(super) fn start(mut input: impl Read + Send + 'static, limit: usize, tail: bool) -> Self {
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let mut kept = Vec::new();
            let mut buffer = [0; 8192];
            while let Ok(read) = input.read(&mut buffer) {
                if read == 0 {
                    break;
                }
                kept.extend_from_slice(&buffer[..read]);
                if kept.len() > limit {
                    if tail {
                        kept.drain(..kept.len() - limit);
                    } else {
                        kept.truncate(limit);
                    }
                }
            }
            let _ = sender.send(kept);
        });
        Self(receiver)
    }
    /// What was read, waiting at most `wait` for the writer to close.
    pub(super) fn finish(self, wait: Duration) -> Vec<u8> {
        self.0.recv_timeout(wait).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    #[test]
    fn stopping_a_group_reaches_the_processes_it_started() {
        let mut child = Command::new("/bin/sh")
            .args(["-c", "sleep 30 & echo $!; wait"])
            .stdout(Stdio::piped())
            .process_group(0)
            .spawn()
            .unwrap();
        let output = Collector::start(child.stdout.take().unwrap(), 64, false);
        std::thread::sleep(Duration::from_millis(200));
        stop_group(&mut child, Duration::from_millis(500));
        let sleeper: i32 = String::from_utf8(output.finish(Duration::from_secs(2)))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(unsafe { libc::kill(sleeper, 0) }, -1, "sleep survived");
    }

    #[test]
    fn collectors_keep_the_head_or_the_tail() {
        let head = Collector::start(&b"abcdef"[..], 3, false);
        let tail = Collector::start(&b"abcdef"[..], 3, true);
        assert_eq!(head.finish(Duration::from_secs(1)), b"abc");
        assert_eq!(tail.finish(Duration::from_secs(1)), b"def");
    }
}
