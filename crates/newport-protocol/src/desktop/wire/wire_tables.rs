//! Checks the hand-written tables that pre-validate frames before prost
//! decodes them, and the hand-written enum maps, against the compiled
//! schemas. A renamed, renumbered or added proto field fails here instead of
//! slipping past a scan unbounded or being rejected at run time.
//!
//! Each scanning module's `tables()` describes its root tables through
//! [`Table`], with the proto message each root mirrors; nested tables are
//! checked against the message type of the field that holds them. Each
//! module's `enum_maps()` names the proto enum every map mirrors.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Debug,
    panic::Location,
    path::Path,
    rc::Rc,
};

use prost_reflect::{DescriptorPool, EnumDescriptor, Kind, MessageDescriptor};

/// How a scan table reads one field.
#[derive(Clone)]
pub(crate) enum Wire {
    /// Any varint.
    Varint,
    /// A varint that fits 32 bits.
    Varint32,
    /// A varint of 0 or 1.
    Bool,
    /// A varint accepted only in `min..=max`.
    Enum { min: i32, max: i32 },
    /// A string or bytes value.
    Len,
    /// Packed or unpacked 64-bit fixed-width values.
    Fixed64,
    /// Any value that is not length-delimited, left unchecked.
    Scalar,
    /// A nested message, checked by its own table.
    Message(Table),
}

/// One field a scan table knows.
pub(crate) struct Row {
    number: u32,
    wire: Wire,
    repeated: bool,
    oneof: bool,
}

impl Row {
    pub(crate) fn new(number: u32, wire: Wire) -> Self {
        Self {
            number,
            wire,
            repeated: false,
            oneof: false,
        }
    }

    /// Accepts more than one occurrence.
    pub(crate) fn repeated(mut self, repeated: bool) -> Self {
        self.repeated = repeated;
        self
    }

    /// Excludes the table's other `oneof` rows.
    pub(crate) fn oneof(mut self, oneof: bool) -> Self {
        self.oneof = oneof;
        self
    }
}

/// A scan table, described lazily so nested tables can be followed on demand.
#[derive(Clone)]
pub(crate) struct Table {
    key: String,
    file: &'static str,
    envelope: bool,
    rows: Rc<dyn Fn() -> Vec<Row>>,
}

impl Table {
    /// `key` names the table in failures; equal keys must describe equal rows.
    #[track_caller]
    pub(crate) fn new(key: impl Into<String>, rows: impl Fn() -> Vec<Row> + 'static) -> Self {
        Self {
            key: key.into(),
            file: Location::caller().file(),
            envelope: false,
            rows: Rc::new(rows),
        }
    }

    /// A table that reads some message fields as opaque bytes, without
    /// opening them.
    pub(crate) fn envelope(mut self) -> Self {
        self.envelope = true;
        self
    }

    /// A table for a frame read with `fields::single`: exactly one of the
    /// `numbers` that `child` maps to a table.
    #[track_caller]
    pub(crate) fn dispatch(
        key: impl Into<String>,
        child: impl Fn(u32) -> Option<Table> + 'static,
    ) -> Self {
        Self::new(key, move || {
            numbers()
                .filter_map(|number| {
                    child(number).map(|table| Row::new(number, Wire::Message(table)).oneof(true))
                })
                .collect()
        })
    }
}

/// The field numbers a table described by a `match` is probed with.
pub(crate) fn numbers() -> impl Iterator<Item = u32> {
    1..=1024
}

/// A hand-written map between a Rust enum and a proto enum's numbers.
pub(crate) struct EnumMap {
    proto: &'static str,
    rust: &'static str,
    decode: Box<dyn Fn(i32) -> Option<(String, i32)>>,
}

