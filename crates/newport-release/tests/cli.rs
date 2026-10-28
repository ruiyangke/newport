use base64::{Engine, engine::general_purpose::STANDARD};
use minisign::KeyPair;
use std::{fs, process::Command};

struct AgentFixture {
    _root: tempfile::TempDir,
    directory: std::path::PathBuf,
    public: std::path::PathBuf,
    secret: std::path::PathBuf,
    encoded: String,
}

impl AgentFixture {
    fn new() -> Self {
        use newport_release::agent_product::{CORE_SERVICES, Product, Variant};
        use sha2::{Digest, Sha256};
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("agents");
        fs::create_dir(&directory).unwrap();
        let product = Product {
            variant: Variant::Full,
            control_versions: vec![1],
            services: CORE_SERVICES
                .iter()
                .chain([&"remote-apps"])
                .map(|name| (name.to_string(), vec![1]))
                .collect(),
        };
        for (arch, machine) in [("x86_64", 62u16), ("aarch64", 183)] {
            // Only an ELF architecture/header fixture, never an executable or
            // an assertion of static-link/build provenance qualification.
            let mut bytes = vec![0u8; 64];
            bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
            bytes[16..18].copy_from_slice(&2u16.to_le_bytes());
            bytes[18..20].copy_from_slice(&machine.to_le_bytes());
            bytes[20..24].copy_from_slice(&1u32.to_le_bytes());
            bytes[52..54].copy_from_slice(&64u16.to_le_bytes());
            bytes.extend_from_slice(arch.as_bytes());
            let identity = newport_release::agent_product::Identity {
                schema_version: 1,
                build_sha256: Sha256::digest(&bytes)
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect(),
                product: product.clone(),
            };
            fs::write(directory.join(format!("newport-agent-{arch}")), bytes).unwrap();
            fs::write(
                directory.join(format!("newport-agent-{arch}.product.json")),
                serde_json::to_vec(&identity).unwrap(),
            )
            .unwrap();
        }
        let pair = KeyPair::generate_unencrypted_keypair().unwrap();
        let public = root.path().join("public.key");
        let secret = root.path().join("secret.key");
        let encoded = STANDARD.encode(pair.sk.to_box(None).unwrap().to_string());
        fs::write(
            &public,
            STANDARD.encode(pair.pk.to_box().unwrap().to_string()),
        )
        .unwrap();
        fs::write(&secret, &encoded).unwrap();
        Self {
            _root: root,
            directory,
            public,
            secret,
            encoded,
        }
    }

    fn path(&self, name: &str) -> std::path::PathBuf {
        self.directory.join(name)
    }

    fn envelope(&self) -> std::path::PathBuf {
        self.path(newport_release::needle::FILENAME)
    }

    fn command(&self, action: &str, version: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_newport-release"));
        command
            .arg(action)
            .arg(&self.directory)
            .arg(version)
            .arg(&self.public)
            .env_remove("NEWPORT_SIGNING_PRIVATE_KEY")
            .env_remove("NEWPORT_SIGNING_PRIVATE_KEY_PATH")
            .env_remove("NEWPORT_SIGNING_PRIVATE_KEY_PASSWORD");
        if action != "verify-agent" {
            command.env("NEWPORT_SIGNING_PRIVATE_KEY_PATH", &self.secret);
        }
        command
    }

    fn run(&self, action: &str) -> std::process::Output {
        self.command(action, "1.2.3").output().unwrap()
    }

