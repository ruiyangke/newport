use super::*;
use crate::encoding::protobuf::fields;

const MAX_FIELDS: usize = 2048;
#[derive(Clone, Copy)]
#[cfg_attr(test, derive(Debug))]
enum Shape {
    Client,
    Server,
    Call,
    Response,
    Offer,
    ClipboardRequest,
    ClipboardReply,
    BrowserRequest,
    BrowserReply,
    Snapshot,
    Ack,
    Permissions,
    Applied,
    Error,
    Empty,
}
enum Value {
    Child(Shape),
    Bytes(usize),
    Integer,
    Bool,
    Enum,
    ExportControl,
    ExportEvent,
}
pub(super) fn client(body: &[u8]) -> Result<(), Error> {
    let mut remaining = MAX_FIELDS;
    scan(body, Shape::Client, &mut remaining)
}
pub(super) fn server(body: &[u8]) -> Result<(), Error> {
    let mut remaining = MAX_FIELDS;
    scan(body, Shape::Server, &mut remaining)
}
/// How `shape` reads field `number`, and whether it is one of the shape's
/// exclusive cases.
fn rule(shape: Shape, number: u32) -> Option<(Value, bool)> {
    Some(match (shape, number) {
        (Shape::Client, 1..=4) => (
            Value::Child(match number {
                1 => Shape::Call,
                2 => Shape::ClipboardReply,
                3 => Shape::BrowserReply,
                _ => Shape::Empty,
            }),
            true,
        ),
        (Shape::Server, 1..=6) => (
            match number {
                1 => Value::Child(Shape::Response),
                2 => Value::Child(Shape::ClipboardRequest),
                3 => Value::Child(Shape::BrowserRequest),
                4 => Value::Child(Shape::Empty),
                5 => Value::ExportEvent,
                _ => Value::Child(Shape::Error),
            },
            true,
        ),
        (Shape::Call, 2..=7) => (
            match number {
                2 => Value::Child(Shape::Permissions),
                3 => Value::Child(Shape::Offer),
                4 => Value::Child(Shape::Snapshot),
                5 | 6 => Value::Child(Shape::Empty),
                _ => Value::ExportControl,
            },
            true,
        ),
        (Shape::Response, 2..=4) => (
            Value::Child(match number {
                2 => Shape::Ack,
                3 => Shape::Applied,
                _ => Shape::Error,
            }),
            true,
        ),
        (
            Shape::Call
            | Shape::Response
            | Shape::Offer
            | Shape::Permissions
            | Shape::BrowserRequest
            | Shape::BrowserReply,
            1,
        )
        | (Shape::ClipboardRequest | Shape::ClipboardReply, 1 | 2) => (Value::Integer, false),
        (Shape::Offer, 2) | (Shape::ClipboardRequest, 3) => {
            (Value::Bytes(imports::MAX_FORMAT_BYTES), false)
        }
        (Shape::ClipboardReply, 3) | (Shape::Error, 3) => (Value::Enum, false),
        (Shape::ClipboardReply, 4) | (Shape::BrowserReply, 2) | (Shape::Permissions, 2 | 3) => {
            (Value::Bool, false)
        }
        (Shape::ClipboardReply, 5) => (Value::Bytes(imports::CHUNK), false),
        (Shape::BrowserRequest, 2)
        | (Shape::BrowserReply, 3)
        | (Shape::Ack, 1)
        | (Shape::Error, 2) => (Value::Bytes(MAX_TEXT), false),
        (Shape::Snapshot, 1) => (Value::Bytes(MAX_ARCHIVE), false),
        (Shape::Error, 1) => (Value::Bytes(64), false),
        (Shape::Applied, 1) => (Value::Child(Shape::Permissions), false),
        (Shape::Applied, 2) => (Value::Child(Shape::Error), false),
        _ => return None,
    })
}
fn repeated(shape: Shape, number: u32) -> bool {
    matches!((shape, number), (Shape::Offer, 2))
}
fn scan(mut body: &[u8], shape: Shape, remaining: &mut usize) -> Result<(), Error> {
    let mut counts = [0usize; 8];
    let mut cases = 0;
    while !body.is_empty() {
        *remaining = remaining.checked_sub(1).ok_or(Invalid("field budget"))?;
        let field = fields::field(&mut body)?;
        let number = field.number;
        let Some((value, case)) = rule(shape, number) else {
            continue;
        };
        if case {
            cases += 1;
        }
        if cases > 1 {
            return Err(Invalid("ambiguous operation").into());
        }
        counts[number as usize] += 1;
        let max = if repeated(shape, number) {
            imports::MAX_FORMATS
        } else {
            1
        };
        if counts[number as usize] > max {
            return Err(Invalid("repeated field").into());
        }
        match value {
            Value::Child(child) => scan(field.bytes()?, child, remaining)?,
            Value::Bytes(limit) => {
                if field.bytes()?.len() > limit {
                    return Err(Invalid("field size").into());
                }
            }
            Value::Integer => {
                field.u64()?;
            }
            Value::Bool => {
                if field.u32()? > 1 {
                    return Err(Invalid("boolean").into());
                }
            }
            Value::Enum => {
                if field.u32()? > 2 {
                    return Err(Invalid("enumeration").into());
                }
            }
            Value::ExportControl => export::scan::control(field.bytes()?)?,
            Value::ExportEvent => export::scan::event(field.bytes()?)?,
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn tables() -> Vec<(Table, &'static str)> {
    vec![
        (table(Shape::Client), "newport.integration.ClientFrame"),
        (table(Shape::Server), "newport.integration.ServerFrame"),
    ]
}

#[cfg(test)]
use crate::desktop::wire::wire_tables::{Row, Table, Wire, numbers};

#[cfg(test)]
#[track_caller]
fn table(shape: Shape) -> Table {
    Table::new(format!("integration {shape:?}"), move || {
        numbers()
            .filter_map(|number| {
                let (value, case) = rule(shape, number)?;
                let wire = match value {
                    Value::Child(child) => Wire::Message(table(child)),
                    Value::Bytes(_) => Wire::Len,
                    Value::Integer => Wire::Varint,
                    Value::Bool => Wire::Bool,
                    Value::Enum => Wire::Enum { min: 0, max: 2 },
                    Value::ExportControl => Wire::Message(export::scan::control_table()),
                    Value::ExportEvent => Wire::Message(export::scan::event_table()),
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
