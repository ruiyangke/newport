//! Bounded asynchronous service IO. Input EOF leaves output and final status
//! readable; dropping the stream cancels only its own worker channel.
use super::{Channel, signal_text};
use russh::ChannelMsg;
use std::{
    future::Future,
    io,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    sync::{mpsc, oneshot},
};

const CHUNK: usize = 32 * 1024;
const STDERR_LIMIT: usize = 64 * 1024;
const OUTPUT_STALL: Duration = Duration::from_secs(2);
type Outcome = Arc<Mutex<Option<Result<(), String>>>>;
type Reservation = Pin<
    Box<dyn Future<Output = Result<mpsc::OwnedPermit<Input>, mpsc::error::SendError<()>>> + Send>,
>;
type Barrier = Pin<Box<dyn Future<Output = io::Result<()>> + Send>>;

enum Input {
    Data(Vec<u8>),
    Flush(oneshot::Sender<Result<(), String>>),
    Eof(oneshot::Sender<Result<(), String>>),
}

pub struct Stream {
    uses_session_slot: bool,
    fate: Option<newport_ssh::Fate>,
    input: mpsc::Sender<Input>,
    output: mpsc::Receiver<Vec<u8>>,
    pending: io::Cursor<Vec<u8>>,
    reserve: Option<Reservation>,
    flush: Option<Barrier>,
    shutdown: Option<Barrier>,
    closing: bool,
    closed: bool,
    outcome: Outcome,
    cancel: Option<oneshot::Sender<()>>,
}
impl Stream {
    pub(super) fn new(channel: Channel) -> Self {
        let uses_session_slot = !channel.reader.framed;
        let fate = channel.fate.clone();
        let (input, pending) = mpsc::channel(8);
        let (output, received) = mpsc::channel(32);
        let (cancel, cancelled) = oneshot::channel();
        let outcome = Arc::new(Mutex::new(None));
        // Create this before spawning so even an unpolled task dropped during
        // runtime shutdown cannot make a lost worker look like a clean EOF.
        let completion = Completion(outcome.clone());
        tokio::spawn(relay(channel, pending, output, cancelled, completion));
        Self {
            uses_session_slot,
            fate,
            input,
            output: received,
            pending: io::Cursor::new(Vec::new()),
            reserve: None,
            flush: None,
            shutdown: None,
            closing: false,
            closed: false,
            outcome,
            cancel: Some(cancel),
        }
    }
    pub(crate) fn uses_session_slot(&self) -> bool {
        self.uses_session_slot
    }
    /// The fate of the link connection under the stream.
    pub(crate) fn fate(&self) -> Option<&newport_ssh::Fate> {
        self.fate.as_ref()
    }
    fn barrier(&self, eof: bool) -> Barrier {
        let input = self.input.clone();
        Box::pin(async move {
            let (reply, done) = oneshot::channel();
            input
                .send(if eof {
                    Input::Eof(reply)
                } else {
                    Input::Flush(reply)
                })
                .await
                .map_err(|_| broken("Worker input closed"))?;
            done.await
                .map_err(|_| broken("Worker input barrier cancelled"))?
                .map_err(broken)
        })
    }
}
impl Drop for Stream {
    fn drop(&mut self) {
        self.output.close();
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
    }
}
fn broken(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, message.into())
}
impl AsyncRead for Stream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        loop {
            let position = self.pending.position() as usize;
            let available = &self.pending.get_ref()[position..];
            if !available.is_empty() {
                let length = available.len().min(buf.remaining());
                buf.put_slice(&available[..length]);
                self.pending.set_position((position + length) as u64);
                return Poll::Ready(Ok(()));
            }
            match self.output.poll_recv(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Some(data)) => self.pending = io::Cursor::new(data),
                Poll::Ready(None) => {
                    let outcome = self.outcome.lock().unwrap();
                    return Poll::Ready(match outcome.as_ref() {
                        Some(Ok(())) => Ok(()),
                        Some(Err(error)) => Err(io::Error::other(error.clone())),
                        None => Err(io::Error::other("Worker ended without a final outcome")),
                    });
                }
            }
        }
    }
}
impl AsyncWrite for Stream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.closing {
            return Poll::Ready(Err(broken("Worker input closed")));
        }
        if bytes.is_empty() {
            return Poll::Ready(Ok(0));
        }
        if self.flush.is_some() {
            std::task::ready!(self.as_mut().poll_flush(cx))?;
        }
        if self.reserve.is_none() {
            self.reserve = Some(Box::pin(self.input.clone().reserve_owned()));
        }
        let permit = std::task::ready!(self.reserve.as_mut().unwrap().as_mut().poll(cx));
        self.reserve = None;
        let permit = permit.map_err(|_| broken("Worker input closed"))?;
        // Never copy or accept caller bytes on Poll::Pending. A cancelled
        // write future may be followed by a write with entirely different data.
        let length = bytes.len().min(CHUNK);
        permit.send(Input::Data(bytes[..length].to_vec()));
        Poll::Ready(Ok(length))
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.reserve = None;
        if self.closed {
            return Poll::Ready(Ok(()));
        }
        if self.flush.is_none() {
            self.flush = Some(self.barrier(false));
        }
        let result = std::task::ready!(self.flush.as_mut().unwrap().as_mut().poll(cx));
        self.flush = None;
        Poll::Ready(result)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.closing = true;
        self.reserve = None;
        if self.closed {
            return Poll::Ready(Ok(()));
        }
        if self.flush.is_some() {
            std::task::ready!(self.as_mut().poll_flush(cx))?;
        }
        if self.shutdown.is_none() {
            self.shutdown = Some(self.barrier(true));
        }
        let result = std::task::ready!(self.shutdown.as_mut().unwrap().as_mut().poll(cx));
        self.shutdown = None;
        if result.is_ok() {
            self.closed = true;
        }
        Poll::Ready(result)
    }
}