    fn success(&self, output: &std::process::Output) {
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains(&self.encoded));
        assert!(!String::from_utf8_lossy(&output.stderr).contains(&self.encoded));
    }

    fn edit_identity(&self, arch: &str, edit: impl FnOnce(&mut serde_json::Value)) {
        let path = self.path(&format!("newport-agent-{arch}.product.json"));
        let mut value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        edit(&mut value);
        fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    }

    fn rehash_identity(&self, arch: &str) {
        use sha2::{Digest, Sha256};
        let bytes = fs::read(self.path(&format!("newport-agent-{arch}"))).unwrap();
        let hash: String = Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        self.edit_identity(arch, |identity| identity["buildSha256"] = hash.into());
    }

    fn preserves_previous(&self, command: &mut Command) -> String {
        let before = fs::read(self.envelope()).unwrap();
        let output = command.output().unwrap();
        assert!(!output.status.success(), "unexpected successful command");
        assert!(!String::from_utf8_lossy(&output.stdout).contains(&self.encoded));
        assert!(!String::from_utf8_lossy(&output.stderr).contains(&self.encoded));
        assert_eq!(fs::read(self.envelope()).unwrap(), before);
        String::from_utf8(output.stderr).unwrap()
    }
}

#[test]
fn cli_signs_from_contents_or_a_path_and_rejects_wrong_keys_without_replacing_sidecars() {
    let temp = tempfile::tempdir().unwrap();
    let pair = KeyPair::generate_unencrypted_keypair().unwrap();
    let encoded = STANDARD.encode(pair.sk.to_box(None).unwrap().to_string());
    let key_path = temp.path().join("secret.key");
    let public_path = temp.path().join("public.key");
    let artifact = temp.path().join("Newport fixture.exe");
    fs::write(&key_path, &encoded).unwrap();
    fs::write(
        &public_path,
        STANDARD.encode(pair.pk.to_box().unwrap().to_string()),
    )
    .unwrap();
    fs::write(&artifact, b"test release").unwrap();
    let invoke = |from_file: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_newport-release"));
        command.args([
            "sign".as_ref(),
            artifact.as_os_str(),
            "1.2.3".as_ref(),
            public_path.as_os_str(),
        ]);
        command
            .env_remove("NEWPORT_SIGNING_PRIVATE_KEY")
            .env_remove("NEWPORT_SIGNING_PRIVATE_KEY_PATH");
        command.env("NEWPORT_SIGNING_PRIVATE_KEY_PASSWORD", "");
        if from_file {
            command.env("NEWPORT_SIGNING_PRIVATE_KEY_PATH", &key_path);
        } else {
            command.env("NEWPORT_SIGNING_PRIVATE_KEY", &encoded);
        }
        command.output().unwrap()
    };
    for from_file in [false, true] {
        let output = invoke(from_file);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains(&encoded));
        let signature = String::from_utf8(
            STANDARD
                .decode(fs::read(artifact.with_added_extension("sig")).unwrap())
                .unwrap(),
        )
        .unwrap();
        let signature = minisign_verify::Signature::decode(&signature).unwrap();
        let public =
            minisign_verify::PublicKey::decode(&pair.pk.to_box().unwrap().to_string()).unwrap();
        public.verify(b"test release", &signature, true).unwrap();
    }
    let before = fs::read(artifact.with_added_extension("sig")).unwrap();
    let other = KeyPair::generate_unencrypted_keypair().unwrap();
    fs::write(
        &public_path,
        STANDARD.encode(other.pk.to_box().unwrap().to_string()),
    )
    .unwrap();
    let result = invoke(false);
    assert!(!result.status.success());
    assert!(!String::from_utf8_lossy(&result.stderr).contains(&encoded));
    assert_eq!(
        fs::read(artifact.with_added_extension("sig")).unwrap(),
        before
    );
}