/// Describes a map through its two directions: `from` decodes a proto
/// number, `to` encodes the decoded value back.
pub(crate) fn enum_map<T: Debug + 'static>(
    proto: &'static str,
    from: impl Fn(i32) -> Option<T> + 'static,
    to: impl Fn(&T) -> i32 + 'static,
) -> EnumMap {
    EnumMap {
        proto,
        rust: short_name::<T>(),
        decode: Box::new(move |value| from(value).map(|rust| (format!("{rust:?}"), to(&rust)))),
    }
}

fn short_name<T>() -> &'static str {
    let name = std::any::type_name::<T>();
    name.rsplit("::").next().unwrap_or(name)
}

fn pool() -> DescriptorPool {
    DescriptorPool::decode(crate::encoding::protobuf::DESCRIPTORS).unwrap()
}

/// Every hand-written scan table, by the proto message its root mirrors.
fn roots() -> Vec<(Table, &'static str)> {
    let mut roots = Vec::new();
    roots.extend(crate::monitor::wire::scan::tables());
    roots.extend(crate::git::wire::tables());
    roots.extend(crate::integration::wire::tables());
    roots.extend(crate::clipboard::exports::wire::scan::tables());
    roots.extend(crate::clipboard::source::wire::tables());
    roots.extend(crate::clipboard::service::wire::tables());
    roots.extend(crate::clipboard::provider::wire::tables());
    roots.extend(crate::clipboard::helper::wire::tables());
    roots.extend(crate::terminal::holder::wire::tables());
    roots.extend(crate::browser::wire::tables());
    roots.extend(crate::display::attachment::tables());
    roots.extend(crate::remote_apps::apps::wire::tables());
    roots
}

/// Every hand-written enum map.
fn enum_maps() -> Vec<EnumMap> {
    let mut maps = Vec::new();
    maps.extend(super::enum_maps());
    maps.extend(crate::desktop::stub::wire::enum_maps());
    maps.extend(crate::monitor::wire::enum_maps());
    maps.extend(crate::monitor::ports::discovery::wire::enum_maps());
    maps.extend(crate::git::wire::enum_maps());
    maps.extend(crate::remote_apps::wire::enum_maps());
    maps.extend(crate::clipboard::provider::wire::enum_maps());
    maps
}

/// Fields no scan table lists, and why.
fn unscanned(message: &str, field: &str) -> bool {
    // Every worker service frame reserves it; `worker::wire::reject_controls`
    // takes it out before the service's own scan runs.
    field == "worker_control"
        && (message.ends_with("ClientFrame") || message.ends_with("ServerFrame"))
}

/// Files that read fields by number without a [`Table`], and why.
const UNTABLED: &[(&str, &str)] = &[
    (
        "desktop/wire/scan.rs",
        "walks tables generated from the descriptors",
    ),
    (
        "desktop/stub/wire/scan.rs",
        "walks tables generated from the descriptors",
    ),
    (
        "worker/wire/scan.rs",
        "splits the reserved control field from service bytes",
    ),
    ("worker/wire.rs", "reads the reserved control field's cases"),
    (
        "runtime/wire.rs",
        "reads a frame's one case by the tag its macro names",
    ),
    (
        "terminal/wire.rs",
        "counts holders, attachments and exits for allocation",
    ),
    (
        "monitor/ports/discovery/helper/wire.rs",
        "reads the envelope's contract field",
    ),
];

struct Checker {
    errors: Vec<String>,
    seen: BTreeSet<(String, String)>,
}

