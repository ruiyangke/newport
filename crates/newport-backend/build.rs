fn main() {
    use sha2::{Digest, Sha256};
    use std::{fs, io::Write, path::Path};
    println!("cargo:rerun-if-env-changed=NEWPORT_AGENT_RESOURCE_DIR");
    println!("cargo:rerun-if-env-changed=NEWPORT_REMOTE_APPS");
    let external = std::env::var_os("NEWPORT_AGENT_RESOURCE_DIR");
    let directory = external
        .as_ref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "../../resources/agents".into());
    let metadata = directory.join("needle-release.json");
    // Development builds have no signed metadata; a missing watched file would
    // rerun this script on every build, so watch the directory it appears in.
    if metadata.exists() {
        println!("cargo:rerun-if-changed={}", metadata.display());
    } else {
        println!("cargo:rerun-if-changed={}", directory.display());
    }
    println!("cargo:rerun-if-changed=../../resources/updater.json");
    println!("cargo:rerun-if-changed=../../resources/app.json");
    println!("cargo:rerun-if-env-changed=NEWPORT_REQUIRE_NEEDLE_RELEASE");
    let envelope = match fs::read(&metadata) {
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
    let mut compiled_product = None;
    let mut identity_count = 0;
    for arch in ["x86_64", "aarch64"] {
        let path = directory.join(format!("newport-agent-{arch}"));
        assert!(
            path.is_file(),
            "Run cargo xtask agents, or download the verified linux-agents artifact into resources/agents before building the backend."
        );
        println!("cargo:rerun-if-changed={}", path.display());
        let bytes = fs::read(&path).expect("Cannot read Needle payload");
        let hash: String = Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let identity_path = directory.join(format!("newport-agent-{arch}.product.json"));
        if identity_path.exists() {
            println!("cargo:rerun-if-changed={}", identity_path.display());
            let identity: newport_release::agent_product::Identity = serde_json::from_slice(
                &fs::read(&identity_path).expect("Cannot read agent product identity"),
            )
            .expect("Invalid bundled agent product identity");
            identity
                .validate()
                .expect("Unsupported bundled agent product identity");
            assert_eq!(
                identity.build_sha256, hash,
                "Agent product identity must match its exact payload"
            );
            assert_eq!(
                compiled_product.get_or_insert(identity.product.clone()),
                &identity.product,
                "Both architectures must contain the same compiled product protocols"
            );
            identity_count += 1;
        } else {
            assert!(
                external.is_none(),
                "External agent resources require product identities for both architectures"
            );
        }
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
            "pub(super) const {}: Payload = Payload {{ file: {:?}, size: {}, compressed_size: {}, sha256: {:?} }};\n",
            arch.to_uppercase(),
            name,
            bytes.len(),
            compressed.len(),
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
            let variant = compiled_product
                .as_ref()
                .map_or(newport_release::agent_product::Variant::Core, |p| p.variant);
            assert_eq!(
                release.variant_for(arch, &hash),
                Some(variant),
                "Signed agent variant does not match the payload product"
            );
            if let Some(signed) = release.product_for(arch, &hash) {
                assert_eq!(
                    compiled_product.as_ref(),
                    Some(signed),
                    "Signed protocols require matching payload product identities"
                );
            }
        }
    }
    assert!(
        identity_count == 0 || identity_count == 2,
        "Agent product identities must cover both architectures"
    );
    if std::env::var("NEWPORT_REMOTE_APPS").as_deref() == Ok("1") {
        assert!(
            compiled_product
                .as_ref()
                .is_some_and(|product| product.variant
                    == newport_release::agent_product::Variant::Full
                    && product
                        .services
                        .get("remote-apps")
                        .is_some_and(|versions| versions.contains(&1))),
            "Remote Apps desktop builds must bundle the full agent with remote-apps v1"
        );
    }
    let variant =
        compiled_product.map_or(newport_release::agent_product::Variant::Core, |p| p.variant);
    let notices = resources.join("licenses");
    if notices.exists() {
        fs::remove_dir_all(&notices).expect("Cannot replace staged agent notices");
    }
    if variant == newport_release::agent_product::Variant::Full {
        let source = directory.join("licenses");
        for name in ["newport-core.txt", "newport-full.txt"] {
            assert!(
                source.join(name).is_file(),
                "Full agent export is missing {name}"
            );
        }
        stage_notices(&source, &notices);
    }
    descriptors.push_str(&format!("pub(super) const BUNDLED_VARIANT: newport_release::agent_product::Variant = newport_release::agent_product::Variant::{variant:?};\n"));
    fs::write(out.join("needle-payloads.rs"), descriptors)
        .expect("Cannot record Needle descriptors");
    fs::write(
        Path::new(&std::env::var_os("OUT_DIR").unwrap()).join("needle-release.json"),
        envelope,
    )
    .expect("Cannot embed Needle release metadata");
}

fn stage_notices(source: &std::path::Path, destination: &std::path::Path) {
    use std::fs;
    println!("cargo:rerun-if-changed={}", source.display());
    fs::create_dir_all(destination).expect("Cannot stage agent notices");
    for entry in fs::read_dir(source).expect("Cannot read agent notices") {
        let entry = entry.expect("Cannot read agent notice");
        let kind = entry.file_type().expect("Cannot inspect agent notice");
        let target = destination.join(entry.file_name());
        if kind.is_dir() {
            stage_notices(&entry.path(), &target);
        } else {
            assert!(kind.is_file(), "Agent notices must be regular files");
            fs::copy(entry.path(), target).expect("Cannot copy agent notice");
        }
    }
}
