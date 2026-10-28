//! Bounded blocking IO for a client attached to one backend-owned SSH channel.
//! No private GUI dependencies or GUI protocol interpretation belong here.
use async_channel::{Receiver, Sender};
use russh::ChannelMsg;
use std::{
    io::{self, Read, Write},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

const CHUNK: usize = 32 * 1024;
const INPUT_CHUNKS: usize = 8;
const OUTPUT_CHUNKS: usize = 128;
const STDERR_LIMIT: usize = 64 * 1024;
const OUTPUT_STALL: Duration = Duration::from_secs(2);
const INPUT_CLOSED_GRACE: Duration = Duration::from_secs(5);
const CLOSE_LIMIT: Duration = Duration::from_secs(1);
const FINISH_LIMIT: Duration = Duration::from_secs(3);

/// Final SSH outcome. Diagnostic text is untrusted and bounded to its tail, `STDERR_LIMIT` bytes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub status: Option<u32>,
    pub signal: Option<String>,
    pub stderr: Vec<u8>,
    pub error: Option<String>,
}
impl Report {
    pub fn failure(&self) -> Option<String> {
        self.error
            .clone()
            .or_else(|| {
                self.signal
                    .as_ref()
                    .map(|signal| format!("Graphical worker exited on {signal}"))
            })
            .or_else(|| {
                self.status
                    .filter(|status| *status != 0)
                    .map(|status| format!("Graphical worker exited with status {status}"))
            })
    }
}
enum Input {
    Data(Vec<u8>),
    Flushed(Sender<()>),
}
struct State {
    input: Sender<Input>,
    pending: Receiver<Input>,
    output: Receiver<Vec<u8>>,
    cancel: Sender<()>,
    cancelled: AtomicBool,
    report: Mutex<Option<Report>>,
    completed: Condvar,
}
/// Cloning this handle does not keep either IO half open. cancel only fences
/// this channel; it never disconnects the SSH connection shared by other services.
#[derive(Clone)]
pub struct Control(Arc<State>);
impl Control {
    /// Synchronously wake blocked readers, writers and flushes, even if the
    /// async runtime is stalled. The relay requests bounded SSH cleanup.
    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::Release);
        self.0.cancel.close();
        self.close_pipes();
    }
    fn close_pipes(&self) {
        self.0.input.close();
        self.0.output.close();
        // A pending flush must not retain its reply sender after cancellation.
        while self.0.pending.try_recv().is_ok() {}
    }
    fn check(&self) -> io::Result<()> {
        if self.0.cancelled.load(Ordering::Acquire) {
            Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "Graphical channel cancelled",
            ))
        } else {
            Ok(())
        }
    }
    /// Bounded cleanup confirmation for a blocking worker, never the UI or a
    /// Tokio executor thread. Timeout cancels this channel and reports failure;
    /// it does not claim that a remote process was killed.
    pub fn finish(&self) -> Result<Report, String> {
        let (report, _) = self
            .0
            .completed
            .wait_timeout_while(self.0.report.lock().unwrap(), FINISH_LIMIT, |report| {
                report.is_none()
            })
            .unwrap();
        if let Some(report) = report.as_ref() {
            return Ok(report.clone());
        }
        drop(report);
        self.cancel();
        Err("Graphical channel cleanup did not finish in time".into())
    }
    fn complete(&self, report: Report) {
        self.close_pipes();
        *self.0.report.lock().unwrap() = Some(report);
        self.0.completed.notify_all();
    }
}

pub struct Reader {
    control: Control,
    pending: io::Cursor<Vec<u8>>,
}
impl Read for Reader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        self.control.check()?;
        loop {
            let count = self.pending.read(buffer)?;
            if count != 0 {
                return Ok(count);
            }
            match self.control.0.output.recv_blocking() {
                Ok(data) => {
                    self.control.check()?;
                    self.pending = io::Cursor::new(data);
                }
                Err(_) => {
                    self.control.check()?;
                    return Ok(0);
                }
            }
        }
    }
}
impl Drop for Reader {
    fn drop(&mut self) {
        self.control.cancel();
    }
}
pub struct Writer {
    control: Control,
}
impl Write for Writer {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        self.control.check()?;
        let length = buffer.len().min(CHUNK);
        self.control
            .0
            .input
            .send_blocking(Input::Data(buffer[..length].to_vec()))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "Graphical input closed"))?;
        Ok(length)
    }
    /// A barrier for bytes accepted by the SSH sending pipeline, not a remote
    /// application acknowledgment. Cancellation also unblocks this barrier.
    fn flush(&mut self) -> io::Result<()> {
        self.control.check()?;
        let (reply, done) = async_channel::bounded(1);
        self.control
            .0
            .input
            .send_blocking(Input::Flushed(reply))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "Graphical input closed"))?;
        done.recv_blocking()
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "Graphical flush cancelled"))
    }
}
impl Drop for Writer {
    fn drop(&mut self) {
        self.control.0.input.close();
    }
}