impl Checker {
    fn table(&mut self, table: &Table, message: &MessageDescriptor, path: &str) {
        if !self
            .seen
            .insert((table.key.clone(), message.full_name().to_owned()))
        {
            return;
        }
        let rows = (table.rows)();
        let mut numbers = BTreeSet::new();
        let mut oneofs = BTreeSet::new();
        for row in &rows {
            let at = format!("{path} ({}) field {}", table.key, row.number);
            if !numbers.insert(row.number) {
                self.errors.push(format!("{at}: listed twice"));
                continue;
            }
            let Some(field) = message.get_field(row.number) else {
                self.errors
                    .push(format!("{at}: {} has no such field", message.full_name()));
                continue;
            };
            let at = format!("{path} ({}) {} = {}", table.key, field.name(), row.number);
            let list = field.is_list() || field.is_map();
            if row.repeated != list {
                self.errors.push(format!(
                    "{at}: the table reads it as {}, the schema declares it {}",
                    if row.repeated { "repeated" } else { "singular" },
                    if list { "repeated" } else { "singular" },
                ));
            }
            let oneof = field
                .containing_oneof()
                .filter(|oneof| !oneof.is_synthetic());
            match (&oneof, row.oneof) {
                (Some(oneof), true) => {
                    oneofs.insert(oneof.name().to_owned());
                }
                // A message of one field needs no oneof for its cases to exclude each other.
                (None, true) if message.fields().len() > 1 => self.errors.push(format!(
                    "{at}: the table excludes it, the schema has no oneof"
                )),
                (None, _) => {}
                (Some(oneof), false) => self.errors.push(format!(
                    "{at}: in oneof {} but the table doesn't exclude the other cases",
                    oneof.name()
                )),
            }
            let kind = field.kind();
            let fits = match (&row.wire, &kind) {
                (
                    Wire::Varint,
                    Kind::Int32
                    | Kind::Int64
                    | Kind::Uint32
                    | Kind::Uint64
                    | Kind::Sint32
                    | Kind::Sint64
                    | Kind::Bool
                    | Kind::Enum(_),
                ) => true,
                // Zigzag keeps every `sint32` within 32 bits.
                (Wire::Varint32, Kind::Uint32 | Kind::Sint32 | Kind::Bool | Kind::Enum(_)) => true,
                (Wire::Bool, Kind::Bool) => true,
                (Wire::Enum { min, max }, Kind::Enum(values)) => {
                    let accepted: BTreeSet<i32> = (*min..=*max).collect();
                    let declared: BTreeSet<i32> = values
                        .values()
                        .map(|value| value.number())
                        .filter(|number| number >= min)
                        .collect();
                    if accepted != declared {
                        self.errors.push(format!(
                            "{at}: the table accepts {accepted:?}, {} declares {declared:?}",
                            values.full_name()
                        ));
                    }
                    true
                }
                (Wire::Len, Kind::String | Kind::Bytes) => true,
                (Wire::Len, Kind::Message(_)) => table.envelope,
                (Wire::Fixed64, Kind::Fixed64 | Kind::Sfixed64 | Kind::Double) => true,
                (Wire::Scalar, kind) => {
                    !matches!(kind, Kind::String | Kind::Bytes | Kind::Message(_))
                }
                (Wire::Message(child), Kind::Message(child_message)) => {
                    self.table(child, child_message, &format!("{path}.{}", field.name()));
                    true
                }
                _ => false,
            };
            if !fits {
                self.errors.push(format!(
                    "{at}: the table reads it as {}, the schema declares {kind:?}",
                    wire_name(&row.wire)
                ));
            }
            if field.is_packed()
                && matches!(
                    row.wire,
                    Wire::Varint | Wire::Varint32 | Wire::Bool | Wire::Enum { .. }
                )
            {
                self.errors.push(format!(
                    "{at}: packed in the schema, but the table reads one varint per field"
                ));
            }
        }
        if oneofs.len() > 1 {
            self.errors.push(format!(
                "{path} ({}): one exclusive group covers oneofs {oneofs:?}",
                table.key
            ));
        }
        for field in message.fields() {
            if !numbers.contains(&field.number()) && !unscanned(message.full_name(), field.name()) {
                self.errors.push(format!(
                    "{path} ({}): {} = {} of {} is not in the table",
                    table.key,
                    field.name(),
                    field.number(),
                    message.full_name()
                ));
            }
        }
    }

