//! Reject duplicate known fields and ambiguous operations before prost decode.
use super::*;
#[derive(Clone, Copy)]
#[cfg_attr(test, derive(Debug))]
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
/// The fields `shape` knows, and how it reads them.
fn known(shape: Shape) -> &'static [(u32, Kind)] {
    use Kind::{Boolean as B, Bytes as S, Integer as I, Message as M};
    use Shape::*;
    match shape {
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
    }
}
/// An exit is one oneof: its fields exclude each other.
fn exclusive(shape: Shape) -> bool {
    matches!(shape, Shape::Exit)
}
fn scan(mut body: &[u8], shape: Shape) -> Result<(), Error> {
    use Kind::{Boolean as B, Bytes as S, Integer as I, Message as M};
    let known = known(shape);
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
        if seen & bit != 0 || (exclusive(shape) && seen != 0) {
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
fn client_shape(number: u32) -> Option<Shape> {
    use Shape::*;
    Some(match number {
        1 => Attach,
        2 => Data,
        3 => Size,
        4 | 6 => Empty,
        5 => Rename,
        _ => return None,
    })
}
fn server_shape(number: u32) -> Option<Shape> {
    use Shape::*;
    Some(match number {
        1 => Hello,
        2 | 7 => Boolean,
        3..=5 => Data,
        6 => Exit,
        8 => Error,
        9 => Rename,
        10 => Size,
        _ => return None,
    })
}
pub(super) fn client(body: &[u8]) -> Result<(), Error> {
    let field = fields::single(body)?;
    let shape = client_shape(field.number).ok_or(Invalid("client operation"))?;
    scan(field.bytes()?, shape)
}
pub(super) fn server(body: &[u8]) -> Result<(), Error> {
    let field = fields::single(body)?;
    let shape = server_shape(field.number).ok_or(Invalid("server operation"))?;
    scan(field.bytes()?, shape)
}

#[cfg(test)]
use crate::desktop::wire::wire_tables::{Row, Table, Wire};

#[cfg(test)]
pub(crate) fn tables() -> Vec<(Table, &'static str)> {
    let client = |number| client_shape(number).map(table);
    let server = |number| server_shape(number).map(table);
    vec![
        (
            Table::dispatch("terminal holder client", client),
            "newport.terminal.holder.ClientFrame",
        ),
        (
            Table::dispatch("terminal holder server", server),
            "newport.terminal.holder.ServerFrame",
        ),
    ]
}

#[cfg(test)]
fn table(shape: Shape) -> Table {
    Table::new(format!("terminal holder {shape:?}"), move || {
        known(shape)
            .iter()
            .map(|&(number, kind)| {
                let wire = match kind {
                    Kind::Integer => Wire::Varint32,
                    Kind::Boolean => Wire::Bool,
                    Kind::Bytes => Wire::Len,
                    Kind::Message(child) => Wire::Message(table(child)),
                };
                Row::new(number, wire).oneof(exclusive(shape))
            })
            .collect()
    })
}
