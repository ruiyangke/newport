//! Checks known fields before prost allocation, with shape derived from descriptors.
use super::*;
use crate::encoding::protobuf::fields;
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
const MAX_FIELDS: usize = 262_144;
include!(concat!(env!("OUT_DIR"), "/desktop_fields.rs"));
pub(super) fn check(body: &[u8], shape: Shape) -> Result<(), Error> {
    let mut remaining = MAX_FIELDS;
    walk(body, shape.schema(), 0, &mut remaining)
}
fn walk(mut body: &[u8], id: usize, depth: u32, remaining: &mut usize) -> Result<(), Error> {
    if depth > protobuf::MAX_DEPTH {
        return Err(FrameError::TooDeep.into());
    }
    let schema = schema(id).ok_or(Invalid("field schema"))?;
    if discovery(schema.name, body, remaining)? {
        return Ok(());
    }
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
fn discovery(message: &str, body: &[u8], remaining: &mut usize) -> Result<bool, Error> {
    use crate::monitor::wire::scan;
    let fields = match message {
        ".newport.desktop.connection.Discovery" => {
            if body.len() > crate::desktop::connection::discovery::MAX_BYTES {
                return Err(Invalid("discovery encoding budget").into());
            }
            discovery_allocations(body)?;
            return Ok(false);
        }
        ".newport.discovery.Snapshot" => {
            if body.len() > crate::monitor::SERVER_LIMITS.message {
                return Err(Invalid("discovery snapshot encoding budget").into());
            }
            scan::measure_discovery(body)?
        }
        ".newport.discovery.Report" => scan::measure_discovery_report(body)?,
        _ => return Ok(false),
    };
    *remaining = remaining
        .checked_sub(fields)
        .ok_or(Invalid("encoded field budget"))?;
    Ok(true)
}

fn discovery_allocations(mut body: &[u8]) -> Result<(), Error> {
    use crate::{desktop::connection::discovery as limits, monitor::wire::scan};
    let mut counts = [0usize; 5];
    // Reserve the wrapper and bounded retention diagnostics, just as the writer
    // does. Reject aggregate counts before prost allocates the retained scans.
    let mut total = limits::ENVELOPE_FIELDS;
    let mut wrappers = 0usize;
    let mut snapshots = [0usize; 2];
    while !body.is_empty() {
        wrappers += 1;
        if wrappers > limits::MAX_FIELDS {
            return Err(Invalid("encoded field budget").into());
        }
        let field = fields::field(&mut body)?;
        if !matches!(field.number, 1 | 2) {
            continue;
        }
        let index = if field.number == 1 { 0 } else { 1 };
        snapshots[index] += 1;
        if snapshots[0] > 1 || snapshots[1] > limits::MAX_RETAINED {
            return Err(Invalid("duplicate field or collection count").into());
        }
        let snapshot = field.bytes()?;
        if snapshot.len() > crate::monitor::SERVER_LIMITS.message {
            return Err(Invalid("discovery snapshot encoding budget").into());
        }
        total += scan::measure_discovery(snapshot)?;
        total += 1;
        if total > limits::MAX_FIELDS {
            return Err(Invalid("combined discovery field budget").into());
        }
        for (count, next) in counts.iter_mut().zip(scan::discovery_records(snapshot)?) {
            *count += next;
        }
        if counts[..4].iter().any(|count| *count > limits::MAX_ROWS)
            || counts[4] > crate::monitor::ports::discovery::MAX_DETAILS * 2
        {
            return Err(Invalid("combined discovery allocation count").into());
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
fn count_limit(message: &str, field: &str) -> usize {
    match (message, field) {
        (".newport.desktop.connection.Discovery", "retained") => {
            crate::desktop::connection::discovery::MAX_RETAINED
        }
        (".newport.desktop.events.Batch", "entries") => 100,
        (".newport.desktop.applications.StatesArgs", "applications") => 128,
        (".newport.desktop.applications.Command", "argv") => 128,
        (".newport.desktop.applications.Desktop", "uris") => 64,
        (".newport.desktop.files.WatchArgs", "directories") => 64,
        (".newport.desktop.terminal.ShellList", "shells") => crate::terminal::MAX_HOLDERS,
        (".newport.desktop.terminal.HeldShell", "attached") => crate::terminal::MAX_ATTACHED,
        (".newport.runtime.Hello", "services") => 15,
        (".newport.runtime.Hello", "dependencies") => 16,
        (".newport.monitor.MetricsSnapshot", "cpus") => crate::monitor::metrics::MAX_CPUS,
        (".newport.monitor.MetricsSnapshot", "disks")
        | (".newport.desktop.metrics.HistoricalDisks", "items") => {
            crate::monitor::metrics::MAX_DISKS
        }
        (".newport.monitor.MetricsSnapshot", "processes") => crate::monitor::metrics::MAX_PROCESSES,
        (".newport.monitor.MetricsSnapshot", "network")
        | (".newport.desktop.metrics.ChartBucket", "network")
        | (".newport.desktop.metrics.HistoricalNetwork", "items") => {
            crate::monitor::metrics::MAX_INTERFACES
        }
        (".newport.monitor.MetricsCore", "history") => crate::monitor::metrics::MAX_HISTORY,
        (".newport.monitor.ServicesSnapshot", "services") => crate::monitor::services::MAX_SERVICES,
        (".newport.monitor.ServicesSnapshot", "timers") => crate::monitor::services::MAX_TIMERS,
        (".newport.monitor.ServicesLogs", "entries") => crate::monitor::services::MAX_LOG_ENTRIES,
        (".newport.monitor.ContainersContainersSnapshot", "containers") => {
            crate::monitor::containers::MAX_CONTAINERS
        }
        (".newport.monitor.ContainersContainerLogPage", "entries") => {
            crate::monitor::containers::MAX_LOG_ENTRIES
        }
        (".newport.remote.catalogue.Catalogue", "apps") => {
            crate::remote_apps::apps::MAX_APPLICATIONS
        }
        (name, _) if name.starts_with(".newport.git.") => 10_000,
        _ => MAX_ITEMS,
    }
}

#[cfg(test)]
mod tests {
    use super::{Kind, discovery, number};

    #[test]
    fn imported_discovery_checks_text_counts_and_shared_field_budget_before_decoding() {
        use crate::encoding::protobuf::generated::newport::discovery as pb;
        use prost::Message;
        let mut snapshot = pb::Snapshot {
            generation: 1,
            ..Default::default()
        };
        let body = snapshot.encode_to_vec();
        let mut fields = 1;
        assert!(discovery(".newport.discovery.Snapshot", &body, &mut fields).unwrap());
        assert_eq!(fields, 0);
        assert!(discovery(".newport.discovery.Snapshot", &body, &mut fields).is_err());
        snapshot.container_engine = Some("e".repeat(crate::monitor::ports::MAX_FIELD + 1));
        assert!(
            discovery(
                ".newport.discovery.Snapshot",
                &snapshot.encode_to_vec(),
                &mut 100
            )
            .is_err()
        );
        snapshot.container_engine = None;
        snapshot.sockets = vec![pb::Socket::default(); crate::monitor::ports::MAX_LISTENERS + 1];
        assert!(
            discovery(
                ".newport.discovery.Snapshot",
                &snapshot.encode_to_vec(),
                &mut 10000
            )
            .is_err()
        );
        let report = pb::Report {
            issues: vec![pb::Issue {
                message: "m".repeat(crate::monitor::ports::discovery::MAX_DIAGNOSTIC + 1),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(
            discovery(
                ".newport.discovery.Report",
                &report.encode_to_vec(),
                &mut 100
            )
            .is_err()
        );
    }

    #[test]
    fn integer_bounds_distinguish_sign_extension_from_unsigned_overflow() {
        let unsigned = Kind::Integer {
            maximum: u64::from(u32::MAX),
            negative_minimum: None,
        };
        assert!(number(u64::from(u32::MAX), unsigned).is_ok());
        assert!(number(u64::from(u32::MAX) + 1, unsigned).is_err());
        assert!(number(u64::MAX, unsigned).is_err());

        let negative_minimum = u64::from_ne_bytes(i64::from(i32::MIN).to_ne_bytes());
        let positive_maximum = u64::try_from(i32::MAX).unwrap();
        let signed = Kind::Integer {
            maximum: positive_maximum,
            negative_minimum: Some(negative_minimum),
        };
        assert!(number(positive_maximum, signed).is_ok());
        assert!(number(positive_maximum + 1, signed).is_err());
        assert!(number(u64::from(u32::MAX), signed).is_err());
        assert!(number(negative_minimum - 1, signed).is_err());
        assert!(number(negative_minimum, signed).is_ok());
        assert!(number(u64::MAX, signed).is_ok());
    }
}