    fn enum_map(&mut self, map: &EnumMap, proto: &EnumDescriptor) {
        let at = format!("{} for {}", map.rust, map.proto);
        let declared: BTreeMap<i32, String> = proto
            .values()
            .map(|value| (value.number(), value.name().to_owned()))
            .collect();
        let highest = declared.keys().max().copied().unwrap_or(0);
        let probes: BTreeSet<i32> = declared
            .keys()
            .copied()
            .chain(-2..=highest.saturating_add(64))
            .chain([i32::MIN, i32::MAX])
            .collect();
        for value in probes {
            match ((map.decode)(value), declared.get(&value)) {
                (Some((rust, back)), Some(name)) => {
                    let shouty = shouty(&rust);
                    if *name != shouty && !name.ends_with(&format!("_{shouty}")) {
                        self.errors.push(format!(
                            "{at}: {value} decodes as {rust}, the schema calls it {name}"
                        ));
                    }
                    if back != value {
                        self.errors.push(format!(
                            "{at}: {value} decodes as {rust}, which encodes as {back}"
                        ));
                    }
                }
                (Some((rust, _)), None) => self.errors.push(format!(
                    "{at}: {value} decodes as {rust}, the schema has no such value"
                )),
                (None, Some(name)) if value != 0 => self.errors.push(format!(
                    "{at}: the schema's {name} = {value} doesn't decode"
                )),
                (None, _) => {}
            }
        }
    }
}

fn wire_name(wire: &Wire) -> &'static str {
    match wire {
        Wire::Varint => "a varint",
        Wire::Varint32 => "a 32-bit varint",
        Wire::Bool => "a boolean",
        Wire::Enum { .. } => "an enumeration",
        Wire::Len => "text or bytes",
        Wire::Fixed64 => "fixed 64-bit values",
        Wire::Scalar => "a scalar",
        Wire::Message(_) => "a message",
    }
}

/// `CamelCase` as `SCREAMING_SNAKE`, the way proto enum values are named; a
/// digit stays with the word before it.
fn shouty(name: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = name.chars().collect();
    for (index, &char) in chars.iter().enumerate() {
        let boundary = index > 0
            && char.is_uppercase()
            && (chars[index - 1].is_lowercase()
                || chars.get(index + 1).is_some_and(|next| next.is_lowercase()));
        if boundary {
            out.push('_');
        }
        out.extend(char.to_uppercase());
    }
    out
}

/// The crate's non-test sources, by path relative to `src/`.
fn sources() -> Vec<(String, String)> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, root, out);
                continue;
            }
            let name = path.file_name().unwrap().to_string_lossy();
            if !name.ends_with(".rs") || name == "tests.rs" || name.ends_with("_tests.rs") {
                continue;
            }
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            out.push((relative, std::fs::read_to_string(&path).unwrap()));
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut out = Vec::new();
    walk(&root, &root, &mut out);
    out
}

/// `file!()` relative to `src/`, with `/` between components on every
/// platform, as `sources` names files.
fn source_path(file: &str) -> String {
    let file = file.replace('\\', "/");
    file.rsplit_once("src/")
        .map_or(file.as_str(), |(_, path)| path)
        .to_owned()
}

/// The Rust types the crate's sources decode from a proto `i32`: `Convert`
/// and `Record` impls whose wire type is `i32`, `enumeration!` invocations,
/// and free functions taking an `i32` value.
fn decoded_enums(source: &str) -> Vec<String> {
    let last = |ty: &str| ty.rsplit("::").next().unwrap().to_owned();
    let mut found = Vec::new();
    let mut implemented = None;
    for line in source.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("impl ")
            && let Some((_, ty)) = rest.split_once(" for ")
        {
            implemented = ty.split_whitespace().next().map(str::to_owned);
        }
        if matches!(line, "type Proto = i32;" | "type Wire = i32;")
            && let Some(ty) = &implemented
        {
            found.push(last(ty));
        }
        if let Some(rest) = line.strip_prefix("enumeration!(") {
            found.push(rest.split([' ', '{']).next().unwrap().to_owned());
        }
        if let Some((head, rest)) = line.split_once("(value: i32) -> Result<")
            && head.starts_with("fn ")
            && !rest.starts_with("Self")
        {
            found.push(last(rest.split([',', '>']).next().unwrap()));
        }
    }
    found
}

