//! Guard ambiguity and repeated-field counts before generated decoding.
use super::*;
#[derive(Clone, Copy)]
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
fn scan(mut body: &[u8], shape: Shape) -> Result<(), Error> {
    use Shape::*;
    let fields: &[(u32, u16, Kind)] = match shape {
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
    };
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
        if matches!((shape, field.number), (Offer, 3..=5) | (Read, 6..=7)) {
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
pub(super) fn request(body: &[u8]) -> Result<(), Error> {
    use Shape::*;
    let field = fields::single(body)?;
    let shape = match field.number {
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
        _ => return Err(Invalid("request operation").into()),
    };
    scan(field.bytes()?, shape)
}
pub(super) fn response(body: &[u8]) -> Result<(), Error> {
    use Shape::*;
    let field = fields::single(body)?;
    let shape = match field.number {
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
        _ => return Err(Invalid("response operation").into()),
    };
    scan(field.bytes()?, shape)
}
