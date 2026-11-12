//! The framing over tokio's `AsyncRead` and `AsyncWrite`.

use std::io;

use ::tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use bytes::{Buf, Bytes, BytesMut};
use prost::Message;

use crate::encoding::protobuf::{
    FrameError, Layout, Limits, decode,
    frame::{LengthDecoder, RESERVE, encode_frame, encode_message},
};

/// The read buffer's starting size, and the least it grows by once frames
/// read from it are still held.
const BUFFER: usize = 8 * 1024;

/// Reads frames from an async reader into a buffer of its own.
///
/// [`next`](Self::next) and [`next_message`](Self::next_message) are cancel
/// safe: a partly read frame stays in the buffer, so dropping either one,
/// as a `select!` branch that lost, and calling it again reads on from
/// where it stopped. After an error the stream is not in sync and the
/// reader is done.
///
/// The stream ending before a frame's first byte is a clean end, whether a
/// read returns nothing or fails with `UnexpectedEof`: rustls reports a peer
/// that closed without `close_notify` that way. A caller that must tell the
/// two apart asks its transport.
pub struct FrameReader<R> {
    reader: R,
    limits: Limits,
    buffer: BytesMut,
}

impl<R: AsyncRead + Unpin> FrameReader<R> {
    /// Reads `reader` with the protocol's `limits` for this direction.
    pub fn new(reader: R, limits: Limits) -> Self {
        Self {
            reader,
            limits,
            buffer: BytesMut::with_capacity(BUFFER),
        }
    }

    /// The next frame's body, or `None` at the end of the stream. A length
    /// over the message limit is refused before the body is waited for.
    pub async fn next(&mut self) -> Result<Option<Bytes>, FrameError> {
        loop {
            let missing = match self.frame()? {
                Ok(frame) => return Ok(Some(frame)),
                Err(missing) => missing,
            };
            self.buffer.reserve(missing.min(RESERVE));
            match self.reader.read_buf(&mut self.buffer).await {
                Ok(0) => {}
                Ok(_) => continue,
                Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {}
                Err(error) => return Err(error.into()),
            }
            return if self.buffer.is_empty() {
                Ok(None)
            } else {
                Err(FrameError::Truncated)
            };
        }
    }

    /// The next message, or `None` at the end of the stream.
    pub async fn next_message<M: Message + Default + Layout>(
        &mut self,
    ) -> Result<Option<M>, FrameError> {
        self.next()
            .await?
            .map(|frame| decode(frame, &self.limits))
            .transpose()
    }

    /// The reader, to write to when it is a whole duplex stream.
    pub fn get_mut(&mut self) -> &mut R {
        &mut self.reader
    }

    /// The reader, to inspect transport identity without disturbing buffered frames.
    pub fn get_ref(&self) -> &R {
        &self.reader
    }

    /// The reader, and what was read from it past the last frame returned.
    pub fn into_parts(self) -> (R, BytesMut) {
        (self.reader, self.buffer)
    }

    /// Takes the buffered frame if it is whole, or says how many more bytes
    /// it needs at least.
    fn frame(&mut self) -> Result<Result<Bytes, usize>, FrameError> {
        let mut length = LengthDecoder::default();
        for (header, &byte) in self.buffer.iter().enumerate() {
            let Some(len) = length.push(byte, self.limits.message)? else {
                continue;
            };
            let end = header + 1 + len;
            if self.buffer.len() < end {
                return Ok(Err(end - self.buffer.len()));
            }
            let mut frame = self.buffer.split_to(end);
            frame.advance(header + 1);
            return Ok(Ok(frame.freeze()));
        }
        Ok(Err(1))
    }
}

/// [`write_frame`](crate::write_frame) on an async writer.
///
/// Not cancel safe: dropped part way, it leaves part of a frame on the
/// stream, which the peer then misreads. Don't use it as a `select!`
/// branch, and close the stream if a write is cancelled.
pub async fn write_frame<W: AsyncWrite + Unpin + ?Sized>(
    writer: &mut W,
    body: &[u8],
) -> io::Result<()> {
    writer.write_all(&encode_frame(body)).await
}

/// [`write_message`](crate::write_message) on an async writer.
///
/// Not cancel safe, like [`write_frame`].
pub async fn write_message<M, W>(
    writer: &mut W,
    message: &M,
    limits: &Limits,
) -> Result<(), FrameError>
where
    M: Message + Layout,
    W: AsyncWrite + Unpin + ?Sized,
{
    let frame = encode_message(message, limits)?;
    Ok(writer.write_all(&frame).await?)
}