#[test]
fn source_paths_use_forward_slashes_on_every_platform() {
    for file in [
        r"crates\newport-protocol\src\git\wire\scan.rs",
        "crates/newport-protocol/src/git/wire/scan.rs",
    ] {
        assert_eq!(source_path(file), "git/wire/scan.rs");
    }
}

#[test]
fn scan_tables_match_the_schemas() {
    let pool = pool();
    let mut checker = Checker {
        errors: Vec::new(),
        seen: BTreeSet::new(),
    };
    let roots = roots();
    for (table, message) in &roots {
        match pool.get_message_by_name(message) {
            Some(descriptor) => checker.table(table, &descriptor, message),
            None => checker
                .errors
                .push(format!("{} maps to missing {message}", table.key)),
        }
    }
    let tabled: BTreeSet<String> = roots
        .iter()
        .map(|(table, _)| source_path(table.file))
        .collect();
    for (path, source) in sources() {
        let reads = source.contains("fields::field(") || source.contains("fields::single(");
        if !reads || path.starts_with("encoding/") || path == source_path(file!()) {
            continue;
        }
        if !tabled.contains(&path) && !UNTABLED.iter().any(|(file, _)| *file == path) {
            checker.errors.push(format!(
                "{path} reads fields by number, but no scan table describes it"
            ));
        }
    }
    assert!(checker.errors.is_empty(), "\n{}", checker.errors.join("\n"));
}

#[test]
fn enum_maps_match_the_schemas() {
    let pool = pool();
    let mut checker = Checker {
        errors: Vec::new(),
        seen: BTreeSet::new(),
    };
    let maps = enum_maps();
    for map in &maps {
        match pool.get_enum_by_name(map.proto) {
            Some(proto) => checker.enum_map(map, &proto),
            None => checker
                .errors
                .push(format!("{} maps to missing enum {}", map.rust, map.proto)),
        }
    }
    let mut described: Vec<&str> = maps.iter().map(|map| map.rust).collect();
    described.sort_unstable();
    let mut decoded: Vec<String> = sources()
        .iter()
        .flat_map(|(_, source)| decoded_enums(source))
        .collect();
    decoded.sort_unstable();
    assert_eq!(
        decoded, described,
        "every Rust type decoded from a proto i32 needs an enum_map entry"
    );
    assert!(checker.errors.is_empty(), "\n{}", checker.errors.join("\n"));
}

#[test]
fn count_limits_name_repeated_fields() {
    let pool = pool();
    let mut errors = Vec::new();
    for (message, field) in super::scan::count_limit_keys() {
        let Some(descriptor) = pool.get_message_by_name(message.trim_start_matches('.')) else {
            errors.push(format!("{message} is not a message"));
            continue;
        };
        match descriptor.get_field_by_name(field) {
            Some(field) if field.is_list() || field.is_map() => {}
            Some(_) => errors.push(format!("{message}.{field} is not repeated")),
            None => errors.push(format!("{message} has no field {field}")),
        }
    }
    for package in super::scan::count_limit_packages() {
        if !pool
            .all_messages()
            .any(|message| format!(".{}", message.full_name()).starts_with(package))
        {
            errors.push(format!("no message is in {package}"));
        }
    }
    assert!(errors.is_empty(), "\n{}", errors.join("\n"));
}

#[test]
fn shouty_names_split_words_and_keep_digits() {
    assert_eq!(shouty("HeadToIndex"), "HEAD_TO_INDEX");
    assert_eq!(shouty("TupleV1"), "TUPLE_V1");
    assert_eq!(shouty("Io"), "IO");
}