/// Owned unclaimed halves. Dropping this value cancels the relay; into_parts
/// transfers cleanup responsibility to the caller and the IO halves.
pub struct Channel {
    reader: Option<Reader>,
    writer: Option<Writer>,
    control: Control,
}
impl Channel {
    /// Call inside the backend Tokio runtime. The remote command is already
    /// admitted; this does not authenticate, execute, or open another session.
    pub fn attach(channel: impl Into<crate::service_channel::Channel>) -> Self {
        let (input, pending) = async_channel::bounded(INPUT_CHUNKS);
        let (output, received) = async_channel::bounded(OUTPUT_CHUNKS);
        let (cancel, cancelled) = async_channel::bounded(1);
        let control = Control(Arc::new(State {
            input,
            pending,
            output: received,
            cancel,
            cancelled: AtomicBool::new(false),
            report: Mutex::new(None),
            completed: Condvar::new(),
        }));
        // Construct the guard before spawning, including an unpolled task
        // dropped during runtime shutdown. Every exit wakes blocking consumers.
        let completion = Completion {
            control: control.clone(),
            report: Some(Report::default()),
        };
        tokio::spawn(relay(channel.into(), output, cancelled, completion));
        Self {
            reader: Some(Reader {
                control: control.clone(),
                pending: io::Cursor::new(Vec::new()),
            }),
            writer: Some(Writer {
                control: control.clone(),
            }),
            control,
        }
    }
    pub fn into_parts(mut self) -> (Reader, Writer, Control) {
        (
            self.reader.take().unwrap(),
            self.writer.take().unwrap(),
            self.control.clone(),
        )
    }
}
impl Drop for Channel {
    fn drop(&mut self) {
        if self.reader.is_some() {
            self.control.cancel();
        }
    }
}
struct Completion {
    control: Control,
    report: Option<Report>,
}
impl Drop for Completion {
    fn drop(&mut self) {
        let Some(mut report) = self.report.take() else {
            return;
        };
        report
            .error
            .get_or_insert_with(|| "Graphical channel task ended before cleanup".into());
        self.control.complete(report);
    }
}

async fn relay(
    channel: crate::service_channel::Channel,
    output: Sender<Vec<u8>>,
    cancelled: Receiver<()>,
    mut completion: Completion,
) {
    let (mut remote, writer) = channel.split();
    let control = completion.control.clone();
    let report = completion.report.as_mut().unwrap();
    let result = {
        // Poll both directions independently. Waiting for the remote input
        // window must never prevent reading the output that might release it.
        let forward = async {
            while let Ok(input) = control.0.pending.recv().await {
                match input {
                    Input::Data(data) => writer.data(&data[..]).await.map_err(|e| e.to_string())?,
                    Input::Flushed(reply) => {
                        let _ = reply.try_send(());
                    }
                }
            }
            writer.eof().await.map_err(|e| e.to_string())
        };
        let receive = async {
            while let Some(message) = remote.wait().await? {
                match message {
                    ChannelMsg::Data { data } => {
                        for chunk in data.chunks(CHUNK) {
                            // Russh's receive queue is bounded and shared SSH
                            // dispatch can wait on it. Bound a slow decoder's
                            // stall rather than freezing unrelated services.
                            tokio::time::timeout(OUTPUT_STALL, output.send(chunk.to_vec()))
                                .await
                                .map_err(|_| "Graphical output consumer stalled".to_owned())?
                                .map_err(|_| "Graphical output consumer closed".to_owned())?;
                        }
                    }
                    ChannelMsg::ExtendedData { data, .. } => keep_tail(&mut report.stderr, &data),
                    ChannelMsg::ExitStatus { exit_status } => report.status = Some(exit_status),
                    ChannelMsg::ExitSignal { signal_name, .. } => {
                        report.signal = Some(crate::service_channel::signal_text(&signal_name))
                    }
                    ChannelMsg::Failure => {
                        return Err("The server refused the graphical worker".into());
                    }
                    ChannelMsg::Close => break,
                    _ => {}
                }
            }
            if report.status.is_none() && report.signal.is_none() {
                Err("Graphical channel ended without an exit status".into())
            } else {
                Ok(())
            }
        };
        tokio::pin!(forward, receive);
        let mut input_closed = None;
        loop {
            let grace = async {
                match input_closed {
                    Some(at) => tokio::time::sleep_until(at + INPUT_CLOSED_GRACE).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                biased;
                _ = cancelled.recv() => break Ok(()),
                result = &mut receive => break result,
                result = &mut forward, if input_closed.is_none() => {
                    if let Err(error) = result { break Err(error); }
                    input_closed = Some(tokio::time::Instant::now());
                }
                _ = grace => break Err("Graphical worker did not exit after input closed".into()),
            }
        }
    };
    // Drop the receiver before sending close so a full per-channel SSH queue
    // cannot prevent the shared dispatcher from processing the close request.
    drop(remote);
    control.close_pipes();
    let close = tokio::time::timeout(CLOSE_LIMIT, writer.close()).await;
    report.error = result.err().or_else(|| match close {
        Ok(Ok(())) => None,
        Ok(Err(error)) => Some(format!("Graphical channel close failed: {error}")),
        Err(_) => Some("Graphical channel close timed out".into()),
    });
    let report = completion.report.take().unwrap();
    control.complete(report);
}
fn keep_tail(tail: &mut Vec<u8>, bytes: &[u8]) {
    if bytes.len() >= STDERR_LIMIT {
        tail.clear();
        tail.extend_from_slice(&bytes[bytes.len() - STDERR_LIMIT..]);
    } else {
        let excess = (tail.len() + bytes.len()).saturating_sub(STDERR_LIMIT);
        tail.drain(..excess);
        tail.extend_from_slice(bytes);
    }
}

#[cfg(test)]
mod tests;
