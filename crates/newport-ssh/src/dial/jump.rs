use crate::{Abort, BoxFuture, Dialed, Dialer, Error, Link, Login, Open, Retry, Target};
use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, DuplexStream, ReadBuf},
    sync::watch,
    task::AbortHandle,
};

/// How much the stream buffers between the target's connection and the
/// channel; the channel's own window bounds the rest.
const BUFFER: usize = 64 * 1024;

/// Reaches the target through a `direct-tcpip` channel on the jump host's
/// link, as ssh's `ProxyJump` does. The jump host logs in once, through its
/// own link, for every server behind it, and its channels hold no session
/// slots. A chain is a jump host whose link has a `JumpDialer` too.
pub struct JumpDialer {
    jump: Link,
}

impl JumpDialer {
    pub fn new(jump: Link) -> Self {
        Self { jump }
    }

    /// The channel to the target, reporting the jump host's approval wait as
    /// this dial's. A jump host whose login fails meanwhile fails the dial
    /// with its error, so this link backs off and says why; one that stopped
    /// retrying fails it until Retry on this link clears it.
    async fn open(
        &self,
        target: &Target,
        approval: &watch::Sender<bool>,
    ) -> Result<crate::Channel, Error> {
        let mut status = self.jump.status();
        let opening = self.jump.open_unless_failing(Open::DirectTcpip {
            host: target.host.clone(),
            port: target.port,
        });
        tokio::pin!(opening);
        let result = loop {
            let waiting = status.borrow_and_update().login == Some(Login::WaitingForApproval);
            approve(approval, waiting);
            tokio::select! {
                result = &mut opening => break result,
                changed = status.changed() => {
                    if changed.is_err() {
                        break (&mut opening).await;
                    }
                }
            }
        };
        approve(approval, false);
        result
    }
}

fn approve(approval: &watch::Sender<bool>, waiting: bool) {
    approval.send_if_modified(|current| std::mem::replace(current, waiting) != waiting);
}

impl Dialer for JumpDialer {
    fn dial<'a>(
        &'a self,
        target: &'a Target,
        approval: &'a watch::Sender<bool>,
    ) -> BoxFuture<'a, Result<Dialed, Error>> {
        Box::pin(async move {
            let channel = self
                .open(target, approval)
                .await
                .map_err(|error| match error {
                    // The target may come back, so this is retried like any
                    // unreachable server.
                    Error::ChannelRefused { reason } => Error::Unreachable(format!(
                        "the jump host couldn't connect to {}:{} ({reason:?})",
                        target.host, target.port
                    )),
                    Error::Busy => {
                        Error::Unreachable("the jump host has too many requests waiting".into())
                    }
                    Error::Closed => Error::Unreachable("the jump host's link closed".into()),
                    Error::Unreachable(cause) => {
                        Error::Unreachable(format!("its jump host couldn't be reached: {cause}"))
                    }
                    error if error.retry() == Retry::Backoff => {
                        Error::Unreachable(format!("its jump host: {error}"))
                    }
                    error => error,
                })?;
            let mut channel = channel.into_stream();
            let (stream, mut far) = tokio::io::duplex(BUFFER);
            let pump = tokio::spawn(async move {
                let _ = tokio::io::copy_bidirectional(&mut far, &mut channel).await;
            })
            .abort_handle();
            Ok(Dialed {
                stream: Box::new(JumpStream {
                    stream,
                    pump: pump.clone(),
                }),
                // Ending the pump drops the channel, which closes it.
                abort: Abort::new(move || pump.abort()),
            })
        })
    }

    /// The jump link's login has its own deadlines and approval wait.
    fn deadline(&self, _: Duration) -> Option<Duration> {
        None
    }

    fn retry(&self) {
        self.jump.retry();
    }

    fn retry_fresh(&self) {
        self.jump.retry_fresh();
    }
}

/// The link's end of the channel. Dropping it closes the channel.
struct JumpStream {
    stream: DuplexStream,
    pump: AbortHandle,
}

impl AsyncRead for JumpStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for JumpStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

impl Drop for JumpStream {
    fn drop(&mut self) {
        self.pump.abort();
    }
}
