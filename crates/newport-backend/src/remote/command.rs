//! Commands on the server, each on a session channel of its own, run under
//! POSIX sh whatever the account's login shell is.
use super::{lost, unsent};
use crate::ssh::posix;
use newport_ssh::{Channel, Link, Open};
use russh::ChannelMsg;
use std::{fmt, future::Future, time::Duration};
use tokio::io::{AsyncWrite, AsyncWriteExt};

/// The most a command may write to each of stdout and stderr.
pub const MAX_OUTPUT: usize = 4 * 1024 * 1024;
/// How long a command may run once its channel is open, or a stream may
/// take to start.
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(25);
/// How long an upload may wait for the server to take one chunk.
pub const UPLOAD_STALL: Duration = Duration::from_secs(45);
/// How long a whole upload may take; a slow but moving one outlives
/// [`COMMAND_TIMEOUT`].
pub const UPLOAD_LIMIT: Duration = Duration::from_secs(15 * 60);
/// How long the command may take to finish once its upload is sent.
pub const UPLOAD_FINISH: Duration = Duration::from_secs(60);

/// What a command wrote, and how it exited.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Output {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: u32,
}

impl Output {
    /// Stdout of a command that exited 0; otherwise its failure, with
    /// stderr before stdout.
    pub fn into_stdout(self) -> Result<String, Error> {
        if self.exit_code == 0 {
            return Ok(self.stdout);
        }
        let details = [self.stderr.trim(), self.stdout.trim()]
            .into_iter()
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        Err(Error::Failed {
            status: self.exit_code,
            details,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// No channel could be opened, or its connection was lost.
    Link(newport_ssh::Error),
    /// The server refused to run the command.
    Refused,
    /// The command didn't finish, or a stream didn't start, in time.
    TimedOut,
    StdoutTooLarge,
    StderrTooLarge,
    /// A signal ended the command.
    Signal(String),
    /// The channel closed without the command's exit status.
    NoExitStatus,
    /// The command exited with a status other than 0.
    Failed {
        status: u32,
        details: String,
    },
    /// The server took no upload data for [`UPLOAD_STALL`].
    UploadStalled,
    /// The upload took longer than [`UPLOAD_LIMIT`].
    UploadTimedOut,
    /// The upload's source failed.
    Source(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mib = MAX_OUTPUT / (1024 * 1024);
        match self {
            Error::Link(error) => error.fmt(f),
            Error::Refused => f.write_str("The server refused to run the command."),
            Error::TimedOut => f.write_str("The remote command timed out."),
            Error::StdoutTooLarge => write!(f, "Remote output exceeded {mib} MiB"),
            Error::StderrTooLarge => write!(f, "Remote error output exceeded {mib} MiB"),
            Error::Signal(signal) => write!(f, "Remote command terminated by signal {signal}"),
            Error::NoExitStatus => f.write_str("Remote command ended without an exit status"),
            Error::Failed { status, details } => {
                write!(f, "Remote command failed (status {status}): {details}")
            }
            Error::UploadStalled => write!(
                f,
                "The upload stalled for {} seconds.",
                UPLOAD_STALL.as_secs()
            ),
            Error::UploadTimedOut => write!(
                f,
                "The upload took longer than {} minutes.",
                UPLOAD_LIMIT.as_secs() / 60
            ),
            Error::Source(error) => f.write_str(error),
        }
    }
}

impl std::error::Error for Error {}

impl From<Error> for String {
    fn from(error: Error) -> Self {
        error.to_string()
    }
}

impl From<newport_ssh::Error> for Error {
    fn from(error: newport_ssh::Error) -> Self {
        Error::Link(error)
    }
}

/// Runs `command` with `input` as its stdin, and collects its output.
pub async fn run(link: &Link, command: &str, input: Option<&[u8]>) -> Result<Output, Error> {
    run_within(link, command, input, COMMAND_TIMEOUT).await
}

/// Runs `command` and returns its stdout, or its failure when it exits with
/// a status other than 0.
pub async fn stdout(link: &Link, command: &str, input: Option<&[u8]>) -> Result<String, Error> {
    run(link, command, input).await?.into_stdout()
}

async fn run_within(
    link: &Link,
    command: &str,
    input: Option<&[u8]>,
    limit: Duration,
) -> Result<Output, Error> {
    // The deadline starts once the channel is open: opening may wait for a
    // login, and a login may wait for the person to approve it.
    let mut channel = open(link).await?;
    tokio::time::timeout(limit, async {
        start(&channel, command).await?;
        let mut writer = channel.make_writer();
        let send = async move {
            if let Some(input) = input {
                writer.write_all(input).await.map_err(unsent)?;
            }
            writer.shutdown().await.map_err(unsent)
        };
        let send = async { send.await.map_err(Error::Link) };
        let (_, output) = tokio::try_join!(send, collect(&mut channel))?;
        Ok(output)
    })
    .await
    .map_err(|_| Error::TimedOut)?
}

/// Starts a long-running command, such as Needle's runtime, and hands over
/// its channel. Only starting has a deadline.
pub async fn stream(link: &Link, command: &str) -> Result<Channel, Error> {
    let channel = open(link).await?;
    tokio::time::timeout(COMMAND_TIMEOUT, start(&channel, command))
        .await
        .map_err(|_| Error::TimedOut)??;
    Ok(channel)
}

/// Bytes to upload, chunk by chunk.
pub trait Source: Send {
    /// The next chunk, or `None` once everything is sent and checked.
    fn next(&mut self) -> impl Future<Output = Result<Option<Vec<u8>>, String>> + Send;
}

impl Source for crate::agent_payload::Upload {
    fn next(&mut self) -> impl Future<Output = Result<Option<Vec<u8>>, String>> + Send {
        crate::agent_payload::Upload::next(self)
    }
}

/// Runs `command` with `source` as its stdin, reporting the bytes sent, and
/// returns its stdout. A moving upload may take up to [`UPLOAD_LIMIT`]; a
/// stalled one ends after [`UPLOAD_STALL`].
pub async fn upload(
    link: &Link,
    command: &str,
    source: impl Source,
    progress: &(dyn Fn(usize) + Send + Sync),
) -> Result<String, Error> {
    let mut channel = open(link).await?;
    tokio::time::timeout(COMMAND_TIMEOUT, start(&channel, command))
        .await
        .map_err(|_| Error::TimedOut)??;
    let writer = channel.make_writer();
    deliver(source, writer, collect(&mut channel), progress).await
}

/// Sends `source` to a started command while `output` collects what the
/// command writes, and returns its stdout. When the command ends first and
/// fails, its failure is the upload's, not the send it cut short.
async fn deliver(
    source: impl Source,
    mut writer: impl AsyncWrite + Unpin,
    output: impl Future<Output = Result<Output, Error>>,
    progress: &(dyn Fn(usize) + Send + Sync),
) -> Result<String, Error> {
    tokio::time::timeout(UPLOAD_LIMIT, async {
        let send = send_all(source, &mut writer, progress);
        tokio::pin!(send, output);
        let finished = async |output| {
            tokio::time::timeout(UPLOAD_FINISH, output)
                .await
                .map_err(|_| Error::TimedOut)?
        };
        tokio::select! {
            sent = &mut send => match sent {
                Ok(()) => finished(output).await?.into_stdout(),
                Err(Error::Link(unsent)) => match finished(output).await {
                    Ok(output) if output.exit_code != 0 => output.into_stdout(),
                    _ => Err(Error::Link(unsent)),
                },
                Err(error) => Err(error),
            },
            ended = &mut output => {
                let ended = ended?;
                if ended.exit_code == 0 {
                    send.await?;
                }
                ended.into_stdout()
            }
        }
    })
    .await
    .map_err(|_| Error::UploadTimedOut)?
}

/// Writes every chunk of `source`, then EOF, each within [`UPLOAD_STALL`].
async fn send_all(
    mut source: impl Source,
    writer: &mut (impl AsyncWrite + Unpin),
    progress: &(dyn Fn(usize) + Send + Sync),
) -> Result<(), Error> {
    let mut sent = 0;
    while let Some(chunk) = source.next().await.map_err(Error::Source)? {
        tokio::time::timeout(UPLOAD_STALL, writer.write_all(&chunk))
            .await
            .map_err(|_| Error::UploadStalled)?
            .map_err(|error| Error::Link(unsent(error)))?;
        sent += chunk.len();
        progress(sent);
    }
    tokio::time::timeout(UPLOAD_STALL, writer.shutdown())
        .await
        .map_err(|_| Error::UploadStalled)?
        .map_err(|error| Error::Link(unsent(error)))
}

async fn open(link: &Link) -> Result<Channel, Error> {
    link.open(Open::Session).await.map_err(Error::Link)
}

async fn start(channel: &Channel, command: &str) -> Result<(), Error> {
    channel
        .exec(true, posix(command))
        .await
        .map_err(|error| Error::Link(unsent(error)))
}

/// Reads the command's output and exit status until its channel closes.
async fn collect(channel: &mut Channel) -> Result<Output, Error> {
    let (mut stdout, mut stderr, mut status) = (Vec::new(), Vec::new(), None);
    while let Some(message) = channel.wait().await {
        match message {
            ChannelMsg::Data { data } => {
                if stdout.len() + data.len() > MAX_OUTPUT {
                    return Err(Error::StdoutTooLarge);
                }
                stdout.extend_from_slice(&data);
            }
            ChannelMsg::ExtendedData { data, .. } => {
                if stderr.len() + data.len() > MAX_OUTPUT {
                    return Err(Error::StderrTooLarge);
                }
                stderr.extend_from_slice(&data);
            }
            ChannelMsg::ExitStatus { exit_status } => status = Some(exit_status),
            ChannelMsg::ExitSignal { signal_name, .. } => {
                return Err(Error::Signal(format!("{signal_name:?}")));
            }
            ChannelMsg::Failure => return Err(Error::Refused),
            _ => {}
        }
    }
    let Some(exit_code) = status else {
        return Err(lost(&channel.fate())
            .await
            .map_or(Error::NoExitStatus, Error::Link));
    };
    Ok(Output {
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
        exit_code,
    })
}

#[cfg(test)]
pub(crate) async fn run_with_limit(
    link: &Link,
    command: &str,
    limit: Duration,
) -> Result<Output, Error> {
    run_within(link, command, None, limit).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tokio::io::AsyncReadExt;

    struct Chunks(std::vec::IntoIter<Result<Vec<u8>, String>>);
    impl Source for Chunks {
        fn next(&mut self) -> impl Future<Output = Result<Option<Vec<u8>>, String>> + Send {
            std::future::ready(self.0.next().transpose())
        }
    }
    fn chunks(chunks: Vec<Result<Vec<u8>, String>>) -> Chunks {
        Chunks(chunks.into_iter())
    }

    #[test]
    fn a_failure_shows_stderr_then_stdout_with_its_status() {
        let output = Output {
            stdout: "partial output\n".into(),
            stderr: "denied\n".into(),
            exit_code: 1,
        };
        let error = output.into_stdout().unwrap_err();
        assert_eq!(
            error,
            Error::Failed {
                status: 1,
                details: "denied\npartial output".into()
            }
        );
        assert_eq!(
            error.to_string(),
            "Remote command failed (status 1): denied\npartial output"
        );
    }

    #[test]
    fn success_keeps_stdout_as_written_and_ignores_stderr() {
        let output = Output {
            stdout: "  output\n".into(),
            stderr: "warning".into(),
            exit_code: 0,
        };
        assert_eq!(output.into_stdout().unwrap(), "  output\n");
    }

    #[tokio::test(start_paused = true)]
    async fn an_upload_reports_progress_and_ends_with_eof() {
        let (mut writer, mut reader) = tokio::io::duplex(1024);
        let seen = Mutex::new(Vec::new());
        let read = tokio::spawn(async move {
            let mut bytes = Vec::new();
            reader.read_to_end(&mut bytes).await.unwrap();
            bytes
        });
        send_all(
            chunks(vec![Ok(vec![1; 3000]), Ok(vec![2; 10])]),
            &mut writer,
            &|sent| seen.lock().unwrap().push(sent),
        )
        .await
        .unwrap();
        drop(writer);
        assert_eq!(read.await.unwrap().len(), 3010);
        assert_eq!(*seen.lock().unwrap(), vec![3000, 3010]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_slow_send_continues_but_a_stalled_one_ends() {
        let (mut writer, mut reader) = tokio::io::duplex(1024);
        let drain = tokio::spawn(async move {
            let mut bytes = [0; 1024];
            // Slower than the command deadline overall, never stalled.
            for _ in 0..3 {
                tokio::time::sleep(Duration::from_secs(20)).await;
                let _ = reader.read(&mut bytes).await;
            }
            // Then never read again.
            std::future::pending::<()>().await;
        });
        let started = tokio::time::Instant::now();
        let error = send_all(
            chunks(vec![
                Ok(vec![0; 1024]),
                Ok(vec![0; 1024]),
                Ok(vec![0; 1024]),
                Ok(vec![0; 1024]),
                Ok(vec![0; 1024]),
            ]),
            &mut writer,
            &|_| {},
        )
        .await
        .unwrap_err();
        assert_eq!(error, Error::UploadStalled);
        assert_eq!(started.elapsed(), Duration::from_secs(60) + UPLOAD_STALL);
        drain.abort();
    }

    /// Chunks for as long as anyone reads them.
    struct Endless;
    impl Source for Endless {
        fn next(&mut self) -> impl Future<Output = Result<Option<Vec<u8>>, String>> + Send {
            std::future::ready(Ok(Some(vec![0; 1024])))
        }
    }

    fn exited(exit_code: u32, stderr: &str) -> Output {
        Output {
            stdout: String::new(),
            stderr: stderr.into(),
            exit_code,
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_moving_upload_ends_at_its_limit() {
        let (writer, mut reader) = tokio::io::duplex(1024);
        let drain = tokio::spawn(async move {
            let mut bytes = [0; 1024];
            // Never stalled, never done.
            loop {
                tokio::time::sleep(UPLOAD_STALL / 2).await;
                let _ = reader.read(&mut bytes).await;
            }
        });
        let started = tokio::time::Instant::now();
        let error = deliver(Endless, writer, std::future::pending(), &|_| {})
            .await
            .unwrap_err();
        assert_eq!(error, Error::UploadTimedOut);
        assert_eq!(started.elapsed(), UPLOAD_LIMIT);
        drain.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn a_sent_upload_waits_only_so_long_for_its_command() {
        let started = tokio::time::Instant::now();
        let error = deliver(
            chunks(vec![Ok(vec![0; 10])]),
            tokio::io::sink(),
            std::future::pending(),
            &|_| {},
        )
        .await
        .unwrap_err();
        assert_eq!(error, Error::TimedOut);
        assert_eq!(started.elapsed(), UPLOAD_FINISH);
    }

    #[tokio::test(start_paused = true)]
    async fn an_upload_the_command_ended_reports_how_it_ended() {
        // The command fails and its channel closes, so the send fails first.
        let (writer, reader) = tokio::io::duplex(16);
        drop(reader);
        let output = async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            Ok(exited(1, "No space left on device\n"))
        };
        let error = deliver(chunks(vec![Ok(vec![0; 64])]), writer, output, &|_| {})
            .await
            .unwrap_err();
        assert_eq!(
            error,
            Error::Failed {
                status: 1,
                details: "No space left on device".into()
            }
        );
        // The command fails before anything is sent.
        let (writer, _reader) = tokio::io::duplex(16);
        let error = deliver(
            Endless,
            writer,
            std::future::ready(Ok(exited(2, "denied"))),
            &|_| {},
        )
        .await
        .unwrap_err();
        assert_eq!(
            error,
            Error::Failed {
                status: 2,
                details: "denied".into()
            }
        );
    }

    #[tokio::test]
    async fn an_upload_returns_the_stdout_of_its_command() {
        let (writer, mut reader) = tokio::io::duplex(1024);
        let read = tokio::spawn(async move {
            let mut bytes = Vec::new();
            reader.read_to_end(&mut bytes).await.unwrap();
            bytes.len()
        });
        let output = async {
            let length = read.await.unwrap();
            Ok(Output {
                stdout: format!("{length}"),
                stderr: String::new(),
                exit_code: 0,
            })
        };
        let stdout = deliver(chunks(vec![Ok(vec![1; 30])]), writer, output, &|_| {})
            .await
            .unwrap();
        assert_eq!(stdout, "30");
    }

    #[tokio::test]
    async fn a_failed_source_ends_the_upload_with_its_error() {
        let (mut writer, _reader) = tokio::io::duplex(1024);
        let error = send_all(
            chunks(vec![Ok(vec![0; 10]), Err("checksum mismatch".into())]),
            &mut writer,
            &|_| {},
        )
        .await
        .unwrap_err();
        assert_eq!(error, Error::Source("checksum mismatch".into()));
    }
}
