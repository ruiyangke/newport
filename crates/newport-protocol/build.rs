#[cfg(not(feature = "protobuf"))]
fn main() {}

#[allow(dead_code)]
#[cfg(feature = "protobuf")]
#[path = "codegen.rs"]
mod codegen;
#[cfg(feature = "protobuf")]
mod contract_id;

#[cfg(feature = "protobuf")]
use std::{collections::BTreeSet, env, error::Error, path::PathBuf};

#[cfg(feature = "protobuf")]
fn main() -> Result<(), Box<dyn Error>> {
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").ok_or("OUT_DIR is not set")?);
    let mut read = BTreeSet::new();
    read.extend(codegen::generate(
        &out_dir,
        &[
            "newport/wire.proto",
            "newport/desktop/stub.proto",
            "newport/desktop.proto",
            "newport/browser.proto",
            "newport/worker.proto",
            "newport/runtime.proto",
            "newport/terminal.proto",
            "newport/terminal_holder.proto",
            "newport/monitor.proto",
            "newport/discovery.proto",
            "newport/clipboard.proto",
            "newport/clipboard_service.proto",
            "newport/clipboard_source.proto",
            "newport/clipboard_helper.proto",
            "newport/clipboard_provider.proto",
            "newport/integration.proto",
            "newport/git.proto",
            "newport/remote/apps.proto",
            "newport/remote/catalogue.proto",
            "newport/remote/display.proto",
        ],
        &[] as &[&str],
        &[],
    )?);
    // The schemas the integration tests read and write, built as other
    // crates' would be: `ext` takes `wiretest.v1` from `v1`'s.
    read.extend(codegen::compile_into(
        &out_dir.join("test"),
        &["wiretest/v1.proto", "wiretest/v2.proto"],
        &["tests/proto"],
        &[],
    )?);
    read.extend(codegen::compile_into(
        &out_dir.join("ext"),
        &["wiretest/ext.proto"],
        &["tests/proto"],
        &[(".wiretest.v1", "crate::schema::wiretest::v1")],
    )?);
    for path in read {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    use prost::Message;
    let descriptors = prost_types::FileDescriptorSet::decode(
        std::fs::read(out_dir.join("descriptors.bin"))?.as_slice(),
    )?;
    let monitor = contract_id::fingerprint(
        &descriptors,
        &["newport/monitor.proto", "newport/discovery.proto"],
    )?;
    let desktop = contract_id::fingerprint(&descriptors, &["newport/desktop.proto"])?;
    std::fs::write(
        out_dir.join("contract_ids.rs"),
        format!("pub const MONITOR: &str = {monitor:?};\npub const DESKTOP: &str = {desktop:?};\n"),
    )?;
    println!("cargo:rerun-if-changed=contract_id.rs");
    Ok(())
}
