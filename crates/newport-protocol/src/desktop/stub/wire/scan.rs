//! Checks known fields before prost allocation, with shape derived from descriptors.
use super::*;
use crate::encoding::protobuf::fields;
#[allow(dead_code)]
#[derive(Clone, Copy)]
enum Kind {
    Integer {
        maximum: u64,
        negative_minimum: Option<u64>,
    },
    Boolean,
    Fixed32,
    Fixed64,
    Text,
    Data,
    Enum(&'static [u64]),
    Message(usize),
    Map(usize),
}
struct Rule {
    tag: u32,
    name: &'static str,
    repeated: bool,
    group: Option<usize>,
    kind: Kind,
}
struct Schema {
    name: &'static str,
    fields: &'static [Rule],
}
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum MapKey<'a> {
    Text(&'a [u8]),
    Integer(u64),
}
const MAX_FIELDS: usize = MAX_MESSAGE_BYTES / 2;
include!(concat!(env!("OUT_DIR"), "/stub_fields.rs"));
pub(super) fn check(body: &[u8], shape: Shape) -> Result<(), Error> {
    let mut remaining = MAX_FIELDS;
    walk(body, shape.schema(), 0, &mut remaining)
}
fn walk(mut body: &[u8], id: usize, depth: u32, remaining: &mut usize) -> Result<(), Error> {
    if depth > protobuf::MAX_DEPTH {
        return Err(FrameError::TooDeep.into());
    }
    let schema = schema(id).ok_or(Invalid("field schema"))?;
    let mut counts = [0usize; 64];
    let mut groups = [false; 64];
    let mut keys = std::collections::BTreeSet::new();
    while !body.is_empty() {
        *remaining = remaining
            .checked_sub(1)
            .ok_or(Invalid("encoded field budget"))?;
        let wire = body[0] & 7;
        let field = fields::field(&mut body)?;
        let Some((index, rule)) = schema
            .fields
            .iter()
            .enumerate()
            .find(|(_, rule)| rule.tag == field.number)
        else {
            continue;
        };
        let limit = if rule.repeated {
            count_limit(schema.name, rule.name)
        } else {
            1
        };
        let mut count = 1usize;
        match rule.kind {
            Kind::Message(child) => walk(field.bytes()?, child, depth + 1, remaining)?,
            Kind::Map(child) => {
                let bytes = field.bytes()?;
                walk(bytes, child, depth + 1, remaining)?;
                let key = map_key(bytes)?;
                if !keys.insert((rule.tag, key)) {
                    return Err(Invalid("duplicate map key").into());
                }
            }
            Kind::Text | Kind::Data => {
                field.bytes()?;
            }
            Kind::Fixed64 => {
                if !rule.repeated && wire != 1 {
                    return Err(FrameError::Malformed.into());
                }
                count = field.fixed64_count()?;
            }
            Kind::Fixed32 => {
                if wire == 2 && rule.repeated {
                    let value = field.bytes()?;
                    if !value.len().is_multiple_of(4) {
                        return Err(FrameError::Malformed.into());
                    }
                    count = value.len() / 4;
                } else if wire != 5 {
                    return Err(FrameError::Malformed.into());
                }
            }
            kind => {
                if wire == 2 && rule.repeated {
                    let mut packed = field.bytes()?;
                    count = 0;
                    while !packed.is_empty() {
                        number(varint(&mut packed)?, kind)?;
                        count = count.checked_add(1).ok_or(Invalid("collection count"))?;
                        if count > limit {
                            return Err(Invalid("collection count").into());
                        }
                    }
                } else {
                    number(field.u64()?, kind)?;
                }
            }
        }
        let current = counts.get_mut(index).ok_or(Invalid("field schema"))?;
        *current = current
            .checked_add(count)
            .ok_or(Invalid("collection count"))?;
        if *current > limit {
            return Err(Invalid("duplicate field or collection count").into());
        }
        if let Some(group) = rule.group {
            let selected = groups.get_mut(group).ok_or(Invalid("oneof schema"))?;
            if *selected {
                return Err(Invalid("duplicate oneof").into());
            }
            *selected = true;
        }
    }
    Ok(())
}
fn map_key(mut body: &[u8]) -> Result<MapKey<'_>, Error> {
    while !body.is_empty() {
        let wire = body[0] & 7;
        let field = fields::field(&mut body)?;
        if field.number == 1 {
            return Ok(if wire == 2 {
                MapKey::Text(field.bytes()?)
            } else {
                MapKey::Integer(field.u64()?)
            });
        }
    }
    Err(Invalid("map key").into())
}
fn number(value: u64, kind: Kind) -> Result<(), Error> {
    match kind {
        Kind::Boolean if value > 1 => Err(Invalid("boolean value").into()),
        Kind::Integer {
            maximum,
            negative_minimum,
        } if value > maximum && negative_minimum.is_none_or(|minimum| value < minimum) => {
            Err(Invalid("integer range").into())
        }
        Kind::Enum(values) if !values.contains(&value) => Err(Invalid("enumeration").into()),
        Kind::Integer { .. } | Kind::Boolean | Kind::Enum(_) => Ok(()),
        Kind::Fixed32
        | Kind::Fixed64
        | Kind::Text
        | Kind::Data
        | Kind::Message(_)
        | Kind::Map(_) => Err(FrameError::Malformed.into()),
    }
}
fn varint(body: &mut &[u8]) -> Result<u64, Error> {
    let mut value = 0;
    for shift in 0..10 {
        let (&byte, rest) = body.split_first().ok_or(FrameError::Malformed)?;
        *body = rest;
        if shift == 9 && byte > 1 {
            return Err(FrameError::Malformed.into());
        }
        value |= u64::from(byte & 127) << (shift * 7);
        if byte & 128 == 0 {
            return Ok(value);
        }
    }
    Err(FrameError::Malformed.into())
}
fn count_limit(_message: &str, _field: &str) -> usize {
    1
}
