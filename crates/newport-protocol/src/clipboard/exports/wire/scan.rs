use super::*;
use crate::encoding::protobuf::fields;

#[derive(Clone, Copy)]
#[cfg_attr(test, derive(Debug))]
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
/// How `shape` reads field `number`, and whether it is one of the shape's
/// exclusive cases.
fn rule(shape: Shape, number: u32) -> Option<(Value, bool)> {
    Some(match (shape, number) {
        (Shape::Control, 1..=7) => (
            Value::Message(match number {
                1 => Shape::Enable,
                2 => Shape::Empty,
                3 => Shape::Read,
                4 => Shape::Cancel,
                5 => Shape::Commit,
                6 => Shape::Release,
                _ => Shape::ReadCopy,
            }),
            true,
        ),
        (Shape::Event, 1..=4) => (
            Value::Message(match number {
                1 => Shape::Offered,
                2 => Shape::Data,
                3 => Shape::Committed,
                _ => Shape::Revoked,
            }),
            true,
        ),
        (Shape::Offer, 1) | (Shape::Copy, 1) | (Shape::Origin, 1) | (Shape::Revoked, 1) => {
            (Value::Bytes(MAX_ID_BYTES), false)
        }
        (Shape::Offer, 3) => (Value::Message(Shape::Origin), false),
        (Shape::Offer, 4) | (Shape::Read | Shape::ReadCopy, 3) => {
            (Value::Bytes(MAX_FORMAT_BYTES), false)
        }
        (Shape::Copy | Shape::Read | Shape::Commit, 2) | (Shape::Offered, 1) => {
            (Value::Message(Shape::Offer), false)
        }
        (Shape::ReadCopy | Shape::Committed, 2) | (Shape::Release, 1) => {
            (Value::Message(Shape::Copy), false)
        }
        (Shape::Chunk, 1) => (Value::Bytes(CHUNK), false),
        (Shape::Chunk, 2) => (Value::Boolean, false),
        (Shape::Data, 3) => (Value::Message(Shape::Chunk), false),
        (Shape::Enable, 1) => (Value::Version, false),
        (Shape::Offer, 2 | 5)
        | (Shape::Origin, 2)
        | (Shape::Read | Shape::Cancel | Shape::Commit | Shape::ReadCopy | Shape::Committed, 1)
        | (Shape::Data, 1 | 2) => (Value::Integer, false),
        _ => return None,
    })
}
fn repeated(shape: Shape, number: u32) -> bool {
    matches!((shape, number), (Shape::Offer, 4))
}
fn scan(mut body: &[u8], shape: Shape, remaining: &mut usize) -> Result<(), Error> {
    let mut counts = [0usize; 8];
    while !body.is_empty() {
        *remaining = remaining.checked_sub(1).ok_or(Invalid)?;
        let field = fields::field(&mut body)?;
        let number = field.number;
        let Some((value, case)) = rule(shape, number) else {
            continue;
        };
        if case {
            counts[0] += 1;
            if counts[0] > 1 {
                return Err(Invalid.into());
            }
        }
        counts[number as usize] += 1;
        let max = if repeated(shape, number) {
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

#[cfg(test)]
pub(crate) fn tables() -> Vec<(Table, &'static str)> {
    vec![
        (control_table(), "newport.clipboard.ExportControl"),
        (event_table(), "newport.clipboard.ExportEvent"),
    ]
}

#[cfg(test)]
#[track_caller]
pub(crate) fn control_table() -> Table {
    table(Shape::Control)
}

#[cfg(test)]
#[track_caller]
pub(crate) fn event_table() -> Table {
    table(Shape::Event)
}

#[cfg(test)]
use crate::desktop::wire::wire_tables::{Row, Table, Wire, numbers};

#[cfg(test)]
#[track_caller]
fn table(shape: Shape) -> Table {
    Table::new(format!("clipboard exports {shape:?}"), move || {
        numbers()
            .filter_map(|number| {
                let (value, case) = rule(shape, number)?;
                let wire = match value {
                    Value::Message(child) => Wire::Message(table(child)),
                    Value::Bytes(_) => Wire::Len,
                    Value::Integer => Wire::Varint,
                    Value::Boolean => Wire::Bool,
                    Value::Version => Wire::Varint32,
                };
                Some(
                    Row::new(number, wire)
                        .repeated(repeated(shape, number))
                        .oneof(case),
                )
            })
            .collect()
    })
}
