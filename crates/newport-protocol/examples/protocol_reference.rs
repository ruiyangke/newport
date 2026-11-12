//! Documentation tooling only: reads the descriptor set produced by build.rs.
use prost::Message;
use prost_types::{
    DescriptorProto, EnumDescriptorProto, FieldDescriptorProto, FileDescriptorProto,
    FileDescriptorSet,
    field_descriptor_proto::{Label, Type},
};
use std::{collections::BTreeMap, error::Error, fmt::Write as _, fs, path::Path};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

struct Schema<'a> {
    file: &'a FileDescriptorProto,
    comments: BTreeMap<Vec<i32>, String>,
    maps: BTreeMap<String, (&'a FieldDescriptorProto, &'a FieldDescriptorProto)>,
}
impl<'a> Schema<'a> {
    fn new(file: &'a FileDescriptorProto) -> Self {
        let comments = file
            .source_code_info
            .as_ref()
            .into_iter()
            .flat_map(|info| &info.location)
            .map(|location| {
                let text = location
                    .leading_detached_comments
                    .iter()
                    .map(String::as_str)
                    .chain(location.leading_comments.as_deref())
                    .chain(location.trailing_comments.as_deref())
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n\n");
                (location.path.clone(), text)
            })
            .collect();
        let mut value = Self {
            file,
            comments,
            maps: BTreeMap::new(),
        };
        value.collect_maps(file.package(), &file.message_type);
        value
    }
    fn collect_maps(&mut self, parent: &str, messages: &'a [DescriptorProto]) {
        for message in messages {
            let name = format!("{parent}.{}", message.name());
            if is_map(message) && message.field.len() == 2 {
                self.maps
                    .insert(format!(".{name}"), (&message.field[0], &message.field[1]));
            }
            self.collect_maps(&name, &message.nested_type);
        }
    }
    fn comment(&self, path: &[i32]) -> &str {
        self.comments
            .get(path)
            .map(String::as_str)
            .unwrap_or_default()
    }
    fn prose(&self, output: &mut String, path: &[i32]) {
        let comment = self.comment(path);
        if !comment.is_empty() {
            writeln!(output, "{comment}\n").unwrap();
        }
    }
    fn field_type(&self, field: &FieldDescriptorProto) -> String {
        if let Some((key, value)) = self.maps.get(field.type_name()) {
            return format!("map<{}, {}>", scalar_type(key), scalar_type(value));
        }
        scalar_type(field)
    }
    fn messages(
        &self,
        output: &mut String,
        parent: &str,
        messages: &[DescriptorProto],
        path: &[i32],
    ) {
        for (index, message) in messages.iter().enumerate() {
            if is_map(message) {
                continue;
            }
            let mut current = path.to_vec();
            current.push(index as i32);
            let name = format!("{parent}.{}", message.name());
            writeln!(output, "### `{name}`\n").unwrap();
            self.prose(output, &current);
            if message.field.is_empty() {
                writeln!(output, "Empty message.\n").unwrap();
            } else {
                writeln!(output, "| Tag | Field | Type | Presence | Schema comment |\n| --- | --- | --- | --- | --- |").unwrap();
                for (index, field) in message.field.iter().enumerate() {
                    let mut field_path = current.clone();
                    field_path.extend([2, index as i32]);
                    let presence = if self.maps.contains_key(field.type_name()) {
                        "map".to_owned()
                    } else if field.proto3_optional() {
                        "optional".to_owned()
                    } else if let Some(oneof) = field.oneof_index {
                        format!("oneof `{}`", message.oneof_decl[oneof as usize].name())
                    } else if field.label() == Label::Repeated {
                        "repeated".to_owned()
                    } else if field.r#type() == Type::Message {
                        "message presence".to_owned()
                    } else {
                        "implicit".to_owned()
                    };
                    writeln!(
                        output,
                        "| {} | `{}` | `{}` | {} | {} |",
                        field.number(),
                        field.name(),
                        self.field_type(field),
                        presence,
                        cell(self.comment(&field_path))
                    )
                    .unwrap();
                }
                output.push('\n');
            }
            for (index, oneof) in message.oneof_decl.iter().enumerate() {
                let mut oneof_path = current.clone();
                oneof_path.extend([8, index as i32]);
                let comment = self.comment(&oneof_path);
                if !comment.is_empty() {
                    writeln!(output, "Oneof `{}`: {}\n", oneof.name(), comment).unwrap();
                }
            }
            if !message.reserved_range.is_empty() || !message.reserved_name.is_empty() {
                let mut reserved: Vec<_> = message
                    .reserved_range
                    .iter()
                    .map(|range| {
                        if range.end() == range.start() + 1 {
                            range.start().to_string()
                        } else {
                            format!("{}–{}", range.start(), range.end() - 1)
                        }
                    })
                    .collect();
                reserved.extend(message.reserved_name.iter().map(|name| format!("`{name}`")));
                writeln!(output, "Reserved: {}.\n", reserved.join(", ")).unwrap();
            }
            let mut nested_path = current.clone();
            nested_path.push(3);
            self.messages(output, &name, &message.nested_type, &nested_path);
            let mut enum_path = current;
            enum_path.push(4);
            self.enums(output, &name, &message.enum_type, &enum_path);
        }
    }
    fn enums(
        &self,
        output: &mut String,
        parent: &str,
        enums: &[EnumDescriptorProto],
        path: &[i32],
    ) {
        for (index, value) in enums.iter().enumerate() {
            let mut current = path.to_vec();
            current.push(index as i32);
            writeln!(output, "### `{parent}.{}`\n", value.name()).unwrap();
            self.prose(output, &current);
            writeln!(
                output,
                "| Number | Value | Schema comment |\n| --- | --- | --- |"
            )
            .unwrap();
            for (index, variant) in value.value.iter().enumerate() {
                let mut variant_path = current.clone();
                variant_path.extend([2, index as i32]);
                writeln!(
                    output,
                    "| {} | `{}` | {} |",
                    variant.number(),
                    variant.name(),
                    cell(self.comment(&variant_path))
                )
                .unwrap();
            }
            output.push('\n');
        }
    }
    fn render(&self, output: &mut String) {
        let file = self.file;
        writeln!(
            output,
            "## Schema `{}`\n\nSource: [{}](proto/{}). Package: `{}`.\n",
            file.name(),
            file.name(),
            file.name(),
            file.package()
        )
        .unwrap();
        self.prose(output, &[]);
        self.prose(output, &[2]);
        if !file.dependency.is_empty() {
            writeln!(
                output,
                "Imports: {}.\n",
                file.dependency
                    .iter()
                    .map(|name| format!("`{name}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
            .unwrap();
        }
        self.messages(output, file.package(), &file.message_type, &[4]);
        self.enums(output, file.package(), &file.enum_type, &[5]);
    }
}
fn is_map(message: &DescriptorProto) -> bool {
    message
        .options
        .as_ref()
        .is_some_and(|options| options.map_entry())
}
fn scalar_type(field: &FieldDescriptorProto) -> String {
    if matches!(field.r#type(), Type::Message | Type::Enum | Type::Group) {
        return field.type_name().trim_start_matches('.').to_owned();
    }
    field
        .r#type()
        .as_str_name()
        .strip_prefix("TYPE_")
        .unwrap()
        .to_ascii_lowercase()
}
fn cell(comment: &str) -> String {
    comment
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('|', "\\|")
}
fn generated() -> Result<String> {
    let descriptors = FileDescriptorSet::decode(newport_protocol::encoding::protobuf::DESCRIPTORS)?;
    let mut files: Vec<_> = descriptors
        .file
        .iter()
        .filter(|file| file.name().starts_with("newport/"))
        .collect();
    files.sort_by_key(|file| file.name());
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut expected = Vec::new();
    collect_protos(
        &root.join("proto/newport"),
        &root.join("proto"),
        &mut expected,
    )?;
    expected.sort();
    let actual: Vec<_> = files.iter().map(|file| file.name().to_owned()).collect();
    if actual != expected {
        return Err(
            "Compiled descriptors do not cover exactly the checked-in Newport schemas".into(),
        );
    }
    if files.iter().any(|file| file.source_code_info.is_none()) {
        return Err("Descriptor source comments are missing".into());
    }
    let mut output = include_str!("../reference-introduction.md").to_owned();
    writeln!(
        output,
        "\n## Generated schema inventory\n\nThe following {} schemas come from `encoding::protobuf::DESCRIPTORS`, compiled\nby `build.rs` through `codegen.rs` with protox source information enabled.\nNames, field numbers, types, presence, reserved message fields and comments are\nread from that descriptor set; imported Google schemas and synthetic map-entry\nmessages are omitted. Each schema links to its authoritative source.\n\nA field marked implicit uses protobuf's scalar default when absent. Message\npresence and explicit `optional` preserve absence; a `oneof` permits one selected\ncase. Semantic Rust validators can impose stronger requirements. Empty comment\ncells mean that the schema supplies no field comment.\n",
        files.len()
    )?;
    for file in files {
        Schema::new(file).render(&mut output);
    }
    output.truncate(output.trim_end().len());
    output.push('\n');
    Ok(output)
}
fn collect_protos(path: &Path, root: &Path, files: &mut Vec<String>) -> Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            collect_protos(&entry.path(), root, files)?;
        } else if entry
            .path()
            .extension()
            .is_some_and(|value| value == "proto")
        {
            files.push(
                entry
                    .path()
                    .strip_prefix(root)?
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
    Ok(())
}
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let check = match args.as_slice() {
        [] => false,
        [argument] if argument == "--check" => true,
        _ => return Err("Usage: cargo xtask protocol-reference [--check]".into()),
    };
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("REFERENCE.md");
    let output = generated()?;
    if check {
        if fs::read_to_string(&path).ok().as_deref() != Some(output.as_str()) {
            return Err("Protocol reference is stale; run cargo xtask protocol-reference".into());
        }
        println!("Protocol reference is current: {}", path.display());
    } else {
        fs::write(&path, output)?;
        println!("Generated protocol reference: {}", path.display());
    }
    Ok(())
}
