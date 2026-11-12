//! Incremental framing for relays that retain the sender's exact bytes.
use super::{FrameError, LengthDecoder};
use bytes::{Bytes, BytesMut};
use std::fmt;

/// A bounded frame, including its original length prefix. Its message still
/// needs the receiving protocol's structural and semantic validation.
#[derive(Clone, PartialEq, Eq)]
pub struct EncodedFrame {
    bytes: Bytes,
    header: usize,
}

impl EncodedFrame {
    /// The body shares its allocation with the complete frame.
    pub fn body(&self) -> Bytes {
        self.bytes.slice(self.header..)
    }

    /// The original length prefix and body, suitable for forwarding unchanged.
    pub fn into_bytes(self) -> Bytes {
        self.bytes
    }
}

impl fmt::Debug for EncodedFrame {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EncodedFrame")
            .field("length", &self.bytes.len())
            .finish_non_exhaustive()
    }
}

/// Retains partial headers and bodies between reads without owning the IO.
/// A declared length is checked before any body allocation; storage grows
/// only as bytes arrive. Errors and [`finish`](Self::finish) are terminal.
pub struct FrameDecoder {
    limit: usize,
    length: LengthDecoder,
    body_length: Option<usize>,
    header: usize,
    buffer: BytesMut,
    ended: bool,
}

