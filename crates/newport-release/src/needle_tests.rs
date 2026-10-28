use super::*;
use crate::needle::{Envelope, MANIFEST_FILENAME, VerifiedRelease};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;

fn signed(pair: &KeyPair, version: &str, marker: &str) -> (Vec<u8>, VerifiedRelease, String) {
    let directory = tempfile::tempdir().unwrap();
    let bytes = format!("\x7fELF{marker}");
    for arch in ["x86_64", "aarch64"] {
        fs::write(
            directory.path().join(format!("newport-agent-{arch}")),
            &bytes,
        )
        .unwrap();
    }
    let path = sign_needle(directory.path(), version, &public(pair), &pair.sk).unwrap();
    let envelope = fs::read(path).unwrap();
    let release = crate::needle::verify(&envelope, &public(pair)).unwrap();
    let hash = Sha256::digest(bytes.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    (envelope, release, hash)
}

#[test]
fn ordering_requires_signed_platform_and_exact_build_identity() {
    let pair = KeyPair::generate_unencrypted_keypair().unwrap();
    let (_, old, old_hash) = signed(&pair, "1.9.0", "old");
    let (_, new, new_hash) = signed(&pair, "1.10.0", "new");
    assert_eq!(
        old.compare_builds(&old_hash, &new, &new_hash, "x86_64"),
        Some(Ordering::Less)
    );
    assert_eq!(
        new.compare_builds(&new_hash, &old, &old_hash, "aarch64"),
        Some(Ordering::Greater)
    );
    assert_eq!(
        new.compare_builds(&new_hash, &new, &new_hash, "arm64"),
        Some(Ordering::Equal)
    );
    assert_eq!(
        old.compare_builds(&new_hash, &new, &new_hash, "x86_64"),
        None
    );
    assert_eq!(
        old.compare_builds(&old_hash, &new, &old_hash, "x86_64"),
        None
    );
    assert!(old.version_for("riscv64", &old_hash).is_none());
    let (_, pre, pre_hash) = signed(&pair, "1.10.0-rc.1", "pre");
    assert_eq!(
        pre.compare_builds(&pre_hash, &new, &new_hash, "x86_64"),
        Some(Ordering::Less)
    );
    let (_, build, build_hash) = signed(&pair, "1.10.0+other", "other");
    assert_eq!(
        build.compare_builds(&build_hash, &new, &new_hash, "x86_64"),
        Some(Ordering::Equal)
    );
}

fn resign(
    pair: &KeyPair,
    envelope: &[u8],
    change: impl FnOnce(&mut serde_json::Value),
    version: &str,
) -> Vec<u8> {
    let mut envelope: Envelope = serde_json::from_slice(envelope).unwrap();
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&STANDARD.decode(&envelope.manifest).unwrap()).unwrap();
    change(&mut manifest);
    let bytes = serde_json::to_vec(&manifest).unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join(MANIFEST_FILENAME);
    fs::write(&path, &bytes).unwrap();
    envelope.signature =
        fs::read_to_string(sign(&path, version, &public(pair), &pair.sk).unwrap()).unwrap();
    envelope.manifest = STANDARD.encode(bytes);
    serde_json::to_vec(&envelope).unwrap()
}

#[test]
fn tampering_wrong_key_wrong_product_and_ambiguous_metadata_are_rejected() {
    let pair = KeyPair::generate_unencrypted_keypair().unwrap();
    let (bytes, _, _) = signed(&pair, "1.2.3", "build");
    let other = KeyPair::generate_unencrypted_keypair().unwrap();
    assert!(crate::needle::verify(&bytes, &public(&other)).is_err());
    let mut tampered: Envelope = serde_json::from_slice(&bytes).unwrap();
    let manifest = String::from_utf8(STANDARD.decode(&tampered.manifest).unwrap()).unwrap();
    tampered.manifest = STANDARD.encode(manifest.replace("1.2.3", "9.0.0"));
    assert!(
        crate::needle::verify(&serde_json::to_vec(&tampered).unwrap(), &public(&pair)).is_err()
    );
    for field in ["schemaVersion", "product", "version", "builds"] {
        let invalid = resign(
            &pair,
            &bytes,
            |m| m[field] = serde_json::json!("invalid"),
            "1.2.3",
        );
        assert!(
            crate::needle::verify(&invalid, &public(&pair)).is_err(),
            "{field}"
        );
    }
    let invalid = resign(
        &pair,
        &bytes,
        |m| m["builds"]["x86_64"] = serde_json::json!("A".repeat(64)),
        "1.2.3",
    );
    assert!(crate::needle::verify(&invalid, &public(&pair)).is_err());
    let invalid = resign(&pair, &bytes, |_| {}, "1.2.4");
    assert!(crate::needle::verify(&invalid, &public(&pair)).is_err());
    let invalid = resign(
        &pair,
        &bytes,
        |m| m["extra"] = serde_json::json!(true),
        "1.2.3",
    );
    assert!(crate::needle::verify(&invalid, &public(&pair)).is_err());
    let oversized = vec![b' '; crate::needle::ENVELOPE_LIMIT + 1];
    assert!(crate::needle::verify(&oversized, &public(&pair)).is_err());
    let duplicate = String::from_utf8(bytes)
        .unwrap()
        .replacen("{", "{\"manifest\":\"\",", 1);
    assert!(crate::needle::verify(duplicate.as_bytes(), &public(&pair)).is_err());
}

