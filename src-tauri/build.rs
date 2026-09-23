fn main() {
    for arch in ["x86_64", "aarch64"] {
        let path = format!("agents/porthop-agent-{arch}");
        assert!(
            std::path::Path::new(&path).is_file(),
            "Run npm run build:agent before building Porthop."
        );
        println!("cargo:rerun-if-changed={path}");
    }
    tauri_build::build()
}
