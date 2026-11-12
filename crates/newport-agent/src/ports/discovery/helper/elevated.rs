//! The root helper a monitor session keeps: started once with `sudo -n`, it
//! answers one scan per request until this side closes its control pipe.
use super::{
    Error,
    client::{mark_inherited, sudo},
};
use newport_protocol::monitor::{
    self,
    ports::discovery::{
        Snapshot,
        helper::{Reply, Request, wire},
    },
};
use std::{
    io::{Read, Write},
    os::unix::process::CommandExt,
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{Arc, Mutex, PoisonError, mpsc},
    time::{Duration, Instant},
};

/// How often a waiting scan checks for cancellation.
const POLL: Duration = Duration::from_millis(20);
/// How long a stopped helper gets to exit before its status is read.
const EXIT_GRACE: Duration = Duration::from_millis(500);
/// sudo's own explanation is short; anything longer is cut.
const DIAGNOSTIC_LIMIT: usize = 4096;

pub struct Elevated {
    child: Option<Child>,
    input: Option<ChildStdin>,
    requests: mpsc::Sender<Request>,
    replies: mpsc::Receiver<Result<Reply, wire::Error>>,
    diagnostic: Arc<Mutex<String>>,
    /// Closes when the helper's stderr ends, so its text is complete.
    diagnostic_done: mpsc::Receiver<()>,
    answered: bool,
}

impl Elevated {
    /// Starts `agent listeners --serve` as root. sudo never prompts: without
    /// permission it exits, and the first scan reports [`Error::Refused`].
    pub fn start(agent: &Path) -> Result<Self, Error> {
        if !agent.is_absolute() {
            return Err(Error::Invalid(
                "Discovery helper requires an absolute agent path",
            ));
        }
        let mut command = Command::new(sudo()?);
        command
            .args(["-n", "--"])
            .arg(agent)
            .args(["listeners", "--serve"])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("LC_ALL", "C");
        Self::launch(command)
    }

    fn launch(mut command: Command) -> Result<Self, Error> {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        // SAFETY: marking descriptors close-on-exec uses only async-signal-safe
        // syscalls on the child's own descriptor table.
        unsafe {
            command.pre_exec(mark_inherited);
        }
        let mut child = command.spawn()?;
        let input = child.stdin.take();
        let mut output = child
            .stdout
            .take()
            .ok_or(Error::Invalid("Missing discovery output pipe"))?;
        let mut errors = child
            .stderr
            .take()
            .ok_or(Error::Invalid("Missing discovery diagnostic pipe"))?;
        let (requests, pending) = mpsc::channel::<Request>();
        let (replied, replies) = mpsc::channel();
        std::thread::Builder::new()
            .name("discovery-helper-replies".into())
            .spawn(move || {
                for request in pending {
                    let reply = wire::read_reply(&mut output, &request);
                    let ended = reply.is_err();
                    if replied.send(reply).is_err() || ended {
                        return;
                    }
                }
            })?;
        let diagnostic = Arc::new(Mutex::new(String::new()));
        let sink = Arc::clone(&diagnostic);
        let (done, diagnostic_done) = mpsc::channel();
        std::thread::Builder::new()
            .name("discovery-helper-errors".into())
            .spawn(move || {
                let _done = done;
                let mut buffer = [0; 512];
                while let Ok(read @ 1..) = errors.read(&mut buffer) {
                    let mut text = sink.lock().unwrap_or_else(PoisonError::into_inner);
                    if text.len() < DIAGNOSTIC_LIMIT {
                        text.push_str(&String::from_utf8_lossy(&buffer[..read]));
                    }
                }
            })?;
        Ok(Self {
            child: Some(child),
            input,
            requests,
            replies,
            diagnostic,
            diagnostic_done,
            answered: false,
        })
    }

    /// One scan as root. Any error leaves the helper unusable; drop it.
    pub fn scan(
        &mut self,
        generation: u64,
        deadline: Instant,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Snapshot, Error> {
        let request = Request {
            contract_id: Some(monitor::CONTRACT_ID.into()),
            generation,
            projects: None,
        };
        let frame = wire::encode_request(&request)?;
        if self.requests.send(request).is_err() {
            return Err(self.ended());
        }
        let Some(input) = self.input.as_mut() else {
            return Err(Error::Invalid("Discovery helper is closed"));
        };
        if input
            .write_all(&frame)
            .and_then(|()| input.flush())
            .is_err()
        {
            return Err(self.ended());
        }
        loop {
            if cancelled() {
                return Err(Error::Cancelled);
            }
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return Err(Error::TimedOut);
            };
            match self.replies.recv_timeout(left.min(POLL)) {
                Ok(Ok(reply)) => {
                    self.answered = true;
                    return reply
                        .result
                        .map(|snapshot| *snapshot)
                        .map_err(Error::Remote);
                }
                Ok(Err(_)) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(self.ended());
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }

    /// Whether the helper has answered a scan, so sudo let it run.
    pub fn answered(&self) -> bool {
        self.answered
    }

    /// Why the helper stopped: sudo's refusal, or its exit.
    fn ended(&mut self) -> Error {
        self.input.take();
        let started = Instant::now();
        let status = loop {
            let status = self
                .child
                .as_mut()
                .and_then(|child| child.try_wait().ok().flatten());
            if status.is_some() || started.elapsed() >= EXIT_GRACE {
                break status;
            }
            std::thread::sleep(POLL);
        };
        if status.is_some() {
            let _ = self.diagnostic_done.recv_timeout(EXIT_GRACE);
        }
        let diagnostic = self
            .diagnostic
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .trim()
            .to_owned();
        match status {
            Some(_) if !self.answered => Error::Refused { diagnostic },
            Some(status) => Error::Exited { status, diagnostic },
            None => Error::Invalid("Discovery helper stopped answering"),
        }
    }
}

impl Drop for Elevated {
    fn drop(&mut self) {
        // Closing the control pipe ends the helper: between scans at once,
        // during one when it next checks, and within its scan limit at worst.
        self.input.take();
        if let Some(mut child) = self.child.take()
            && child.try_wait().ok().flatten().is_none()
        {
            let _ = std::thread::Builder::new()
                .name("discovery-helper-reaper".into())
                .spawn(move || child.wait());
        }
    }
}

#[cfg(test)]
mod tests;
