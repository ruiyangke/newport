//! Remote Apps requires private client crates and the full shared agent product.
fn main() {
    println!("cargo::rustc-check-cfg=cfg(newport_remote_apps)");
    println!("cargo::rerun-if-env-changed=NEWPORT_REMOTE_APPS");
    println!("cargo::rerun-if-env-changed=NEWPORT_AGENT_RESOURCE_DIR");
    if std::env::var_os("NEWPORT_REMOTE_APPS").is_none_or(|value| value != "1") {
        return;
    }
    let resources = std::path::PathBuf::from(
        std::env::var_os("NEWPORT_AGENT_RESOURCE_DIR")
            .expect("Remote Apps builds require the full NEWPORT_AGENT_RESOURCE_DIR export"),
    );
    for arch in ["x86_64", "aarch64"] {
        for name in [
            format!("newport-agent-{arch}"),
            format!("newport-agent-{arch}.product.json"),
        ] {
            let path = resources.join(name);
            assert!(
                path.is_file(),
                "Missing full agent resource: {}",
                path.display()
            );
            println!("cargo::rerun-if-changed={}", path.display());
        }
    }
    // newport-backend verifies identities/hashes and requires full remote-apps v1.
    println!("cargo::rustc-cfg=newport_remote_apps");
}
