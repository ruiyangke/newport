use crate::{Abort, BoxFuture, Dialed, Dialer, Error, Target};
use std::{
    collections::VecDeque,
    future::Future,
    io,
    pin::Pin,
    process::{ExitStatus, Stdio},
    sync::{Arc, Mutex},
    task::{Context, Poll, ready},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf},
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command},
    sync::{Notify, watch},
    task::AbortHandle,
};

/// How much of the command's stderr a failure keeps. It keeps the end,
/// where the reason usually is.
const OUTPUT_KEPT: usize = 64 * 1024;

/// How long a failure waits for the command's last stderr and its exit
/// status after its stdout closes.
const DRAIN: Duration = Duration::from_secs(1);

/// Runs an `ssh_config` `ProxyCommand` through the user's shell, as ssh
/// does, and speaks SSH over its stdin and stdout. Each dial runs its own
/// command.
pub struct CommandDialer {
    command: String,
    original_host: Option<String>,
}

impl CommandDialer {
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            original_host: None,
        }
    }

    /// The host name as the person gave it, for `%n`, when the target's host
    /// was resolved from it, such as an `ssh_config` alias.
    pub fn original_host(mut self, name: impl Into<String>) -> Self {
        self.original_host = Some(name.into());
        self
    }

    /// The command line for `target`, with ssh's tokens expanded. An unknown
    /// token is an error, as it is for ssh.
    pub fn command_for(&self, target: &Target) -> Result<String, Error> {
        let mut line = String::with_capacity(self.command.len());
        let mut chars = self.command.chars();
        while let Some(c) = chars.next() {
            if c != '%' {
                line.push(c);
                continue;
            }
            match chars.next() {
                Some('%') => line.push('%'),
                Some('h') => line.push_str(&target.host),
                Some('p') => line.push_str(&target.port.to_string()),
                Some('r') => line.push_str(&target.user),
                Some('n') => line.push_str(self.original_host.as_ref().unwrap_or(&target.host)),
                Some(other) => {
                    return Err(Error::InvalidProxy(format!("unknown token %{other}")));
                }
                None => {
                    return Err(Error::InvalidProxy("a % with no token after it".into()));
                }
            }
        }
        Ok(line)
    }
}

impl Dialer for CommandDialer {
    fn dial<'a>(
        &'a self,
        target: &'a Target,
        _: &'a watch::Sender<bool>,
    ) -> BoxFuture<'a, Result<Dialed, Error>> {
        Box::pin(async move {
            let line = self.command_for(target)?;
            let mut child = shell(&line)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .map_err(|error| Error::InvalidProxy(format!("it couldn't start: {error}")))?;
            let stdin = child.stdin.take().expect("stdin is piped");
            let stdout = child.stdout.take().expect("stdout is piped");
            let stderr = child.stderr.take().expect("stderr is piped");
            let output = Arc::new(Mutex::new(Tail::default()));
            let (stderr_closed, stderr_done) = watch::channel(false);
            let collecting = tokio::spawn(collect(stderr, output.clone(), stderr_closed));
            let (exited, exit) = watch::channel(None);
            let kill = Arc::new(Notify::new());
            tokio::spawn(supervise(child, kill.clone(), exited));
            let stream = ProxyStream {
                stdin,
                stdout,
                read_any: false,
                ending: None,
                ended: false,
                output,
                stderr_done,
                exit,
                collecting: collecting.abort_handle(),
                kill: kill.clone(),
            };
            Ok(Dialed {
                stream: Box::new(stream),
                abort: Abort::new(move || kill.notify_one()),
            })
        })
    }
}

#[cfg(unix)]
fn shell(line: &str) -> Command {
    let shell = std::env::var_os("SHELL")
        .filter(|shell| !shell.is_empty())
        .unwrap_or_else(|| "/bin/sh".into());
    let mut command = Command::new(shell);
    // `exec`, as ssh does, so the child is the proxy itself and killing it
    // ends the proxy, not just a shell waiting for it.
    command.arg("-c").arg(format!("exec {line}"));
    command
}

