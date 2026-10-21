fn main() {
    use sha2::{Digest, Sha256};
    use std::{fs, io::Write, path::Path};
    let metadata = "../../resources/agents/needle-release.json";
    // Development builds have no signed metadata; a missing watched file would
    // rerun this script on every build, so watch the directory it appears in.
    if Path::new(metadata).exists() {
        println!("cargo:rerun-if-changed={metadata}");
    } else {
        println!("cargo:rerun-if-changed=../../resources/agents");
    }
    println!("cargo:rerun-if-changed=../../resources/updater.json");
    println!("cargo:rerun-if-changed=../../resources/app.json");
    println!("cargo:rerun-if-env-changed=NEWPORT_REQUIRE_NEEDLE_RELEASE");
    let envelope = match fs::read(metadata) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => panic!("Cannot read Needle release metadata: {error}"),
    };
    assert!(
        !envelope.is_empty() || std::env::var_os("NEWPORT_REQUIRE_NEEDLE_RELEASE").is_none(),
        "Distribution builds require signed Needle release metadata"
    );
    let release = (!envelope.is_empty()).then(|| {
        newport_release::needle::verify_production(&envelope)
            .expect("Bundled Needle release metadata must have a valid vendor signature")
    });
    let product: serde_json::Value = serde_json::from_slice(
        &fs::read("../../resources/app.json").expect("Missing app metadata"),
    )
    .expect("Invalid app metadata");
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    // OUT_DIR is <target>/<profile>/build/<package>/out, including cross builds.
    let resources = out.ancestors().nth(3).unwrap().join("agents");
    fs::create_dir_all(&resources).expect("Cannot stage Needle resources");
    let mut descriptors = String::new();
    for arch in ["x86_64", "aarch64"] {
        let path = format!("../../resources/agents/newport-agent-{arch}");
        assert!(
            std::path::Path::new(&path).is_file(),
            "Run cargo xtask agents, or download the verified linux-agents artifact into resources/agents before building the backend."
        );
        println!("cargo:rerun-if-changed={path}");
        let bytes = fs::read(&path).expect("Cannot read Needle payload");
        let hash: String = Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let mut encoder = flate2::GzBuilder::new()
            .mtime(0)
            .write(Vec::new(), flate2::Compression::best());
        encoder.write_all(&bytes).expect("Cannot compress Needle");
        let compressed = encoder.finish().expect("Cannot finish Needle compression");
        let name = format!("newport-agent-{arch}.gz");
        let destination = resources.join(&name);
        // Recreate resources if a cache restore retained Cargo outputs but omitted these files.
        println!("cargo:rerun-if-changed={}", destination.display());
        if fs::read(&destination).ok().as_deref() != Some(compressed.as_slice()) {
            let temporary = resources.join(format!(".{name}.{}", std::process::id()));
            fs::write(&temporary, &compressed).expect("Cannot stage compressed Needle");
            fs::rename(temporary, destination).expect("Cannot publish compressed Needle");
        }
        descriptors.push_str(&format!(
            "pub(super) const {}: Payload = Payload {{ file: {:?}, size: {}, sha256: {:?} }};\n",
            arch.to_uppercase(),
            name,
            bytes.len(),
            hash
        ));
        if let Some(release) = &release {
            let version = release
                .version_for(arch, &hash)
                .expect("Needle release metadata does not match the bundled payload");
            assert_eq!(
                Some(version.to_string().as_str()),
                product["version"].as_str(),
                "Needle release metadata does not match the desktop version"
            );
        }
    }
    fs::write(out.join("needle-payloads.rs"), descriptors)
        .expect("Cannot record Needle descriptors");
    fs::write(
        Path::new(&std::env::var_os("OUT_DIR").unwrap()).join("needle-release.json"),
        envelope,
    )
    .expect("Cannot embed Needle release metadata");
}
