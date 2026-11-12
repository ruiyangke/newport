//! Schema compilation and bounded-layout generation for this package.
use std::{
    collections::BTreeMap,
    error::Error,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

#[path = "codegen_desktop.rs"]
mod desktop;

use heck::{ToSnakeCase, ToUpperCamelCase};
use prost_types::{
    DescriptorProto, FieldDescriptorProto, FileDescriptorSet, field_descriptor_proto::Type,
};

/// The directory holding `newport/wire.proto`, added to every compile's includes.
const PROTO_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/proto");

/// The generated modules, with a `Layout` for every message, to `include!`.
const MODULES_FILE: &str = "protos.rs";

/// The encoded `FileDescriptorSet` of the compiled files and their imports.
const DESCRIPTORS_FILE: &str = "descriptors.bin";

const WIRE_PACKAGE: &str = ".newport.wire";

fn boxed_oneof(config: &mut prost_build::Config, message: &str, oneof: &str, field: &str) {
    // Prost resolves oneof layout with the group name, but Copy eligibility without it.
    config
        .boxed(format!("{message}.{oneof}.{field}"))
        .boxed(format!("{message}.{field}"));
}

/// Generates imported schemas into another directory; returns the files read.
pub(crate) fn compile_into(
    out_dir: &Path,
    files: &[impl AsRef<Path>],
    includes: &[impl AsRef<Path>],
    externs: &[(&str, &str)],
) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let mut externs = externs.to_vec();
    externs.push((WIRE_PACKAGE, "::newport_protocol::encoding::protobuf"));
    generate(out_dir, files, includes, &externs)
}

/// Generates `files` with the packages in `externs` taken from elsewhere;
/// the files it read.
pub(crate) fn generate(
    out_dir: &Path,
    files: &[impl AsRef<Path>],
    includes: &[impl AsRef<Path>],
    externs: &[(&str, &str)],
) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let compiler = compiler(files, includes)?;
    let read = compiler
        .files()
        .filter_map(|file| file.path().map(Path::to_path_buf))
        .collect();
    fs::create_dir_all(out_dir)?;
    fs::write(
        out_dir.join(DESCRIPTORS_FILE),
        compiler.encode_file_descriptor_set(),
    )?;

    let descriptors = compiler.file_descriptor_set();
    desktop::generate(out_dir, &descriptors)?;
    let mut config = prost_build::Config::new();
    desktop::configure(&mut config, &descriptors);
    boxed_oneof(
        &mut config,
        ".newport.monitor.ServerFrame",
        "message",
        "response",
    );
    boxed_oneof(
        &mut config,
        ".newport.discovery.HelperReply",
        "result",
        "snapshot",
    );
    config
        .boxed(".newport.discovery.HelperRequest.projects")
        .out_dir(out_dir)
        .include_file(MODULES_FILE)
        .bytes(["."])
        .skip_debug([".newport.wire.Secret"]);
    for field in [
        "open",
        "close",
        "status_summary",
        "status",
        "branches",
        "worktrees",
        "remotes",
        "remote",
        "remote_names",
        "remote_refs",
        "blob",
        "blob_page",
        "stashes",
        "tags",
        "tag",
        "history",
        "commit",
        "commit_files",
        "diff_page",
        "commit_diff_page",
        "operation",
        "stream_complete",
    ] {
        boxed_oneof(&mut config, ".newport.git.Reply", "value", field);
    }
    for &(package, path) in externs {
        config.extern_path(package, path);
    }
    let mut externs = externs.to_vec();
    externs.push((".google.protobuf", "::prost_types"));
    let layouts = layouts(&descriptors, &externs);
    config.compile_fds(descriptors)?;

    let modules = out_dir.join(MODULES_FILE);
    let mut code = fs::read_to_string(&modules)?;
    code.push_str(&layouts);
    fs::write(modules, code)?;
    Ok(read)
}

/// A compiler that has read `files` and their imports, with
/// `newport/wire.proto` among its includes.
fn compiler(
    files: &[impl AsRef<Path>],
    includes: &[impl AsRef<Path>],
) -> Result<protox::Compiler, Box<dyn Error>> {
    let includes = includes
        .iter()
        .map(|path| path.as_ref().to_path_buf())
        .chain([PathBuf::from(PROTO_DIR)]);
    let mut compiler = protox::Compiler::new(includes)?;
    compiler
        .include_source_info(true)
        .include_imports(true)
        .open_files(files)?;
    Ok(compiler)
}

