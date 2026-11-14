//! Guard ambiguity and repeated-field counts before generated decoding.
use super::*;
#[derive(Clone, Copy)]
#[cfg_attr(test, derive(Debug))]
enum Shape {
    Empty,
    Access,
    Secret,
    Offer,
    NativeOrigin,
    Lease,
    NativeRequest,
    Provider,
    Read,
    Enabled,
    Publish,
    ImportRequest,
    ImportReply,
    NativePublish,
    NativeReply,
    ExportOffer,
    CopyLease,
    Chunk,
    Requests,
    Export,
    Copy,
    Hello,
    Error,
}
#[derive(Clone, Copy)]
enum Kind {
    Integer,
    Integer32,
    Boolean,
    Bytes,
    Message(Shape),
}
use Kind::{Boolean as B, Bytes as S, Integer as I, Integer32 as U, Message as M};
/// The fields `shape` knows: number, most occurrences, and how it reads them.
fn known(shape: Shape) -> &'static [(u32, u16, Kind)] {
    use Shape::*;
    match shape {
        Empty => &[],
        Access => &[(1, 1, S), (2, 1, S), (3, 1, S), (4, 1, M(Secret))],
        Secret => &[(1, 1, S)],
        Offer => &[
            (1, 1, S),
            (2, 1, I),
            (3, 1, M(Empty)),
            (4, 1, M(Empty)),
            (5, 1, M(NativeOrigin)),
            (6, 256, S),
        ],
        NativeOrigin => &[(1, 1, S), (2, 1, I)],
        Lease => &[(1, 1, S), (2, 1, S), (3, 1, I)],
        NativeRequest => &[(1, 1, M(Lease)), (2, 1, I), (3, 1, I), (4, 1, S)],
        Provider => &[(1, 1, M(Lease)), (2, 1, M(Offer)), (3, 8, M(NativeRequest))],
        Read => &[
            (1, 1, S),
            (2, 1, I),
            (3, 1, S),
            (4, 1, I),
            (5, 1, U),
            (6, 1, M(ExportOffer)),
            (7, 1, M(CopyLease)),
        ],
        Enabled => &[(1, 1, B)],
        Publish => &[(1, 1, I), (2, 256, S)],
        ImportRequest => &[(1, 1, I), (2, 1, I), (3, 1, S)],
        ImportReply => &[(1, 1, I), (2, 1, I), (3, 1, U), (4, 1, B), (5, 1, S)],
        NativePublish => &[
            (1, 1, M(Lease)),
            (2, 1, S),
            (3, 1, I),
            (4, 1, I),
            (5, 256, S),
        ],
        NativeReply => &[(1, 1, M(NativeRequest)), (2, 1, M(Chunk))],
        ExportOffer => &[
            (1, 1, S),
            (2, 1, I),
            (3, 1, M(NativeOrigin)),
            (4, 256, S),
            (5, 1, I),
        ],
        CopyLease => &[(1, 1, S), (2, 1, M(ExportOffer))],
        Chunk => &[(1, 1, S), (2, 1, B)],
        Requests => &[(1, 8, M(ImportRequest))],
        Export => &[(1, 1, M(ExportOffer))],
        Copy => &[(1, 1, M(CopyLease))],
        Hello => &[(1, 1, U), (2, 1, S)],
        Error => &[(1, 1, S), (2, 1, S), (3, 1, U)],
    }
}
/// Whether field `number` is one of `shape`'s exclusive cases.
fn exclusive(shape: Shape, number: u32) -> bool {
    use Shape::*;
    matches!((shape, number), (Offer, 3..=5) | (Read, 6..=7))
}
fn scan(mut body: &[u8], shape: Shape) -> Result<(), Error> {
    let fields = known(shape);
    let mut counts = [0u16; 7];
    let mut oneof = false;
    while !body.is_empty() {
        let field = fields::field(&mut body)?;
        let Some((index, (_, max, kind))) = fields
            .iter()
            .enumerate()
            .find(|(_, (number, _, _))| *number == field.number)
        else {
            continue;
        };
        counts[index] += 1;
        if counts[index] > *max {
            return Err(Invalid("duplicate field or repeated-field limit").into());
        }
        if exclusive(shape, field.number) {
            if oneof {
                return Err(Invalid("ambiguous field").into());
            }
            oneof = true;
        }
        match kind {
            I => {
                field.u64()?;
            }
            U => {
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
fn request_shape(number: u32) -> Option<Shape> {
    use Shape::*;
    Some(match number {
        1 => Access,
        2 | 4 | 7 | 9 | 12 | 14 => Empty,
        3 => Read,
        5 => Enabled,
        6 => Publish,
        8 => ImportReply,
        10 => NativePublish,
        11 => NativeReply,
        13 => ExportOffer,
        15 => CopyLease,
        _ => return None,
    })
}
fn response_shape(number: u32) -> Option<Shape> {
    use Shape::*;
    Some(match number {
        1 => Hello,
        2 => Empty,
        3 => Offer,
        4 => Chunk,
        5 => Requests,
        6 => Provider,
        7 => Enabled,
        8 => Export,
        9 => Copy,
        10 => Error,
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
    let request = Table::dispatch("clipboard service request", shape);
    let shape = |number| response_shape(number).map(table);
    let response = Table::dispatch("clipboard service response", shape);
    vec![
        (request, "newport.clipboard.service.ClientFrame"),
        (response, "newport.clipboard.service.ServerFrame"),
    ]
}

#[cfg(test)]
use crate::desktop::wire::wire_tables::{Row, Table, Wire};

#[cfg(test)]
fn table(shape: Shape) -> Table {
    Table::new(format!("clipboard service {shape:?}"), move || {
        known(shape)
            .iter()
            .map(|&(number, most, kind)| {
                let wire = match kind {
                    I => Wire::Varint,
                    U => Wire::Varint32,
                    B => Wire::Bool,
                    S => Wire::Len,
                    M(child) => Wire::Message(table(child)),
                };
                Row::new(number, wire)
                    .repeated(most > 1)
                    .oneof(exclusive(shape, number))
            })
            .collect()
    })
}
