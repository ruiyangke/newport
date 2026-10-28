//! A feature channel pins its negotiated runtime until the channel is released.
use super::{Control, protocol};
use std::{
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    sync::OwnedSemaphorePermit,
};

#[derive(Clone)]
pub(crate) struct Handle(pub(super) Arc<Control>);
impl Handle {
    pub(crate) fn is_draining(&self) -> bool {
        *self.0.drain.borrow()
    }
    pub(crate) async fn draining(&self) {
        let mut drain = self.0.drain.subscribe();
        while !*drain.borrow_and_update() {
            if drain.changed().await.is_err() {
                return;
            }
        }
    }
    pub(crate) fn is_closed(&self) -> bool {
        self.0.is_closed()
    }
    pub(crate) fn hello(&self) -> &protocol::Hello {
        &self.0.hello
    }
}
pub(crate) struct Worker {
    pub(crate) runtime: Handle,
    pub(crate) id: uuid::Uuid,
    pub(crate) endpoint: Option<protocol::worker_stream::Endpoint>,
    pub(super) _permit: OwnedSemaphorePermit,
}
impl Worker {
    pub(crate) fn stream(self, stream: crate::service_channel::Stream) -> WorkerStream {
        WorkerStream {
            stream,
            worker: self,
        }
    }
}
pub struct WorkerStream {
    stream: crate::service_channel::Stream,
    worker: Worker,
}
impl WorkerStream {
    pub(crate) fn uses_session_slot(&self) -> bool {
        self.stream.uses_session_slot()
    }
    pub(crate) fn is_draining(&self) -> bool {
        self.worker.runtime.is_draining()
    }
    fn check(&self) -> io::Result<()> {
        if self.worker.runtime.is_closed() {
            Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "Needle runtime disconnected",
            ))
        } else {
            Ok(())
        }
    }
}
impl AsyncRead for WorkerStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if let Err(error) = self.check() {
            return Poll::Ready(Err(error));
        }
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}
impl AsyncWrite for WorkerStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        if let Err(error) = self.check() {
            return Poll::Ready(Err(error));
        }
        Pin::new(&mut self.stream).poll_write(cx, bytes)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}
