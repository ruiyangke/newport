//! Reject duplicate known fields and ambiguous operations before prost decode.
use super::*;
#[derive(Clone, Copy)]
enum Shape {
    Empty,
    Hello,
    Size,
    Colors,
    Computer,
    Attach,
    Data,
    Rename,
    Boolean,
    Exit,
    Error,
}
#[derive(Clone, Copy)]
enum Kind {
    Integer,
    Boolean,
    Bytes,
    Message(Shape),
}
fn scan(mut body: &[u8], shape: Shape) -> Result<(), Error> {
    use Kind::{Boolean as B, Bytes as S, Integer as I, Message as M};
    use Shape::*;
    let known: &[(u32, Kind)] = match shape {
        Empty => &[],
        Hello => &[(1, I), (2, S)],
        Size => &[(1, I), (2, I)],
        Colors => &[(1, I), (2, I), (3, I)],
        Computer => &[(1, S), (2, S)],
        Attach => &[
            (1, I),
            (2, M(Size)),
            (3, B),
            (4, B),
            (5, M(Computer)),
            (6, M(Colors)),
        ],
        Data | Rename => &[(1, S)],
        Boolean => &[(1, B)],
        Exit => &[(1, I), (2, I)],
        Error => &[(1, S), (2, S), (3, I)],
    };
    let mut seen = 0u64;
    while !body.is_empty() {
        let field = fields::field(&mut body)?;
        let Some((index, (_, kind))) = known
            .iter()
            .enumerate()
            .find(|(_, (number, _))| *number == field.number)
        else {
            continue;
        };
        let bit = 1u64 << index;
        if seen & bit != 0 || (matches!(shape, Exit) && seen != 0) {
            return Err(Invalid("duplicate field").into());
        }
        seen |= bit;
        match kind {
            I => {
                field.u32()?;
            }
            B => {
                if field.u64()? > 1 {
                    return Err(Invalid("boolean").into());
                }
            }
            S => {
                field.bytes()?;
            }
            M(shape) => scan(field.bytes()?, *shape)?,
        }
    }
    Ok(())
}
pub(super) fn client(body: &[u8]) -> Result<(), Error> {
    use Shape::*;
    let field = fields::single(body)?;
    let shape = match field.number {
        1 => Attach,
        2 => Data,
        3 => Size,
        4 | 6 => Empty,
        5 => Rename,
        _ => return Err(Invalid("client operation").into()),
    };
    scan(field.bytes()?, shape)
}
pub(super) fn server(body: &[u8]) -> Result<(), Error> {
    use Shape::*;
    let field = fields::single(body)?;
    let shape = match field.number {
        1 => Hello,
        2 | 7 => Boolean,
        3..=5 => Data,
        6 => Exit,
        8 => Error,
        9 => Rename,
        _ => return Err(Invalid("server operation").into()),
    };
    scan(field.bytes()?, shape)
}