/// `impl Layout` for every message the crate generates. The impls sit beside
/// the package modules, so a local type's path starts at `self`.
fn layouts(descriptors: &FileDescriptorSet, externs: &[(&str, &str)]) -> String {
    let mut messages = BTreeMap::new();
    for file in &descriptors.file {
        let package = match file.package() {
            "" => String::new(),
            package => format!(".{package}"),
        };
        collect(&package, &file.message_type, &mut messages);
    }
    let mut code = String::new();
    for (name, message) in &messages {
        if is_map_entry(message) || extern_root(name, externs).is_some() {
            continue;
        }
        let fields = message
            .field
            .iter()
            .filter_map(|field| field_layout(field, &messages, externs))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(
            code,
            "impl ::newport_protocol::encoding::protobuf::Layout for {} {{ fn fields() -> &'static [::newport_protocol::encoding::protobuf::Field] {{ \
             const FIELDS: &[::newport_protocol::encoding::protobuf::Field] = &[{fields}]; FIELDS }} }}",
            rust_path(name, externs),
        );
    }
    code
}

fn collect<'a>(
    scope: &str,
    messages: &'a [DescriptorProto],
    into: &mut BTreeMap<String, &'a DescriptorProto>,
) {
    for message in messages {
        let name = format!("{scope}.{}", message.name());
        collect(&name, &message.nested_type, into);
        into.insert(name, message);
    }
}

/// The `Field` for `field`, if it holds bytes anywhere inside.
fn field_layout(
    field: &FieldDescriptorProto,
    messages: &BTreeMap<String, &DescriptorProto>,
    externs: &[(&str, &str)],
) -> Option<String> {
    let number = field.number();
    match field.r#type() {
        Type::Bytes => Some(format!(
            "::newport_protocol::encoding::protobuf::Field::bytes({number})"
        )),
        Type::Message => {
            let name = field.type_name();
            // Well-known types have no `Layout`; their bytes count as metadata.
            if name.starts_with(".google.protobuf.") {
                return None;
            }
            let message = messages.get(name)?;
            if !is_map_entry(message) {
                let path = rust_path(name, externs);
                return Some(format!(
                    "::newport_protocol::encoding::protobuf::Field::message({number}, <{path} as ::newport_protocol::encoding::protobuf::Layout>::fields)"
                ));
            }
            // A map is a repeated entry message with no Rust type of its own.
            let value = message.field.iter().find(|field| field.number() == 2)?;
            let value = field_layout(value, messages, externs)?;
            Some(format!(
                "::newport_protocol::encoding::protobuf::Field::message({number}, {{ fn entry() -> &'static [::newport_protocol::encoding::protobuf::Field] {{ \
                 const FIELDS: &[::newport_protocol::encoding::protobuf::Field] = &[{value}]; FIELDS }} entry }})"
            ))
        }
        _ => None,
    }
}

fn is_map_entry(message: &DescriptorProto) -> bool {
    message
        .options
        .as_ref()
        .is_some_and(|options| options.map_entry())
}

fn extern_root<'a>(name: &'a str, externs: &[(&str, &'a str)]) -> Option<(&'a str, &'a str)> {
    externs.iter().find_map(|&(package, root)| {
        let rest = name.strip_prefix(package)?.strip_prefix('.')?;
        Some((root, rest))
    })
}

/// The Rust path prost-build gives the message `name`: snake case modules for
/// packages and enclosing messages, then the type in upper camel case.
fn rust_path(name: &str, externs: &[(&str, &str)]) -> String {
    let (root, rest) = extern_root(name, externs).unwrap_or(("self", &name[1..]));
    let mut segments = rest.split('.').collect::<Vec<_>>();
    let ty = segments.pop().unwrap_or_default();
    let mut path = root.to_owned();
    for segment in segments {
        path.push_str("::");
        path.push_str(&sanitize(segment.to_snake_case()));
    }
    path.push_str("::");
    path.push_str(&sanitize(ty.to_upper_camel_case()));
    path
}

/// prost-build's escaping of identifiers that are Rust keywords.
fn sanitize(ident: String) -> String {
    match ident.as_str() {
        "as" | "break" | "const" | "continue" | "else" | "enum" | "false" | "fn" | "for" | "if"
        | "impl" | "in" | "let" | "loop" | "match" | "mod" | "move" | "mut" | "pub" | "ref"
        | "return" | "static" | "struct" | "trait" | "true" | "type" | "unsafe" | "use"
        | "where" | "while" | "dyn" | "abstract" | "become" | "box" | "do" | "final" | "macro"
        | "override" | "priv" | "typeof" | "unsized" | "virtual" | "yield" | "async" | "await"
        | "try" | "gen" => format!("r#{ident}"),
        "_" | "super" | "self" | "Self" | "extern" | "crate" => format!("{ident}_"),
        _ if ident.starts_with(|c: char| c.is_numeric()) => format!("_{ident}"),
        _ => ident,
    }
}
