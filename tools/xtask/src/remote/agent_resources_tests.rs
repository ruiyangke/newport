use super::*;
use newport_protocol::product::{CORE_SERVICES, Identity, Product, VERSION, Variant};
use std::collections::BTreeMap;

const ARCHITECTURES: [&str; 2] = ["x86_64", "aarch64"];

fn fixture_agent(directory: &Path, arch: &str, contents: &[u8]) -> Result<Identity> {
    let mut binary = vec![0; 64];
    binary[..6].copy_from_slice(b"\x7fELF\x02\x01");
    let machine: u16 = if arch == "x86_64" { 62 } else { 183 };
    binary[18..20].copy_from_slice(&machine.to_le_bytes());
    binary.extend_from_slice(contents);
    let identity = Identity {
        schema_version: VERSION,
        build_sha256: crate::util::sha256_hex(&binary),
        version: None,
        product: Product {
            variant: Variant::Core,
            control_versions: vec![1],
            services: CORE_SERVICES
                .iter()
                .map(|name| ((*name).to_owned(), vec![1]))
                .collect(),
        },
    };
    fs::write(directory.join(format!("newport-agent-{arch}")), binary)?;
    write_identity(directory, arch, &identity)?;
    Ok(identity)
}

fn write_identity(directory: &Path, arch: &str, identity: &Identity) -> Result<()> {
    fs::write(
        directory.join(format!("newport-agent-{arch}.product.json")),
        serde_json::to_vec(identity)?,
    )?;
    Ok(())
}

fn snapshot(directory: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    fs::read_dir(directory)?
        .map(|entry| {
            let entry = entry?;
            Ok((
                entry.file_name().to_string_lossy().into_owned(),
                fs::read(entry.path())?,
            ))
        })
        .collect()
}

fn existing_agents(directory: &Path) -> Result<()> {
    for arch in ARCHITECTURES {
        fixture_agent(directory, arch, b"accepted")?;
    }
    fs::write(
        directory.join("needle-release.json"),
        b"accepted release metadata",
    )?;
    Ok(())
}

#[test]
fn core_publication_preserves_the_emitted_identity_bytes() -> Result<()> {
    let staging = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    existing_agents(output.path())?;
    for arch in ARCHITECTURES {
        fixture_agent(staging.path(), arch, b"rebuilt")?;
    }
    let emitted = snapshot(staging.path())?;

    publish_agents(staging.path(), output.path(), &ARCHITECTURES)?;

    assert_eq!(snapshot(output.path())?, emitted);
    assert!(snapshot(staging.path())?.is_empty());
    Ok(())
}

#[test]
fn single_architecture_publication_leaves_the_other_pair_unchanged() -> Result<()> {
    for selected in ARCHITECTURES {
        for populated in [false, true] {
            let staging = tempfile::tempdir()?;
            let output = tempfile::tempdir()?;
            if populated {
                existing_agents(output.path())?;
            }
            let mut expected = snapshot(output.path())?;
            fixture_agent(staging.path(), selected, b"rebuilt")?;
            expected.extend(snapshot(staging.path())?);
            expected.remove("needle-release.json");

            publish_agents(staging.path(), output.path(), &[selected])?;

            assert_eq!(snapshot(output.path())?, expected);
        }
    }
    Ok(())
}

#[test]
fn invalid_second_architecture_preserves_all_accepted_resources() -> Result<()> {
    for defect in [
        "missing",
        "json",
        "schema",
        "hash",
        "full",
        "elf",
        "architecture",
        "protocols",
    ] {
        let staging = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        existing_agents(output.path())?;
        let accepted = snapshot(output.path())?;
        fixture_agent(staging.path(), "x86_64", b"rebuilt")?;
        let mut identity = fixture_agent(staging.path(), "aarch64", b"rebuilt")?;
        let identity_path = staging.path().join("newport-agent-aarch64.product.json");
        match defect {
            "missing" => fs::remove_file(identity_path)?,
            "json" => fs::write(identity_path, b"not JSON")?,
            "schema" => {
                identity.schema_version = VERSION + 1;
                write_identity(staging.path(), "aarch64", &identity)?;
            }
            "hash" => {
                identity.build_sha256 = "0".repeat(64);
                write_identity(staging.path(), "aarch64", &identity)?;
            }
            "full" => {
                identity.product.variant = Variant::Full;
                identity
                    .product
                    .services
                    .insert("remote-apps".into(), vec![1]);
                identity.validate().unwrap();
                write_identity(staging.path(), "aarch64", &identity)?;
            }
            "elf" => {
                let binary = b"not an ELF";
                fs::write(staging.path().join("newport-agent-aarch64"), binary)?;
                identity.build_sha256 = crate::util::sha256_hex(binary);
                write_identity(staging.path(), "aarch64", &identity)?;
            }
            "architecture" => {
                let path = staging.path().join("newport-agent-aarch64");
                let mut binary = fs::read(&path)?;
                binary[18..20].copy_from_slice(&62_u16.to_le_bytes());
                identity.build_sha256 = crate::util::sha256_hex(&binary);
                fs::write(path, binary)?;
                write_identity(staging.path(), "aarch64", &identity)?;
            }
            "protocols" => {
                identity.product.control_versions = vec![2];
                identity.validate().unwrap();
                write_identity(staging.path(), "aarch64", &identity)?;
            }
            _ => unreachable!(),
        }

        assert!(
            publish_agents(staging.path(), output.path(), &ARCHITECTURES).is_err(),
            "{defect}"
        );
        assert_eq!(snapshot(output.path())?, accepted, "{defect}");
        assert!(staging.path().join("newport-agent-x86_64").is_file());
    }
    Ok(())
}
