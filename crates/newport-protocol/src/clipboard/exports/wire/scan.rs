use super::*;
use crate::encoding::protobuf::fields;

#[derive(Clone, Copy)]
enum Shape {
    Control,
    Event,
    Offer,
    Origin,
    Copy,
    Chunk,
    Enable,
    Empty,
    Read,
    Cancel,
    Commit,
    Release,
    ReadCopy,
    Offered,
    Data,
    Committed,
    Revoked,
}
enum Value {
    Message(Shape),
    Bytes(usize),
    Integer,
    Boolean,
    Version,
}

pub(crate) fn control(body: &[u8]) -> Result<(), Error> {
    check(body, Shape::Control)
}
pub(crate) fn event(body: &[u8]) -> Result<(), Error> {
    check(body, Shape::Event)
}
fn check(body: &[u8], shape: Shape) -> Result<(), Error> {
    if body.len() > MAX_FRAME {
        return Err(FrameError::TooLarge {
            length: body.len() as u64,
            limit: MAX_FRAME,
        }
        .into());
    }
    let mut remaining = MAX_FIELDS;
    scan(body, shape, &mut remaining)
}
fn scan(mut body: &[u8], shape: Shape, remaining: &mut usize) -> Result<(), Error> {
    let mut counts = [0usize; 8];
    while !body.is_empty() {
        *remaining = remaining.checked_sub(1).ok_or(Invalid)?;
        let field = fields::field(&mut body)?;
        let number = field.number;
        let value = match (shape, number) {
            (Shape::Control, 1..=7) => {
                counts[0] += 1;
                if counts[0] > 1 {
                    return Err(Invalid.into());
                }
                Value::Message(match number {
                    1 => Shape::Enable,
                    2 => Shape::Empty,
                    3 => Shape::Read,
                    4 => Shape::Cancel,
                    5 => Shape::Commit,
                    6 => Shape::Release,
                    _ => Shape::ReadCopy,
                })
            }
            (Shape::Event, 1..=4) => {
                counts[0] += 1;
                if counts[0] > 1 {
                    return Err(Invalid.into());
                }
                Value::Message(match number {
                    1 => Shape::Offered,
                    2 => Shape::Data,
                    3 => Shape::Committed,
                    _ => Shape::Revoked,
                })
            }
            (Shape::Offer, 1) | (Shape::Copy, 1) | (Shape::Origin, 1) | (Shape::Revoked, 1) => {
                Value::Bytes(MAX_ID_BYTES)
            }
            (Shape::Offer, 3) => Value::Message(Shape::Origin),
            (Shape::Offer, 4) | (Shape::Read | Shape::ReadCopy, 3) => {
                Value::Bytes(MAX_FORMAT_BYTES)
            }
            (Shape::Copy | Shape::Read | Shape::Commit, 2) | (Shape::Offered, 1) => {
                Value::Message(Shape::Offer)
            }
            (Shape::ReadCopy | Shape::Committed, 2) | (Shape::Release, 1) => {
                Value::Message(Shape::Copy)
            }
            (Shape::Chunk, 1) => Value::Bytes(CHUNK),
            (Shape::Chunk, 2) => Value::Boolean,
            (Shape::Data, 3) => Value::Message(Shape::Chunk),
            (Shape::Enable, 1) => Value::Version,
            (Shape::Offer, 2 | 5)
            | (Shape::Origin, 2)
            | (
                Shape::Read | Shape::Cancel | Shape::Commit | Shape::ReadCopy | Shape::Committed,
                1,
            )
            | (Shape::Data, 1 | 2) => Value::Integer,
            _ => continue,
        };
        counts[number as usize] += 1;
        let max = if matches!((shape, number), (Shape::Offer, 4)) {
            MAX_FORMATS
        } else {
            1
        };
        if counts[number as usize] > max {
            return Err(Invalid.into());
        }
        match value {
            Value::Message(child) => scan(field.bytes()?, child, remaining)?,
            Value::Bytes(limit) => {
                if field.bytes()?.len() > limit {
                    return Err(Invalid.into());
                }
            }
            Value::Integer => {
                field.u64()?;
            }
            Value::Boolean => {
                if field.u32()? > 1 {
                    return Err(Invalid.into());
                }
            }
            Value::Version => {
                field.u32()?;
            }
        }
    }
    Ok(())
}
