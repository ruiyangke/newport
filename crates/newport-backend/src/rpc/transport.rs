//! Server-side framing and allocation admission. Bytes read past an envelope
//! remain in this reader when the connection changes to a retained stream.
use newport_protocol::{
    desktop::{self, Allocation, wire},
    encoding::protobuf::{EncodedFrame, FrameDecoder, LengthDecoder},
};
use std::{
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf},
    sync::{OwnedSemaphorePermit, Semaphore},
    time::Instant,
};

pub(super) struct Received {
    pub frame: EncodedFrame,
    /// Kept with the decoded request until its bulk allocation is released.
    pub large: Option<OwnedSemaphorePermit>,
}

struct Pending {
    length: LengthDecoder,
    prefix: [u8; 10],
    prefix_len: usize,
    decoder: Option<FrameDecoder>,
    limit: usize,
    large_pool: Option<Arc<Semaphore>>,
    large: Option<OwnedSemaphorePermit>,
    deadline: Option<Instant>,
}

/// Partial prefixes, bodies and surplus bytes survive cancellation of `frame`.
/// Any framing error retires the reader. Raw reads are legal only between frames.
pub(super) struct Reader<R> {
    inner: R,
    buffer: [u8; 8192],
    start: usize,
    end: usize,
    buffer_deadline: Option<Instant>,
    pending: Option<Pending>,
    retired: bool,
}

impl<R> Reader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            buffer: [0; 8192],
            start: 0,
            end: 0,
            buffer_deadline: None,
            pending: None,
            retired: false,
        }
    }

    pub fn has_buffered(&self) -> bool {
        self.start < self.end
    }

    fn consume(&mut self) -> io::Result<Option<Received>> {
        let pending = self
            .pending
            .as_mut()
            .ok_or_else(|| io::Error::other("Missing frame state"))?;
        if self.start < self.end
            && let Some(deadline) = self.buffer_deadline
        {
            pending.deadline.get_or_insert(deadline);
        }
        while pending.decoder.is_none() && self.start < self.end {
            let byte = self.buffer[self.start];
            self.start += 1;
            let slot = pending.prefix.get_mut(pending.prefix_len).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "Overlong frame length")
            })?;
            *slot = byte;
            pending.prefix_len += 1;
            let length = pending
                .length
                .push(byte, pending.limit)
                .map_err(wire::Error::from)?;
            if let Some(length) = length {
                if let Some(pool) = &pending.large_pool {
                    let length = u64::try_from(length)
                        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                    if desktop::request_allocation(length).map_err(wire::Error::from)?
                        == Allocation::Large
                    {
                        // No body allocation occurs before this permit. At this
                        // point there is no decoded request ID to acknowledge.
                        pending.large =
                            Some(pool.clone().try_acquire_owned().map_err(|error| {
                                io::Error::new(io::ErrorKind::WouldBlock, error)
                            })?);
                    }
                }
                let mut decoder = FrameDecoder::new(pending.limit);
                let frame = decoder
                    .consume(&mut &pending.prefix[..pending.prefix_len])
                    .map_err(wire::Error::from)?;
                pending.decoder = Some(decoder);
                if let Some(frame) = frame {
                    let large = pending.large.take();
                    self.pending = None;
                    return Ok(Some(Received { frame, large }));
                }
            }
        }
        if let Some(decoder) = &mut pending.decoder {
            let mut bytes = &self.buffer[self.start..self.end];
            let frame = decoder.consume(&mut bytes).map_err(wire::Error::from)?;
            self.start = self.end - bytes.len();
            if let Some(frame) = frame {
                let large = pending.large.take();
                self.pending = None;
                return Ok(Some(Received { frame, large }));
            }
        }
        Ok(None)
    }
}

