use crate::encoding::protobuf::{FrameError, MAX_DEPTH};

/// Where a message's `bytes` fields are, directly or in the messages it
/// holds. Generated for every message by the `build` feature's code
/// generation.
pub trait Layout {
    /// The fields that are `bytes` or messages holding them; any other field
    /// counts as metadata.
    fn fields() -> &'static [Field];
}

/// One entry of a [`Layout`].
#[derive(Clone, Copy, Debug)]
pub struct Field {
    number: u32,
    kind: Kind,
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    Bytes,
    Message(fn() -> &'static [Field]),
}

impl Field {
    /// A `bytes` field.
    pub const fn bytes(number: u32) -> Self {
        Self {
            number,
            kind: Kind::Bytes,
        }
    }

    /// A message field, repeated or not, or a map whose values hold bytes.
    pub const fn message(number: u32, fields: fn() -> &'static [Field]) -> Self {
        Self {
            number,
            kind: Kind::Message(fields),
        }
    }
}

/// Refuses `frame` if everything outside the contents of its `bytes` fields
/// is over `limit`, or its messages nest too deep.
///
/// It walks the wire format without decoding. Fields it doesn't know, and
/// anything it can't parse, count as metadata; the decoder then skips or
/// rejects them.
pub(crate) fn check_metadata(
    frame: &[u8],
    fields: &[Field],
    limit: usize,
) -> Result<(), FrameError> {
    let length = frame.len() - bytes_len(frame, fields, 0)?;
    if length > limit {
        return Err(FrameError::MetadataTooLarge { length, limit });
    }
    Ok(())
}

fn bytes_len(mut buf: &[u8], fields: &[Field], depth: u32) -> Result<usize, FrameError> {
    let mut total = 0;
    while let Some(key) = varint(&mut buf) {
        let skip = match key & 0b111 {
            0 => varint(&mut buf).map(|_| 0),
            1 => Some(8),
            5 => Some(4),
            2 => varint(&mut buf).and_then(|len| usize::try_from(len).ok()),
            // Groups never appear in proto3.
            _ => None,
        };
        let Some(len) = skip.filter(|&len| len <= buf.len()) else {
            break;
        };
        let (body, rest) = buf.split_at(len);
        buf = rest;
        if key & 0b111 != 2 {
            continue;
        }
        let number = key >> 3;
        match fields
            .iter()
            .find(|field| u64::from(field.number) == number)
        {
            Some(Field {
                kind: Kind::Bytes, ..
            }) => total += len,
            Some(Field {
                kind: Kind::Message(nested),
                ..
            }) => {
                if depth + 1 > MAX_DEPTH {
                    return Err(FrameError::TooDeep);
                }
                total += bytes_len(body, nested(), depth + 1)?;
            }
            None => {}
        }
    }
    Ok(total)
}

fn varint(buf: &mut &[u8]) -> Option<u64> {
    let mut value = 0u64;
    for (i, &byte) in buf.iter().enumerate().take(10) {
        // A tenth byte holds only the top bit of a u64.
        if i == 9 && byte > 1 {
            return None;
        }
        value |= u64::from(byte & 0x7f) << (7 * i);
        if byte & 0x80 == 0 {
            *buf = &buf[i + 1..];
            return Some(value);
        }
    }
    None
}