#[cfg(windows)]
fn shell(line: &str) -> Command {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut command = Command::new("cmd");
    // `cmd` parses its own command line; quoting it as one argument would
    // change its meaning.
    command
        .arg("/C")
        .raw_arg(line)
        .creation_flags(CREATE_NO_WINDOW);
    command
}

/// Waits for the command to exit, or kills it when asked, and reaps it
/// either way.
async fn supervise(mut child: Child, kill: Arc<Notify>, exited: watch::Sender<Option<ExitStatus>>) {
    let status = tokio::select! {
        status = child.wait() => status,
        () = kill.notified() => {
            let _ = child.kill().await;
            child.wait().await
        }
    };
    exited.send_replace(status.ok());
}

async fn collect(mut stderr: ChildStderr, output: Arc<Mutex<Tail>>, done: watch::Sender<bool>) {
    let mut bytes = [0; 4096];
    while let Ok(n @ 1..) = stderr.read(&mut bytes).await {
        output.lock().unwrap().push(&bytes[..n]);
    }
    done.send_replace(true);
}

/// The last bytes the command wrote to stderr.
#[derive(Default)]
struct Tail(VecDeque<u8>);

impl Tail {
    fn push(&mut self, bytes: &[u8]) {
        self.0.extend(bytes);
        let excess = self.0.len().saturating_sub(OUTPUT_KEPT);
        self.0.drain(..excess);
    }

    fn text(&self) -> String {
        let (front, back) = self.0.as_slices();
        String::from_utf8_lossy(&[front, back].concat())
            .trim()
            .to_owned()
    }
}

/// The command's stdout and stdin as one stream. Dropping it kills the
/// command.
struct ProxyStream {
    stdin: ChildStdin,
    stdout: ChildStdout,
    read_any: bool,
    /// Decides how the stream ends once stdout has closed.
    ending: Option<Pin<Box<dyn Future<Output = Option<io::Error>> + Send>>>,
    ended: bool,
    output: Arc<Mutex<Tail>>,
    stderr_done: watch::Receiver<bool>,
    exit: watch::Receiver<Option<ExitStatus>>,
    collecting: AbortHandle,
    kill: Arc<Notify>,
}

/// Unless the command produced a stream and then exited cleanly, its stdout
/// closing is a failure, explained by what it wrote to stderr.
async fn ending(
    read_any: bool,
    output: Arc<Mutex<Tail>>,
    mut stderr_done: watch::Receiver<bool>,
    mut exit: watch::Receiver<Option<ExitStatus>>,
) -> Option<io::Error> {
    // Stderr and the exit status arrive on their own and may trail stdout.
    let _ = tokio::time::timeout(DRAIN, async {
        let _ = stderr_done.wait_for(|done| *done).await;
        let _ = exit.wait_for(Option::is_some).await;
    })
    .await;
    let status = *exit.borrow();
    if read_any && status.is_none_or(|status| status.success()) {
        return None;
    }
    let mut text = output.lock().unwrap().text();
    if text.is_empty() {
        text = match status {
            Some(status) => format!("it ended with {status}"),
            None => "it closed its output".into(),
        };
    }
    Some(io::Error::other(Error::ProxyFailed { output: text }))
}

impl AsyncRead for ProxyStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if let Some(ending) = &mut this.ending {
            let failure = ready!(ending.as_mut().poll(cx));
            this.ending = None;
            this.ended = true;
            return Poll::Ready(failure.map_or(Ok(()), Err));
        }
        if this.ended || buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        let before = buf.filled().len();
        ready!(Pin::new(&mut this.stdout).poll_read(cx, buf))?;
        if buf.filled().len() > before {
            this.read_any = true;
            return Poll::Ready(Ok(()));
        }
        this.ending = Some(Box::pin(ending(
            this.read_any,
            this.output.clone(),
            this.stderr_done.clone(),
            this.exit.clone(),
        )));
        Pin::new(this).poll_read(cx, buf)
    }
}

impl AsyncWrite for ProxyStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stdin).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stdin).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stdin).poll_shutdown(cx)
    }
}

impl Drop for ProxyStream {
    fn drop(&mut self) {
        self.kill.notify_one();
        self.collecting.abort();
    }
}
