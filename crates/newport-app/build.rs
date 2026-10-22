#[path = "build_support/windows.rs"]
mod windows;

fn main() {
    use std::{env, fs, path::PathBuf};
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../..");
    let metadata = root.join("resources/app.json");
    let icon = root.join("resources/icons/icon.ico");
    for path in ["build.rs", "build_support/windows.rs"] {
        println!("cargo:rerun-if-changed={path}");
    }
    println!("cargo:rerun-if-changed={}", metadata.display());
    println!("cargo:rerun-if-changed={}", icon.display());
    let product: serde_json::Value = serde_json::from_slice(&fs::read(metadata).unwrap()).unwrap();
    let version = env::var("CARGO_PKG_VERSION").unwrap();
    assert_eq!(
        product["version"].as_str(),
        Some(version.as_str()),
        "Application and product versions differ"
    );
    assert_eq!(product["productName"], "Newport");
    assert_eq!(product["identifier"], "app.newport");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let resource = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("newport.rc");
    fs::write(
        &resource,
        windows::resource_script("Newport", &version, &icon).unwrap(),
    )
    .unwrap();
    embed_resource::compile_for(resource, ["newport"], embed_resource::NONE)
        .manifest_required()
        .expect("Newport's Windows icon/version resources must compile");
}