impl<R: AsyncRead + Unpin> Reader<R> {
    pub async fn frame(
        &mut self,
        limit: usize,
        large_pool: Option<Arc<Semaphore>>,
    ) -> io::Result<Option<Received>> {
        if self.retired {
            return Err(io::ErrorKind::NotConnected.into());
        }
        if let Some(pending) = &self.pending {
            if pending.limit != limit || pending.large_pool.is_some() != large_pool.is_some() {
                self.retired = true;
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Frame policy changed during a read",
                ));
            }
        } else {
            self.pending = Some(Pending {
                length: LengthDecoder::default(),
                prefix: [0; 10],
                prefix_len: 0,
                decoder: None,
                limit,
                large_pool,
                large: None,
                deadline: None,
            });
        }
        let result = self.next().await;
        if result.is_err() {
            self.retired = true;
            self.pending = None;
        }
        result
    }

    async fn next(&mut self) -> io::Result<Option<Received>> {
        loop {
            if let Some(frame) = self.consume()? {
                return Ok(Some(frame));
            }
            let deadline = self.pending.as_ref().and_then(|pending| pending.deadline);
            // Idle leases may wait indefinitely; once the first byte arrives,
            // the same deadline spans prefix, body, cancellation and progress.
            let read = if let Some(deadline) = deadline {
                if Instant::now() >= deadline {
                    return Err(io::ErrorKind::TimedOut.into());
                }
                let read = tokio::time::timeout_at(deadline, self.inner.read(&mut self.buffer))
                    .await
                    .map_err(|error| io::Error::new(io::ErrorKind::TimedOut, error))??;
                if Instant::now() >= deadline {
                    return Err(io::ErrorKind::TimedOut.into());
                }
                read
            } else {
                self.inner.read(&mut self.buffer).await?
            };
            if read == 0 {
                let partial = self
                    .pending
                    .as_ref()
                    .is_some_and(|pending| pending.prefix_len != 0);
                self.retired = true;
                self.pending = None;
                return if partial {
                    Err(io::ErrorKind::UnexpectedEof.into())
                } else {
                    Ok(None)
                };
            }
            self.start = 0;
            self.end = read;
            self.buffer_deadline = Some(Instant::now() + desktop::RESPONSE_WRITE_TIMEOUT);
        }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for Reader<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        output: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.retired {
            return Poll::Ready(Err(io::ErrorKind::NotConnected.into()));
        }
        if this.pending.is_some() {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Raw read inside an envelope",
            )));
        }
        if this.start < this.end {
            let count = output.remaining().min(this.end - this.start);
            output.put_slice(&this.buffer[this.start..this.start + count]);
            this.start += count;
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut this.inner).poll_read(cx, output)
    }
}