struct Completion(Outcome);
impl Completion {
    fn finish(&self, result: Result<(), String>) {
        *self.0.lock().unwrap() = Some(result);
    }
}
impl Drop for Completion {
    fn drop(&mut self) {
        self.0
            .lock()
            .unwrap()
            .get_or_insert_with(|| Err("Worker channel task ended before cleanup".into()));
    }
}

async fn relay(
    channel: Channel,
    mut input: mpsc::Receiver<Input>,
    output: mpsc::Sender<Vec<u8>>,
    mut cancelled: oneshot::Receiver<()>,
    completion: Completion,
) {
    let (mut reader, writer) = channel.split();
    let result = {
        let forward = async {
            while let Some(message) = input.recv().await {
                match message {
                    Input::Data(data) => writer.data(&data).await?,
                    Input::Flush(reply) => {
                        let _ = reply.send(Ok(()));
                    }
                    Input::Eof(reply) => {
                        let result = writer.eof().await;
                        let _ = reply.send(result.clone());
                        return result;
                    }
                }
            }
            Err("Worker input disappeared without EOF".into())
        };
        let receive = async {
            let mut stderr = Vec::new();
            let mut status = None;
            let mut signal = None;
            while let Some(message) = reader.wait().await? {
                match message {
                    ChannelMsg::Data { data } => {
                        for chunk in data.chunks(CHUNK) {
                            tokio::time::timeout(OUTPUT_STALL, output.send(chunk.to_vec()))
                                .await
                                .map_err(|_| "Worker output consumer stalled")?
                                .map_err(|_| "Worker output consumer closed")?;
                        }
                    }
                    ChannelMsg::ExtendedData { data, .. } => {
                        if data.len() >= STDERR_LIMIT {
                            stderr.clear();
                            stderr.extend_from_slice(&data[data.len() - STDERR_LIMIT..]);
                        } else {
                            let excess = (stderr.len() + data.len()).saturating_sub(STDERR_LIMIT);
                            stderr.drain(..excess);
                            stderr.extend_from_slice(&data);
                        }
                    }
                    ChannelMsg::ExitStatus { exit_status } => status = Some(exit_status),
                    ChannelMsg::ExitSignal { signal_name, .. } => {
                        signal = Some(signal_text(&signal_name))
                    }
                    ChannelMsg::Failure => return Err("Server refused the worker".into()),
                    ChannelMsg::Close => break,
                    _ => (),
                }
            }
            let failure = if let Some(signal) = signal {
                Some(format!("Worker exited on {signal}"))
            } else {
                match status {
                    Some(0) => None,
                    Some(status) => Some(format!("Worker exited with status {status}")),
                    None => Some("Worker ended without an exit status".into()),
                }
            };
            match failure {
                Some(error) if !stderr.is_empty() => {
                    Err(format!("{error}: {}", String::from_utf8_lossy(&stderr)))
                }
                Some(error) => Err(error),
                None => Ok(()),
            }
        };
        tokio::pin!(forward, receive);
        let mut input_closed = false;
        loop {
            tokio::select! {
                biased;
                _ = &mut cancelled => break Err("Worker channel cancelled".into()),
                result = &mut receive => break result,
                result = &mut forward, if !input_closed => {
                    if let Err(error) = result { break Err(error); }
                    input_closed = true;
                }
            }
        }
    };
    drop(reader);
    // Release queued input and barriers before waiting for transport cleanup.
    drop(input);
    let close = tokio::time::timeout(Duration::from_secs(1), writer.close()).await;
    completion.finish(result.and_then(|()| match close {
        Ok(result) => result,
        Err(_) => Err("Worker channel close timed out".into()),
    }));
}
