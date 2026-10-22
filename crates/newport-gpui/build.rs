//! Remote apps streaming needs private Newport Remote crates, which only full
//! builds add. Those builds set NEWPORT_REMOTE_APPS=1 to compile it in, and
//! name the compiled agent archives that Install sends to servers:
//! NEWPORT_REMOTE_AGENT_X86_64 and NEWPORT_REMOTE_AGENT_AARCH64
//! (`cargo xtask dev --remote-apps` and `build --remote-apps` make them).
fn main() {
    println!("cargo::rustc-check-cfg=cfg(newport_remote_apps)");
    println!("cargo::rerun-if-env-changed=NEWPORT_REMOTE_APPS");
    if std::env::var_os("NEWPORT_REMOTE_APPS").is_none_or(|value| value != "1") {
        return;
    }
    for variable in [
        "NEWPORT_REMOTE_AGENT_X86_64",
        "NEWPORT_REMOTE_AGENT_AARCH64",
    ] {
        println!("cargo::rerun-if-env-changed={variable}");
        let path = std::env::var(variable).unwrap_or_default();
        if !std::path::Path::new(&path).is_file() {
            panic!(
                "{variable} must name a compiled agent archive; build with cargo xtask build --remote-apps"
            );
        }
        println!("cargo::rerun-if-changed={path}");
    }
    println!("cargo::rustc-cfg=newport_remote_apps");
}