/// Once a write begins, every error is terminal for the connection. Callers
/// discard the complete writer; no later response follows a partial frame.
pub(super) async fn write_until(
    writer: &mut (impl AsyncWrite + Unpin),
    bytes: &[u8],
    deadline: Instant,
) -> io::Result<()> {
    if Instant::now() >= deadline {
        return Err(io::ErrorKind::TimedOut.into());
    }
    tokio::time::timeout_at(deadline, async {
        writer.write_all(bytes).await?;
        writer.flush().await
    })
    .await
    .map_err(|error| io::Error::new(io::ErrorKind::TimedOut, error))??;
    if Instant::now() >= deadline {
        return Err(io::ErrorKind::TimedOut.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    fn prefix(mut value: usize) -> Vec<u8> {
        let mut output = Vec::new();
        loop {
            let byte = u8::try_from(value & 127).unwrap();
            value >>= 7;
            output.push(if value == 0 { byte } else { byte | 128 });
            if value == 0 {
                return output;
            }
        }
    }
    #[tokio::test]
    async fn read_ahead_survives_stream_transition_with_exact_original_prefix() {
        let (mut peer, input) = tokio::io::duplex(1024);
        let mut reader = Reader::new(input);
        // The second frame deliberately uses a noncanonical but accepted prefix.
        peer.write_all(&[1, b'a', 0x81, 0, b'b']).await.unwrap();
        assert_eq!(
            reader
                .frame(32, None)
                .await
                .unwrap()
                .unwrap()
                .frame
                .into_bytes(),
            vec![1, b'a']
        );
        assert!(reader.has_buffered());
        assert_eq!(
            reader
                .frame(64, None)
                .await
                .unwrap()
                .unwrap()
                .frame
                .into_bytes(),
            vec![0x81, 0, b'b']
        );
    }
    #[tokio::test]
    async fn large_allocation_requires_a_permit_after_prefix_before_body() {
        let (mut peer, input) = tokio::io::duplex(128);
        let mut reader = Reader::new(input);
        let pool = Arc::new(Semaphore::new(0));
        peer.write_all(&prefix(desktop::MAX_REQUEST + 1))
            .await
            .unwrap();
        assert_eq!(
            reader
                .frame(desktop::MAX_SAVE_REQUEST, Some(pool))
                .await
                .err()
                .unwrap()
                .kind(),
            io::ErrorKind::WouldBlock
        );
        assert_eq!(
            reader
                .frame(desktop::MAX_SAVE_REQUEST, None)
                .await
                .err()
                .unwrap()
                .kind(),
            io::ErrorKind::NotConnected
        );
    }
    #[tokio::test]
    async fn cancelled_large_read_retains_permit_and_releases_it_when_retired() {
        let (mut peer, input) = tokio::io::duplex(128);
        let mut reader = Reader::new(input);
        let pool = Arc::new(Semaphore::new(1));
        peer.write_all(&prefix(desktop::MAX_REQUEST + 1))
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(
                Duration::from_millis(5),
                reader.frame(desktop::MAX_SAVE_REQUEST, Some(pool.clone()))
            )
            .await
            .is_err()
        );
        assert_eq!(pool.available_permits(), 0);
        drop(peer);
        assert_eq!(
            reader
                .frame(desktop::MAX_SAVE_REQUEST, Some(pool.clone()))
                .await
                .err()
                .unwrap()
                .kind(),
            io::ErrorKind::UnexpectedEof
        );
        assert_eq!(pool.available_permits(), 1);
    }
    #[tokio::test(start_paused = true)]
    async fn partial_frame_keeps_its_deadline_across_cancelled_reads_and_ready_bytes() {
        for first in [vec![0x80], vec![2, b'a']] {
            let (mut peer, input) = tokio::io::duplex(128);
            let mut reader = Reader::new(input);
            peer.write_all(&first).await.unwrap();
            assert!(
                tokio::time::timeout(Duration::from_millis(1), reader.frame(1024, None))
                    .await
                    .is_err()
            );
            let deadline = reader.pending.as_ref().unwrap().deadline.unwrap();
            tokio::time::advance(desktop::RESPONSE_WRITE_TIMEOUT).await;
            peer.write_all(&[1, b'b']).await.unwrap();
            assert!(Instant::now() >= deadline);
            assert_eq!(
                reader.frame(1024, None).await.err().unwrap().kind(),
                io::ErrorKind::TimedOut
            );
            assert!(reader.retired);
        }
    }
    #[tokio::test(start_paused = true)]
    async fn a_partial_read_ahead_frame_keeps_the_original_arrival_deadline() {
        let (mut peer, input) = tokio::io::duplex(128);
        let mut reader = Reader::new(input);
        peer.write_all(&[1, b'a', 2, b'b']).await.unwrap();
        reader.frame(1024, None).await.unwrap().unwrap();
        let arrived = reader.buffer_deadline.unwrap();
        tokio::time::advance(desktop::RESPONSE_WRITE_TIMEOUT).await;
        peer.write_all(b"c").await.unwrap();
        assert!(Instant::now() >= arrived);
        assert_eq!(
            reader.frame(1024, None).await.err().unwrap().kind(),
            io::ErrorKind::TimedOut
        );
    }
}
