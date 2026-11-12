//! Test-only accounting at the plaintext RPC boundary, before SSH encryption.
use std::{
    io,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
#[derive(Default)]
pub(super) struct Counters {
    sent: AtomicU64,
    received: AtomicU64,
    sent_frames: AtomicU64,
    received_frames: AtomicU64,
}
impl Counters {
    pub(super) fn snapshot(&self) -> [u64; 4] {
        [
            &self.sent,
            &self.received,
            &self.sent_frames,
            &self.received_frames,
        ]
        .map(|n| n.load(Ordering::Relaxed))
    }
}
#[derive(Default)]
struct Frames {
    length: newport_protocol::encoding::protobuf::LengthDecoder,
    remaining: usize,
}
impl Frames {
    fn feed(&mut self, mut bytes: &[u8]) -> u64 {
        let mut count = 0;
        while !bytes.is_empty() {
            if self.remaining > 0 {
                let n = self.remaining.min(bytes.len());
                self.remaining -= n;
                bytes = &bytes[n..];
            } else {
                let next = self
                    .length
                    .push(bytes[0], newport_protocol::git::MAX_FRAME)
                    .expect("valid measured frame");
                bytes = &bytes[1..];
                if let Some(length) = next {
                    self.remaining = length;
                    self.length = Default::default();
                    count += 1;
                }
            }
        }
        count
    }
}
pub(super) struct Stream<S> {
    inner: S,
    pub(super) counters: Arc<Counters>,
    input: Frames,
    output: Frames,
}
impl<S> Stream<S> {
    pub(super) fn new(inner: S) -> Self {
        Self {
            inner,
            counters: Arc::default(),
            input: Frames::default(),
            output: Frames::default(),
        }
    }
}
impl<S: AsyncRead + Unpin> AsyncRead for Stream<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let before = buf.filled().len();
        let result = Pin::new(&mut this.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = &result {
            let bytes = &buf.filled()[before..];
            this.counters
                .received
                .fetch_add(bytes.len() as u64, Ordering::Relaxed);
            let frames = this.input.feed(bytes);
            this.counters
                .received_frames
                .fetch_add(frames, Ordering::Relaxed);
        }
        result
    }
}
impl<S: AsyncWrite + Unpin> AsyncWrite for Stream<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let result = Pin::new(&mut this.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(n)) = &result {
            this.counters.sent.fetch_add(*n as u64, Ordering::Relaxed);
            let frames = this.output.feed(&buf[..*n]);
            this.counters
                .sent_frames
                .fetch_add(frames, Ordering::Relaxed);
        }
        result
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frames_are_counted_across_fragmentation_and_coalescing() {
        let bytes = ["one", "two", "three"]
            .into_iter()
            .flat_map(|nonce| {
                newport_protocol::git::wire::Message::encode(
                    &newport_protocol::git::ClientMessage::Ping(nonce.into()),
                )
                .unwrap()
            })
            .collect::<Vec<_>>();
        for width in 1..=bytes.len() {
            let mut frames = Frames::default();
            assert_eq!(bytes.chunks(width).map(|b| frames.feed(b)).sum::<u64>(), 3);
            assert_eq!(frames.remaining, 0);
            assert_eq!(frames.feed(&bytes), 3);
        }
    }
}