#[test]
fn full_agent_cli_signs_exact_products_and_verifies_without_loading_private_keys() {
    let f = AgentFixture::new();
    let before: Vec<_> = ["x86_64", "aarch64"]
        .into_iter()
        .flat_map(|arch| {
            [
                format!("newport-agent-{arch}"),
                format!("newport-agent-{arch}.product.json"),
            ]
        })
        .map(|name| (name.clone(), fs::read(f.path(&name)).unwrap()))
        .collect();
    f.success(&f.run("agent"));
    let envelope = fs::read(f.envelope()).unwrap();
    let raw: newport_release::needle::Envelope = serde_json::from_slice(&envelope).unwrap();
    let manifest: newport_release::needle::Manifest =
        serde_json::from_slice(&STANDARD.decode(raw.manifest).unwrap()).unwrap();
    assert_eq!(manifest.schema_version, 2);
    assert_eq!(
        manifest.agent.as_ref().unwrap().variant,
        newport_release::agent_product::Variant::Full
    );
    let release =
        newport_release::needle::verify(&envelope, &fs::read_to_string(&f.public).unwrap())
            .unwrap();
    for arch in ["x86_64", "aarch64"] {
        let identity: newport_release::agent_product::Identity = serde_json::from_slice(
            &fs::read(f.path(&format!("newport-agent-{arch}.product.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(
            release.product_for(arch, &identity.build_sha256),
            Some(&identity.product)
        );
        assert_eq!(
            release
                .version_for(arch, &identity.build_sha256)
                .unwrap()
                .to_string(),
            "1.2.3"
        );
    }
    f.success(&f.run("verify-agent"));
    // Both conflicting, invalid sources must be ignored, not merely left unused
    // after successful key loading. The nonexistent path must never be opened.
    let output = f
        .command("verify-agent", "1.2.3")
        .env("NEWPORT_SIGNING_PRIVATE_KEY", "invalid-secret-fixture")
        .env(
            "NEWPORT_SIGNING_PRIVATE_KEY_PATH",
            f.path("nonexistent-private-key"),
        )
        .env(
            "NEWPORT_SIGNING_PRIVATE_KEY_PASSWORD",
            "invalid-password-fixture",
        )
        .output()
        .unwrap();
    f.success(&output);
    assert_eq!(fs::read(f.envelope()).unwrap(), envelope);
    for (name, bytes) in before {
        assert_eq!(fs::read(f.path(&name)).unwrap(), bytes);
    }
}

#[test]
fn legacy_needle_cli_keeps_schema_one_without_product_sidecars() {
    let f = AgentFixture::new();
    for arch in ["x86_64", "aarch64"] {
        fs::remove_file(f.path(&format!("newport-agent-{arch}.product.json"))).unwrap();
    }
    f.success(&f.run("needle"));
    let raw: newport_release::needle::Envelope =
        serde_json::from_slice(&fs::read(f.envelope()).unwrap()).unwrap();
    let manifest: serde_json::Value =
        serde_json::from_slice(&STANDARD.decode(raw.manifest).unwrap()).unwrap();
    assert_eq!(manifest["schemaVersion"], 1);
    assert!(manifest.get("agent").is_none());
}

#[test]
fn full_agent_invalid_inputs_preserve_the_previous_envelope() {
    for case in 0..15 {
        let f = AgentFixture::new();
        fs::write(f.envelope(), b"previous envelope").unwrap();
        match case {
            0 => fs::remove_file(f.path("newport-agent-aarch64")).unwrap(),
            1 => fs::remove_file(f.path("newport-agent-x86_64.product.json")).unwrap(),
            2 => {
                let path = f.path("newport-agent-x86_64");
                let mut bytes = fs::read(&path).unwrap();
                bytes[18..20].copy_from_slice(&183u16.to_le_bytes());
                fs::write(path, bytes).unwrap();
            }
            3 => {
                let path = f.path("newport-agent-aarch64");
                let mut bytes = fs::read(&path).unwrap();
                bytes[4] = 1;
                fs::write(path, bytes).unwrap();
            }
            4 => fs::write(f.path("newport-agent-x86_64"), b"\x7fELF").unwrap(),
            5 => {
                let path = f.path("newport-agent-aarch64");
                let mut bytes = fs::read(&path).unwrap();
                bytes.push(1);
                fs::write(path, bytes).unwrap();
            }
            6 => f.edit_identity("x86_64", |id| id["buildSha256"] = "0".repeat(64).into()),
            7 => f.edit_identity("aarch64", |id| {
                id["product"]["services"]["monitor"] = serde_json::json!([2])
            }),
            8 => f.edit_identity("x86_64", |id| {
                id["product"]["variant"] = "core".into();
                id["product"]["services"]
                    .as_object_mut()
                    .unwrap()
                    .remove("remote-apps");
            }),
            9 => f.edit_identity("aarch64", |id| id["extra"] = true.into()),
            10 => f.edit_identity("x86_64", |id| {
                id["product"]["controlVersions"] = serde_json::json!([true])
            }),
            11 => f.edit_identity("aarch64", |id| {
                id["product"]["services"]
                    .as_object_mut()
                    .unwrap()
                    .remove("git")
                    .map(drop)
                    .unwrap()
            }),
            12..=14 => {
                let path = f.path("newport-agent-x86_64");
                let mut bytes = fs::read(&path).unwrap();
                match case {
                    12 => bytes[16..18].copy_from_slice(&0u16.to_le_bytes()),
                    13 => bytes[20..24].copy_from_slice(&2u32.to_le_bytes()),
                    14 => bytes[52..54].copy_from_slice(&63u16.to_le_bytes()),
                    _ => unreachable!(),
                }
                fs::write(path, bytes).unwrap();
            }
            _ => unreachable!(),
        }
        let invalid_elf = match case {
            2 | 4 | 12..=14 => Some("x86_64"),
            3 => Some("aarch64"),
            _ => None,
        };
        if let Some(arch) = invalid_elf {
            // Isolate ELF validation: a matching digest must not accidentally
            // make a wrong architecture/class/header a signable payload.
            f.rehash_identity(arch);
        }
        let error = f.preserves_previous(&mut f.command("agent", "1.2.3"));
        if invalid_elf.is_some() {
            assert!(error.contains("ELF"), "case {case}: {error}");
            assert!(
                !error.contains("identity does not match"),
                "case {case}: {error}"
            );
        }
    }
}

#[test]
fn full_agent_invalid_input_does_not_create_an_envelope() {
    let f = AgentFixture::new();
    f.edit_identity("x86_64", |id| id["schemaVersion"] = 2.into());
    assert!(!f.run("agent").status.success());
    assert!(!f.envelope().exists());
    assert!(!f.run("verify-agent").status.success());
    assert!(!f.envelope().exists());
}

#[test]
fn full_agent_signing_rejects_wrong_keys_and_versions_without_replacing_the_envelope() {
    let f = AgentFixture::new();
    f.success(&f.run("agent"));
    for version in ["1.2", "v1.2.3", "1.2.3\tversion:9.0.0"] {
        f.preserves_previous(&mut f.command("agent", version));
    }
    let other = KeyPair::generate_unencrypted_keypair().unwrap();
    fs::write(
        &f.public,
        STANDARD.encode(other.pk.to_box().unwrap().to_string()),
    )
    .unwrap();
    f.preserves_previous(&mut f.command("agent", "1.2.3"));
}

#[test]
fn full_agent_verification_requires_signed_version_product_and_exact_payload_hashes() {
    use sha2::{Digest, Sha256};
    for case in 0..7 {
        let f = AgentFixture::new();
        f.success(&f.run("agent"));
        match case {
            0 => {
                let path = f.path("newport-agent-aarch64");
                let mut bytes = fs::read(&path).unwrap();
                bytes.push(1);
                fs::write(path, &bytes).unwrap();
                let hash: String = Sha256::digest(bytes)
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect();
                f.edit_identity("aarch64", |id| id["buildSha256"] = hash.into());
            }
            1 => {
                for arch in ["x86_64", "aarch64"] {
                    f.edit_identity(arch, |id| {
                        id["product"]["services"]["monitor"] = serde_json::json!([2])
                    });
                }
            }
            2 => {
                let other = KeyPair::generate_unencrypted_keypair().unwrap();
                fs::write(
                    &f.public,
                    STANDARD.encode(other.pk.to_box().unwrap().to_string()),
                )
                .unwrap();
            }
            3 => f.success(&f.run("needle")),
            4 => {
                let mut envelope: serde_json::Value =
                    serde_json::from_slice(&fs::read(f.envelope()).unwrap()).unwrap();
                envelope["extra"] = 1.into();
                fs::write(f.envelope(), serde_json::to_vec(&envelope).unwrap()).unwrap();
            }
            5 => {
                let mut envelope: serde_json::Value =
                    serde_json::from_slice(&fs::read(f.envelope()).unwrap()).unwrap();
                envelope["signature"] = "tampered".into();
                fs::write(f.envelope(), serde_json::to_vec(&envelope).unwrap()).unwrap();
            }
            6 => (),
            _ => unreachable!(),
        }
        let error = f.preserves_previous(
            &mut f.command("verify-agent", if case == 6 { "1.2.4" } else { "1.2.3" }),
        );
        let expected = match case {
            0 | 6 => "hash or release version",
            1 | 3 => "product does not match",
            _ => "release",
        };
        assert!(error.contains(expected), "case {case}: {error}");
    }
}

#[test]
fn full_agent_file_limits_reject_before_publication_and_accept_bounded_json_padding() {
    for (name, size, action) in [
        ("newport-agent-aarch64", 512u64 * 1024 * 1024 + 1, "agent"),
        ("newport-agent-x86_64.product.json", 16 * 1024 + 1, "agent"),
        (
            "needle-release.json",
            newport_release::needle::ENVELOPE_LIMIT as u64 + 1,
            "verify-agent",
        ),
        ("newport-agent-aarch64", 0, "agent"),
        ("newport-agent-x86_64.product.json", 0, "agent"),
        ("needle-release.json", 0, "verify-agent"),
    ] {
        let f = AgentFixture::new();
        fs::write(f.envelope(), b"previous envelope").unwrap();
        // Sparse oversized fixture: the checker must reject metadata without
        // reading or allocating the payload.
        fs::OpenOptions::new()
            .write(true)
            .open(f.path(name))
            .unwrap()
            .set_len(size)
            .unwrap();
        let error = f.preserves_previous(&mut f.command(action, "1.2.3"));
        assert!(error.contains("Invalid file size"), "{error}");
    }
    let f = AgentFixture::new();
    for arch in ["x86_64", "aarch64"] {
        let path = f.path(&format!("newport-agent-{arch}.product.json"));
        let mut bytes = fs::read(&path).unwrap();
        bytes.resize(16 * 1024, b' ');
        fs::write(path, bytes).unwrap();
    }
    f.success(&f.run("agent"));
    let mut envelope = fs::read(f.envelope()).unwrap();
    envelope.resize(newport_release::needle::ENVELOPE_LIMIT, b' ');
    fs::write(f.envelope(), envelope).unwrap();
    f.success(&f.run("verify-agent"));
}

#[test]
fn full_agent_public_key_size_is_bounded_before_signing_or_verification() {
    for size in [0, 16 * 1024 + 1] {
        let f = AgentFixture::new();
        f.success(&f.run("agent"));
        fs::OpenOptions::new()
            .write(true)
            .open(&f.public)
            .unwrap()
            .set_len(size)
            .unwrap();
        for action in ["agent", "verify-agent"] {
            let error = f.preserves_previous(&mut f.command(action, "1.2.3"));
            assert!(error.contains("public key size"), "{error}");
        }
    }
}

#[cfg(unix)]
#[test]
fn full_agent_rejects_symlink_and_non_regular_inputs_and_output() {
    use std::os::unix::{fs::symlink, net::UnixListener};
    for name in [
        "newport-agent-x86_64",
        "newport-agent-aarch64.product.json",
        "needle-release.json",
    ] {
        let f = AgentFixture::new();
        f.success(&f.run("agent"));
        let original = f.path(name);
        let retained = f.path(&format!("{name}.retained"));
        fs::rename(&original, &retained).unwrap();
        symlink(&retained, &original).unwrap();
        let before = fs::read(&retained).unwrap();
        for action in ["agent", "verify-agent"] {
            assert!(
                !f.run(action).status.success(),
                "{action} accepted {name} symlink"
            );
            assert_eq!(fs::read(&retained).unwrap(), before);
            assert_eq!(fs::read_link(&original).unwrap(), retained);
        }
    }
    for special in ["directory", "socket"] {
        let f = AgentFixture::new();
        fs::write(f.envelope(), b"previous envelope").unwrap();
        let path = f.path("newport-agent-x86_64");
        fs::remove_file(&path).unwrap();
        let _socket = if special == "socket" {
            Some(UnixListener::bind(&path).unwrap())
        } else {
            fs::create_dir(&path).unwrap();
            None
        };
        let error = f.preserves_previous(&mut f.command("agent", "1.2.3"));
        assert!(error.contains("regular file"), "{error}");
    }
    let f = AgentFixture::new();
    let alias = f._root.path().join("alias");
    symlink(&f.directory, &alias).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_newport-release"));
    let output = command
        .arg("agent")
        .arg(alias)
        .arg("1.2.3")
        .arg(&f.public)
        .env_remove("NEWPORT_SIGNING_PRIVATE_KEY")
        .env("NEWPORT_SIGNING_PRIVATE_KEY_PATH", &f.secret)
        .env_remove("NEWPORT_SIGNING_PRIVATE_KEY_PASSWORD")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!f.envelope().exists());
}

#[cfg(unix)]
#[test]
fn desktop_agent_comparison_reports_the_exact_signed_export_and_rejects_stale_bundles() {
    use sha2::{Digest, Sha256};
    use std::{io::Write, os::unix::fs::PermissionsExt};
    let f = AgentFixture::new();
    f.success(&f.run("agent"));
    let notices = f.path("licenses");
    fs::create_dir(&notices).unwrap();
    fs::write(notices.join("newport-full.txt"), b"fixture notice").unwrap();
    let envelope = fs::read(f.envelope()).unwrap();
    let hex = |bytes: &[u8]| -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    };
    let identity: newport_release::agent_product::Identity =
        serde_json::from_slice(&fs::read(f.path("newport-agent-x86_64.product.json")).unwrap())
            .unwrap();
    let payload = |arch: &str| {
        let bytes = fs::read(f.path(&format!("newport-agent-{arch}"))).unwrap();
        serde_json::json!({"sha256": hex(&bytes), "size": bytes.len()})
    };
    let report = serde_json::json!({
        "schema": 1, "variant": "full",
        "payloads": {"x86_64": payload("x86_64"), "aarch64": payload("aarch64")},
        "envelope": {"sha256": hex(&envelope), "size": envelope.len(), "version": "1.2.3",
                     "product": identity.product},
    });
    let app = f._root.path().join("Newport.app");
    let agents = app.join("Contents/Resources/agents");
    fs::create_dir_all(agents.join("licenses")).unwrap();
    fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
    fs::copy(
        notices.join("newport-full.txt"),
        agents.join("licenses/newport-full.txt"),
    )
    .unwrap();
    let bundle = |payload_suffix: &[u8]| {
        for arch in ["x86_64", "aarch64"] {
            let mut bytes = fs::read(f.path(&format!("newport-agent-{arch}"))).unwrap();
            if arch == "aarch64" {
                bytes.extend_from_slice(payload_suffix);
            }
            let mut encoder =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
            encoder.write_all(&bytes).unwrap();
            fs::write(
                agents.join(format!("newport-agent-{arch}.gz")),
                encoder.finish().unwrap(),
            )
            .unwrap();
        }
    };
    let executable = app.join("Contents/MacOS/newport");
    fs::write(
        &executable,
        format!("#!/bin/sh\n[ \"$1\" = --agent-identity ] || exit 97\nprintf '%s\\n' '{report}'\n"),
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    let compare = || {
        Command::new(env!("CARGO_BIN_EXE_newport-release"))
            .arg("verify-desktop-agent")
            .arg(&app)
            .arg(&f.directory)
            .arg("1.2.3")
            .arg(&f.public)
            .env("NEWPORT_SIGNING_PRIVATE_KEY", "never-loaded")
            .output()
            .unwrap()
    };
    bundle(b"");
    let output = compare();
    f.success(&output);
    let printed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(printed, report);
    bundle(b"stale");
    let output = compare();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Bundled aarch64 agent"));
}
