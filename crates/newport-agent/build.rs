#[path = "src/version_source.rs"]
mod version_source;

fn main() {
    let metadata =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../resources/app.json");
    println!("cargo:rerun-if-changed={}", metadata.display());
    // Cargo compares the value, not a modification time.
    println!("cargo:rerun-if-env-changed=NEWPORT_AGENT_VERSION");
    let from_metadata = std::fs::read(&metadata).ok().map(|bytes| {
        let app: serde_json::Value =
            serde_json::from_slice(&bytes).expect("Invalid Newport product metadata");
        app["version"]
            .as_str()
            .expect("Missing Newport product version")
            .to_owned()
    });
    let from_env = std::env::var("NEWPORT_AGENT_VERSION").ok();
    let version = version_source::resolve(from_env.as_deref(), from_metadata.as_deref())
        .unwrap_or_else(|error| panic!("{error}"));
    semver::Version::parse(&version).expect("Newport product version must be semantic");
    println!("cargo:rustc-env=NEWPORT_AGENT_VERSION={version}");
}