#[test]
fn swapped_platform_hashes_do_not_authorize_another_executable() {
    let pair = KeyPair::generate_unencrypted_keypair().unwrap();
    let (bytes, _, hash) = signed(&pair, "1.2.3", "build");
    let signed = resign(
        &pair,
        &bytes,
        |m| m["builds"]["aarch64"] = serde_json::json!("0".repeat(64)),
        "1.2.3",
    );
    let release = crate::needle::verify(&signed, &public(&pair)).unwrap();
    assert!(release.version_for("x86_64", &hash).is_some());
    assert!(release.version_for("aarch64", &hash).is_none());
}

#[test]
fn failed_signing_preserves_previous_release_identity() {
    let pair = KeyPair::generate_unencrypted_keypair().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join(crate::needle::FILENAME);
    fs::write(&output, b"previous").unwrap();
    fs::write(
        directory.path().join("newport-agent-x86_64"),
        b"\x7fELFfixture",
    )
    .unwrap();
    assert!(sign_needle(directory.path(), "1.2.3", &public(&pair), &pair.sk).is_err());
    fs::write(
        directory.path().join("newport-agent-aarch64"),
        b"\x7fELFfixture",
    )
    .unwrap();
    assert!(sign_needle(directory.path(), "bad version", &public(&pair), &pair.sk).is_err());
    assert_eq!(fs::read(output).unwrap(), b"previous");
}

#[test]
fn schema_two_binds_variant_and_compiled_protocols_and_preserves_legacy_core() {
    use crate::agent_product::{CORE_SERVICES, Product, Variant};
    let pair = KeyPair::generate_unencrypted_keypair().unwrap();
    let (legacy, core, hash) = signed(&pair, "1.2.3", "build");
    assert_eq!(core.variant_for("x86_64", &hash), Some(Variant::Core));
    assert!(core.product_for("x86_64", &hash).is_none());
    let mut product = Product {
        variant: Variant::Full,
        control_versions: vec![1],
        services: CORE_SERVICES
            .iter()
            .map(|name| (name.to_string(), vec![1]))
            .collect(),
    };
    product.services.insert("remote-apps".into(), vec![1, 2]);
    let full = resign(
        &pair,
        &legacy,
        |m| {
            m["schemaVersion"] = 2.into();
            m["agent"] = serde_json::to_value(&product).unwrap();
        },
        "1.2.3",
    );
    let release = crate::needle::verify(&full, &public(&pair)).unwrap();
    assert_eq!(release.variant_for("arm64", &hash), Some(Variant::Full));
    assert_eq!(release.product_for("x86_64", &hash), Some(&product));
    assert_eq!(release.variant_for("x86_64", &"0".repeat(64)), None);
    for change in [
        serde_json::json!({"schemaVersion": 1}),
        serde_json::json!({"schemaVersion": 3}),
        serde_json::json!({"agent": null}),
        serde_json::json!({"agent": {"variant": "unknown", "controlVersions": [1], "services": {}}}),
    ] {
        let invalid = resign(
            &pair,
            &full,
            |m| {
                for (field, value) in change.as_object().unwrap() {
                    m[field] = value.clone();
                }
            },
            "1.2.3",
        );
        assert!(crate::needle::verify(&invalid, &public(&pair)).is_err());
    }
    // The v1 manifest shape still omits new fields, so previously released
    // strict readers can keep validating core releases without schema changes.
    let envelope: Envelope = serde_json::from_slice(&legacy).unwrap();
    let manifest: serde_json::Value =
        serde_json::from_slice(&STANDARD.decode(envelope.manifest).unwrap()).unwrap();
    assert!(manifest.get("agent").is_none());
    let directory = tempfile::tempdir().unwrap();
    for arch in ["x86_64", "aarch64"] {
        fs::write(
            directory.path().join(format!("newport-agent-{arch}")),
            b"\x7fELFfull",
        )
        .unwrap();
    }
    let output = crate::sign_agent(
        directory.path(),
        "1.2.3",
        &public(&pair),
        &pair.sk,
        product.clone(),
    )
    .unwrap();
    let signed = crate::needle::verify(&fs::read(output).unwrap(), &public(&pair)).unwrap();
    let hash = Sha256::digest(b"\x7fELFfull")
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    assert_eq!(signed.product_for("x86_64", &hash), Some(&product));
}
