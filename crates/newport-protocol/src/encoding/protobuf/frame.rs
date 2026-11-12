use std::io::{self, Read, Write};

use bytes::Bytes;
use prost::Message;

use crate::encoding::protobuf::{FrameError, Layout, Limits, layout::check_metadata};

/// The most of a frame's body reserved before it arrives; the buffer grows
/// as the rest does, so a length alone never allocates the limit.
pub(crate) const RESERVE: usize = 64 * 1024;

/// Decodes one message from `frame`, a frame's body or a Mach message's,
/// after checking it against `limits`. Its `bytes` fields borrow from `frame`.
pub fn decode<M: Message + Default + Layout>(
    frame: Bytes,
    limits: &Limits,
) -> Result<M, FrameError> {
    if frame.len() > limits.message {
        return Err(FrameError::TooLarge {
            length: frame.len() as u64,
            limit: limits.message,
        });
    }
    check_metadata(&frame, M::fields(), limits.metadata)?;
    Ok(M::decode(frame)?)
}

/// Reads one frame's body, or `None` at the end of the stream. A length over
/// `limit` is refused before any of the body is read.
///
/// The stream ending before a frame's first byte is a clean end, whether a
/// read returns nothing or fails with `UnexpectedEof`: rustls reports a peer
/// that closed without `close_notify` that way. A caller that must tell the
/// two apart asks its transport.
///
/// The length is read a byte at a time; give it a buffered reader.
pub fn read_frame<R: Read + ?Sized>(
    reader: &mut R,
    limit: usize,
) -> Result<Option<Bytes>, FrameError> {
    let mut length = LengthDecoder::default();
    let mut byte = [0];
    let len = loop {
        match reader.read_exact(&mut byte) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
                return if length.bytes == 0 {
                    Ok(None)
                } else {
                    Err(FrameError::Truncated)
                };
            }
            Err(error) => return Err(error.into()),
        }
        if let Some(len) = length.push(byte[0], limit)? {
            break len;
        }
    };
    let mut body = Vec::with_capacity(len.min(RESERVE));
    match reader.take(len as u64).read_to_end(&mut body) {
        Ok(read) if read == len => Ok(Some(body.into())),
        Ok(_) => Err(FrameError::Truncated),
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Err(FrameError::Truncated),
        Err(error) => Err(error.into()),
    }
}

/// Reads and decodes one message, or `None` at the end of the stream.
pub fn read_message<M, R>(reader: &mut R, limits: &Limits) -> Result<Option<M>, FrameError>
where
    M: Message + Default + Layout,
    R: Read + ?Sized,
{
    read_frame(reader, limits.message)?
        .map(|frame| decode(frame, limits))
        .transpose()
}

/// Writes `body` as one frame, in one write.
pub fn write_frame<W: Write + ?Sized>(writer: &mut W, body: &[u8]) -> io::Result<()> {
    writer.write_all(&encode_frame(body))
}

/// Writes `message` as one frame, in one write, after checking it against
/// `limits`, the ones the peer reads with.
pub fn write_message<M, W>(writer: &mut W, message: &M, limits: &Limits) -> Result<(), FrameError>
where
    M: Message + Layout,
    W: Write + ?Sized,
{
    Ok(writer.write_all(&encode_message(message, limits)?)?)
}

/// `body` with its length in front.
pub(crate) fn encode_frame(body: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(prost::length_delimiter_len(body.len()) + body.len());
    prost::encoding::encode_varint(body.len() as u64, &mut frame);
    frame.extend_from_slice(body);
    frame
}

/// `message` as a frame, if it is within `limits`.
pub(crate) fn encode_message<M: Message + Layout>(
    message: &M,
    limits: &Limits,
) -> Result<Vec<u8>, FrameError> {
    let len = message.encoded_len();
    if len > limits.message {
        return Err(FrameError::TooLarge {
            length: len as u64,
            limit: limits.message,
        });
    }
    let frame = message.encode_length_delimited_to_vec();
    check_metadata(&frame[frame.len() - len..], M::fields(), limits.metadata)?;
    Ok(frame)
}

/// A frame's varint length, read a byte at a time.
#[derive(Default)]
pub struct LengthDecoder {
    value: u64,
    complete: bool,
    pub(crate) bytes: u32,
}

impl LengthDecoder {
    /// Adds the next byte; returns the length once complete. Create a new decoder
    /// after completion or failure. Use the same limit for the whole prefix.
    pub fn push(&mut self, byte: u8, limit: usize) -> Result<Option<usize>, FrameError> {
        if self.complete {
            return Err(FrameError::Malformed);
        }
        // A u64 fills nine bytes and the low bit of a tenth; anything more
        // is a length no limit allows.
        if self.bytes == 9 && byte > 1 {
            self.complete = true;
            return Err(FrameError::TooLarge {
                length: u64::MAX,
                limit,
            });
        }
        self.value |= u64::from(byte & 0x7f) << (7 * self.bytes);
        self.bytes += 1;
        if byte & 0x80 != 0 {
            return Ok(None);
        }
        self.complete = true;
        match usize::try_from(self.value) {
            Ok(len) if len <= limit => Ok(Some(len)),
            _ => Err(FrameError::TooLarge {
                length: self.value,
                limit,
            }),
        }
    }
}
