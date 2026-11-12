//! Structural desktop checks derive from the same descriptors as prost's records.
use prost_types::{
    DescriptorProto, EnumDescriptorProto, FileDescriptorSet,
    field_descriptor_proto::{Label, Type},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt::Write as _,
    fs,
    path::Path,
};
const ROOTS: &[(&str, &str)] = &[
    ("ClientMessage", ".newport.desktop.ClientMessage"),
    ("ServerMessage", ".newport.desktop.ServerMessage"),
    ("TerminalClient", ".newport.desktop.stream.TerminalClient"),
    ("TerminalServer", ".newport.desktop.stream.TerminalServer"),
    ("OpaqueClient", ".newport.desktop.stream.OpaqueClient"),
    ("OpaqueServer", ".newport.desktop.stream.OpaqueServer"),
];
pub(super) fn generate(
    out_dir: &Path,
    descriptors: &FileDescriptorSet,
) -> Result<(), Box<dyn Error>> {
    generate_family(out_dir, descriptors, ROOTS, "desktop_fields.rs")?;
    generate_family(
        out_dir,
        descriptors,
        &[
            ("Host", ".newport.desktop.stub.HostFrame"),
            ("Stub", ".newport.desktop.stub.StubFrame"),
        ],
        "stub_fields.rs",
    )
}
fn generate_family(
    out_dir: &Path,
    descriptors: &FileDescriptorSet,
    roots: &[(&str, &str)],
    filename: &str,
) -> Result<(), Box<dyn Error>> {
    let mut messages = BTreeMap::new();
    let mut enums = BTreeMap::new();
    for file in &descriptors.file {
        let package = format!(".{}", file.package());
        collect(
            &package,
            &file.message_type,
            &file.enum_type,
            &mut messages,
            &mut enums,
        );
    }
    if !messages.contains_key(roots[0].1) {
        return Ok(());
    }
    let mut reached = BTreeSet::new();
    for (_, root) in roots {
        visit(root, &messages, &mut reached)?;
    }
    let ids: BTreeMap<_, _> = reached
        .iter()
        .enumerate()
        .map(|(id, name)| (name.as_str(), id))
        .collect();
    let mut code = String::from("#[derive(Clone, Copy)]\npub(super) enum Shape {\n");
    for (variant, _) in roots {
        writeln!(code, "{variant},")?;
    }
    code.push_str("}\nimpl Shape { fn schema(self) -> usize { match self {\n");
    for (variant, root) in roots {
        let id = ids.get(root).ok_or("missing desktop root")?;
        writeln!(code, "Self::{variant} => {id},")?;
    }
    code.push_str("} } }\nfn schema(id: usize) -> Option<&'static Schema> { Some(match id {\n");
    for (name, id) in &ids {
        let message = messages.get(*name).ok_or("missing desktop message")?;
        if message.field.len() > 64 || message.oneof_decl.len() > 64 {
            return Err(format!("desktop scanner shape exceeds slots: {name}").into());
        }
        writeln!(code, "{id} => &Schema {{ name: {name:?}, fields: &[")?;
        for field in &message.field {
            let kind = match field.r#type() {
                Type::Message => {
                    let child = messages
                        .get(field.type_name())
                        .ok_or("missing field message")?;
                    let id = ids
                        .get(field.type_name())
                        .ok_or("unvisited field message")?;
                    let constructor = if child.options.as_ref().is_some_and(|v| v.map_entry()) {
                        "Map"
                    } else {
                        "Message"
                    };
                    format!("Kind::{constructor}({id})")
                }
                Type::Enum => {
                    let values = enums.get(field.type_name()).ok_or("missing field enum")?;
                    let values = values
                        .value
                        .iter()
                        .map(|v| {
                            u64::from_ne_bytes(i64::from(v.number()).to_ne_bytes()).to_string()
                        })
                        .collect::<Vec<_>>()
                        .join(",");
                    format!("Kind::Enum(&[{values}])")
                }
                Type::Bool => "Kind::Boolean".into(),
                Type::Uint32 | Type::Sint32 => format!(
                    "Kind::Integer {{ maximum: {}, negative_minimum: None }}",
                    u32::MAX,
                ),
                Type::Int32 => format!(
                    "Kind::Integer {{ maximum: {}, negative_minimum: Some({}) }}",
                    i32::MAX,
                    u64::from_ne_bytes(i64::from(i32::MIN).to_ne_bytes()),
                ),
                Type::Int64 | Type::Uint64 | Type::Sint64 => {
                    "Kind::Integer { maximum: u64::MAX, negative_minimum: None }".into()
                }
                Type::Double | Type::Fixed64 | Type::Sfixed64 => "Kind::Fixed64".into(),
                Type::Float | Type::Fixed32 | Type::Sfixed32 => "Kind::Fixed32".into(),
                Type::String => "Kind::Text".into(),
                Type::Bytes => "Kind::Data".into(),
                Type::Group => return Err("desktop schemas cannot contain protobuf groups".into()),
            };
            let group = field
                .oneof_index
                .filter(|_| !field.proto3_optional())
                .map(|v| format!("Some({v})"))
                .unwrap_or_else(|| "None".into());
            writeln!(
                code,
                "Rule {{ tag: {}, name: {:?}, repeated: {}, group: {group}, kind: {kind} }},",
                field.number(),
                field.name(),
                field.label() == Label::Repeated
            )?;
        }
        code.push_str("] },\n");
    }
    code.push_str("_ => return None,\n}) }\n");
    fs::write(out_dir.join(filename), code)?;
    Ok(())
}
fn collect<'a>(
    scope: &str,
    definitions: &'a [DescriptorProto],
    enum_definitions: &'a [EnumDescriptorProto],
    messages: &mut BTreeMap<String, &'a DescriptorProto>,
    enums: &mut BTreeMap<String, &'a EnumDescriptorProto>,
) {
    for value in enum_definitions {
        enums.insert(format!("{scope}.{}", value.name()), value);
    }
    for message in definitions {
        let name = format!("{scope}.{}", message.name());
        collect(
            &name,
            &message.nested_type,
            &message.enum_type,
            messages,
            enums,
        );
        messages.insert(name, message);
    }
}
fn visit(
    name: &str,
    messages: &BTreeMap<String, &DescriptorProto>,
    reached: &mut BTreeSet<String>,
) -> Result<(), Box<dyn Error>> {
    if !reached.insert(name.to_owned()) {
        return Ok(());
    }
    let message = messages
        .get(name)
        .ok_or("missing reachable desktop message")?;
    for field in &message.field {
        if field.r#type() == Type::Message {
            visit(field.type_name(), messages, reached)?;
        }
    }
    Ok(())
}

/// Heap-place message alternatives; generated oneofs should not inherit the largest DTO's stack size.
pub(super) fn configure(config: &mut prost_build::Config, descriptors: &FileDescriptorSet) {
    let mut messages = BTreeMap::new();
    let mut enums = BTreeMap::new();
    for file in &descriptors.file {
        let package = format!(".{}", file.package());
        collect(
            &package,
            &file.message_type,
            &file.enum_type,
            &mut messages,
            &mut enums,
        );
    }
    for (name, message) in messages {
        if !name.starts_with(".newport.desktop.") {
            continue;
        }
        for field in &message.field {
            if field.r#type() != Type::Message || field.proto3_optional() {
                continue;
            }
            let Some(index) = field.oneof_index.and_then(|v| usize::try_from(v).ok()) else {
                continue;
            };
            let Some(oneof) = message.oneof_decl.get(index) else {
                continue;
            };
            super::boxed_oneof(config, &name, oneof.name(), field.name());
        }
    }
}