impl FrameDecoder {
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            length: LengthDecoder::default(),
            body_length: None,
            header: 0,
            buffer: BytesMut::new(),
            ended: false,
        }
    }

    /// Consume at most one frame, leaving any following bytes in `input`.
    /// An empty input never discards a partially received frame.
    pub fn consume(&mut self, input: &mut &[u8]) -> Result<Option<EncodedFrame>, FrameError> {
        if self.ended {
            return Err(FrameError::Malformed);
        }
        let result = self.consume_frame(input);
        if result.is_err() {
            self.ended = true;
            self.buffer = BytesMut::new();
        }
        result
    }

    fn consume_frame(&mut self, input: &mut &[u8]) -> Result<Option<EncodedFrame>, FrameError> {
        while self.body_length.is_none() {
            let Some((&byte, rest)) = input.split_first() else {
                return Ok(None);
            };
            *input = rest;
            let length = self.length.push(byte, self.limit)?;
            self.buffer.extend_from_slice(&[byte]);
            if let Some(length) = length {
                self.body_length = Some(length);
                self.header = self.buffer.len();
            }
        }
        let length = self.body_length.ok_or(FrameError::Malformed)?;
        let remaining = length - (self.buffer.len() - self.header);
        let count = remaining.min(input.len());
        self.buffer.extend_from_slice(&input[..count]);
        *input = &input[count..];
        if count != remaining {
            return Ok(None);
        }
        let frame = EncodedFrame {
            bytes: std::mem::take(&mut self.buffer).freeze(),
            header: self.header,
        };
        self.length = LengthDecoder::default();
        self.body_length = None;
        self.header = 0;
        Ok(Some(frame))
    }

    /// Whether EOF or a worker control can follow without truncating a frame.
    pub fn is_at_boundary(&self) -> bool {
        !self.ended && self.buffer.is_empty()
    }

    /// End the byte stream, rejecting an incomplete prefix or body.
    pub fn finish(&mut self) -> Result<(), FrameError> {
        let complete = self.is_at_boundary();
        self.ended = true;
        self.buffer = BytesMut::new();
        if complete {
            Ok(())
        } else {
            Err(FrameError::Truncated)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoding::protobuf::write_frame;

    fn framed(body: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        write_frame(&mut bytes, body).unwrap();
        bytes
    }

    #[test]
    fn every_split_preserves_exact_frames_and_coalesced_following_data() {
        let body = vec![0xff; 257];
        let first = framed(&body);
        let second = framed(b"next frame");
        let joined = [first.clone(), second.clone()].concat();
        for split in 0..first.len() {
            let mut decoder = FrameDecoder::new(body.len());
            let mut prefix = &joined[..split];
            assert!(decoder.consume(&mut prefix).unwrap().is_none());
            assert!(prefix.is_empty());
            assert!(decoder.consume(&mut &[][..]).unwrap().is_none());
            let mut rest = &joined[split..];
            let frame = decoder.consume(&mut rest).unwrap().unwrap();
            assert_eq!(frame.body(), body);
            assert_eq!(frame.into_bytes(), first);
            assert_eq!(rest, second);
            let frame = decoder.consume(&mut rest).unwrap().unwrap();
            assert_eq!(frame.into_bytes(), second);
            assert!(rest.is_empty());
            assert!(decoder.is_at_boundary());
            decoder.finish().unwrap();
        }
    }

    #[test]
    fn noncanonical_prefixes_are_forwarded_without_reencoding() {
        let bytes = [0x81, 0, b'x'];
        let mut decoder = FrameDecoder::new(1);
        let mut input = bytes.as_slice();
        let frame = decoder.consume(&mut input).unwrap().unwrap();
        let body = frame.body();
        let encoded = frame.into_bytes();
        assert_eq!(encoded, bytes.as_slice());
        assert_eq!(body.as_ptr(), encoded[2..].as_ptr());
        assert_eq!(body, &b"x"[..]);
    }

    #[test]
    fn declared_lengths_never_allocate_a_body_and_fail_before_consuming_one() {
        let mut header = Vec::new();
        prost::encoding::encode_varint(32 * 1024 * 1024, &mut header);
        let mut decoder = FrameDecoder::new(32 * 1024 * 1024);
        assert!(decoder.consume(&mut header.as_slice()).unwrap().is_none());
        assert!(decoder.buffer.capacity() < super::super::frame::RESERVE);
        let mut decoder = FrameDecoder::new(16);
        let mut input = [header, vec![0; 128]].concat();
        let mut rest = input.as_slice();
        assert!(matches!(
            decoder.consume(&mut rest),
            Err(FrameError::TooLarge { .. })
        ));
        assert_eq!(rest.len(), 128);
        assert_eq!(decoder.buffer.capacity(), 0);
        assert!(!decoder.is_at_boundary());
        input.clear();
        assert!(decoder.consume(&mut input.as_slice()).is_err());
    }

    #[test]
    fn bytewise_input_empty_frames_and_terminal_errors_preserve_boundaries() {
        let body = b"private payload";
        let encoded = framed(body);
        let mut decoder = FrameDecoder::new(body.len());
        for (index, byte) in encoded.iter().enumerate() {
            let frame = decoder.consume(&mut std::slice::from_ref(byte)).unwrap();
            if index + 1 == encoded.len() {
                let frame = frame.unwrap();
                assert_eq!(frame.body(), &body[..]);
                assert!(!format!("{frame:?}").contains("private"));
            } else {
                assert!(frame.is_none());
                assert!(!decoder.is_at_boundary());
            }
        }
        let frame = decoder.consume(&mut &[0][..]).unwrap().unwrap();
        assert!(frame.body().is_empty());
        decoder.finish().unwrap();
        assert!(decoder.consume(&mut encoded.as_slice()).is_err());
        assert!(decoder.finish().is_err());

        for fragment in [&[0x80][..], &[2, 1][..]] {
            let mut decoder = FrameDecoder::new(16);
            assert!(decoder.consume(&mut &fragment[..]).unwrap().is_none());
            assert!(matches!(decoder.finish(), Err(FrameError::Truncated)));
            assert!(decoder.consume(&mut &[0][..]).is_err());
        }
        let mut decoder = FrameDecoder::new(usize::MAX);
        assert!(decoder.consume(&mut &[0xff; 10][..]).is_err());
        assert!(decoder.consume(&mut &[0][..]).is_err());
    }
}
