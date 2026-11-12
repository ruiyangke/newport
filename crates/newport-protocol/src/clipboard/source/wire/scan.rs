//! Bound repeated metadata and reject duplicate known fields before prost.
use super::*;
#[derive(Clone, Copy)]
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
fn scan(mut body: &[u8], shape: Shape) -> Result<(), Error> {
    use Kind::*;
    let fields: &[(u32, Kind)] = match shape {
        Shape::Empty => &[],
        Shape::Hello => &[(1, Unsigned)],
        Shape::Read => &[(1, Integer), (2, Bytes)],
        Shape::Offer => &[(1, Integer), (2, Formats)],
        Shape::Chunk => &[(1, Bytes), (2, Boolean)],
        Shape::Error => &[(1, Bytes), (2, Bytes), (3, Unsigned)],
    };
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
pub(super) fn request(body: &[u8]) -> Result<(), Error> {
    let field = fields::single(body)?;
    match field.number {
        1 => scan(field.bytes()?, Shape::Empty),
        2 => scan(field.bytes()?, Shape::Read),
        _ => Err(Invalid("request operation").into()),
    }
}
pub(super) fn response(body: &[u8]) -> Result<(), Error> {
    let field = fields::single(body)?;
    let shape = match field.number {
        1 => Shape::Hello,
        2 => Shape::Offer,
        3 => Shape::Chunk,
        4 => Shape::Error,
        _ => return Err(Invalid("response operation").into()),
    };
    scan(field.bytes()?, shape)
}
