//! Bound repeated metadata and reject duplicate known fields before prost.
use super::*;
#[derive(Clone, Copy)]
#[cfg_attr(test, derive(Debug))]
enum Shape {
    Empty,
    Hello,
    Read,
    Offer,
    Chunk,
    Error,
}
#[derive(Clone, Copy)]
enum Kind {
    Integer,
    Unsigned,
    Bytes,
    Formats,
    Boolean,
}
/// The fields `shape` knows, and how it reads them.
fn known(shape: Shape) -> &'static [(u32, Kind)] {
    use Kind::*;
    match shape {
        Shape::Empty => &[],
        Shape::Hello => &[(1, Unsigned)],
        Shape::Read => &[(1, Integer), (2, Bytes)],
        Shape::Offer => &[(1, Integer), (2, Formats)],
        Shape::Chunk => &[(1, Bytes), (2, Boolean)],
        Shape::Error => &[(1, Bytes), (2, Bytes), (3, Unsigned)],
    }
}
fn scan(mut body: &[u8], shape: Shape) -> Result<(), Error> {
    use Kind::*;
    let fields = known(shape);
    let mut seen = 0u8;
    let mut formats = 0;
    while !body.is_empty() {
        let field = fields::field(&mut body)?;
        let Some((index, (_, kind))) = fields
            .iter()
            .enumerate()
            .find(|(_, (number, _))| *number == field.number)
        else {
            continue;
        };
        if matches!(kind, Formats) {
            formats += 1;
            if formats > imports::MAX_FORMATS {
                return Err(Invalid("format count").into());
            }
        } else {
            let bit = 1u8 << index;
            if seen & bit != 0 {
                return Err(Invalid("duplicate field").into());
            }
            seen |= bit;
        }
        match kind {
            Integer => {
                field.u64()?;
            }
            Unsigned => {
                field.u32()?;
            }
            Bytes | Formats => {
                field.bytes()?;
            }
            Boolean => {
                if field.u64()? > 1 {
                    return Err(Invalid("boolean").into());
                }
            }
        }
    }
    Ok(())
}
fn request_shape(number: u32) -> Option<Shape> {
    Some(match number {
        1 => Shape::Empty,
        2 => Shape::Read,
        _ => return None,
    })
}
fn response_shape(number: u32) -> Option<Shape> {
    Some(match number {
        1 => Shape::Hello,
        2 => Shape::Offer,
        3 => Shape::Chunk,
        4 => Shape::Error,
        _ => return None,
    })
}
pub(super) fn request(body: &[u8]) -> Result<(), Error> {
    let field = fields::single(body)?;
    let shape = request_shape(field.number).ok_or(Invalid("request operation"))?;
    scan(field.bytes()?, shape)
}
pub(super) fn response(body: &[u8]) -> Result<(), Error> {
    let field = fields::single(body)?;
    let shape = response_shape(field.number).ok_or(Invalid("response operation"))?;
    scan(field.bytes()?, shape)
}

#[cfg(test)]
pub(crate) fn tables() -> Vec<(Table, &'static str)> {
    let shape = |number| request_shape(number).map(table);
    let request = Table::dispatch("clipboard source request", shape);
    let shape = |number| response_shape(number).map(table);
    let response = Table::dispatch("clipboard source response", shape);
    vec![
        (request, "newport.clipboard.source.ClientFrame"),
        (response, "newport.clipboard.source.ServerFrame"),
    ]
}

#[cfg(test)]
use crate::desktop::wire::wire_tables::{Row, Table, Wire};

#[cfg(test)]
fn table(shape: Shape) -> Table {
    Table::new(format!("clipboard source {shape:?}"), move || {
        known(shape)
            .iter()
            .map(|&(number, kind)| {
                let wire = match kind {
                    Kind::Integer => Wire::Varint,
                    Kind::Unsigned => Wire::Varint32,
                    Kind::Bytes | Kind::Formats => Wire::Len,
                    Kind::Boolean => Wire::Bool,
                };
                Row::new(number, wire).repeated(matches!(kind, Kind::Formats))
            })
            .collect()
    })
}
