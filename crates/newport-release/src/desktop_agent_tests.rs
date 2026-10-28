use super::*;
use crate::{
    agent_product::{CORE_SERVICES, Identity, Product},
    needle,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use minisign::KeyPair;
use std::{io::Write, path::PathBuf};

const VERSION: &str = "1.2.3";
const CORE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const REMOTE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn write(path: &Path, bytes: impl AsRef<[u8]>) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

/// A signed full export and the desktop packaged from it, with a disposable key pair.
struct World {
    _root: tempfile::TempDir,
    public: String,
    export: PathBuf,
    app: PathBuf,
}

impl World {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let pair = KeyPair::generate_unencrypted_keypair().unwrap();
        let public = STANDARD.encode(pair.pk.to_box().unwrap().to_string());
        let export = root.path().join("export");
        let mut product = Product {
            variant: Variant::Full,
            control_versions: vec![1],
            services: CORE_SERVICES
                .iter()
                .map(|name| (name.to_string(), vec![1]))
                .collect(),
        };
        product.services.insert("remote-apps".into(), vec![1]);
        for (arch, machine) in [("x86_64", 62u16), ("aarch64", 183)] {
            let mut bytes = vec![0u8; 64];
            bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
            bytes[16..18].copy_from_slice(&2u16.to_le_bytes());
            bytes[18..20].copy_from_slice(&machine.to_le_bytes());
            bytes[20..24].copy_from_slice(&1u32.to_le_bytes());
            bytes[52..54].copy_from_slice(&64u16.to_le_bytes());
            bytes.extend(arch.bytes().cycle().take(100_000));
            let identity = Identity {
                schema_version: 1,
                build_sha256: sha256(&bytes),
                product: product.clone(),
            };
            write(&export.join(format!("newport-agent-{arch}")), &bytes);
            write(
                &export.join(format!("newport-agent-{arch}.product.json")),
                serde_json::to_vec(&identity).unwrap(),
            );
        }
        for name in ["README.md", "newport-core.txt", "newport-full.txt"] {
            write(
                &export.join("licenses").join(name),
                format!("fixture {name}"),
            );
        }
        write(
            &export.join("full-agent-provenance.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema": 1, "variant": "full", "development": false,
                "sources": {"core": {"commit": CORE, "dirty": false},
                            "remote": {"commit": REMOTE, "dirty": false}},
                "sourceSha256": "c".repeat(64),
            }))
            .unwrap(),
        );
        super::super::agent_export::sign_full_agent_export(&export, VERSION, &public, &pair.sk)
            .unwrap();
        let world = Self {
            app: root.path().join("bundle/Newport.app"),
            _root: root,
            public,
            export,
        };
        world.build_app();
        world
    }

    fn public(&self) -> &str {
        &self.public
    }

    fn report(&self) -> AgentReport {
        let verified = verified_export(&self.export, VERSION, self.public()).unwrap();
        let payload = |arch: &str| {
            let bytes = fs::read(self.export.join(format!("newport-agent-{arch}"))).unwrap();
            Payload {
                sha256: sha256(&bytes),
                size: bytes.len() as u64,
            }
        };
        AgentReport {
            schema: REPORT_SCHEMA,
            variant: Variant::Full,
            payloads: Payloads {
                x86_64: payload("x86_64"),
                aarch64: payload("aarch64"),
            },
            envelope: Some(desktop::Envelope {
                sha256: sha256(&verified.envelope),
                size: verified.envelope.len() as u64,
                version: VERSION.into(),
                product: Some(verified.product),
            }),
        }
    }

    /// The packaged bundle: a probe script printing the exact report, compressed
    /// payloads and notices from the export, and one internal link.
    fn build_app(&self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::remove_dir_all(&self.app);
        let contents = self.app.join("Contents");
        write(
            &contents.join("Info.plist"),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>app.newport</string>
<key>CFBundleShortVersionString</key><string>{VERSION}</string>
<key>CFBundleExecutable</key><string>newport</string>
</dict></plist>"#
            ),
        );
        let report = serde_json::to_string(&self.report()).unwrap();
        let executable = contents.join("MacOS/newport");
        write(
            &executable,
            format!(
                "#!/bin/sh\n[ \"$1\" = --agent-identity ] || exit 97\nprintf '%s\\n' '{report}'\n"
            ),
        );
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        let agents = contents.join("Resources/agents");
        for arch in ARCHITECTURES {
            let bytes = fs::read(self.export.join(format!("newport-agent-{arch}"))).unwrap();
            let mut encoder =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
            encoder.write_all(&bytes).unwrap();
            write(
                &agents.join(format!("newport-agent-{arch}.gz")),
                encoder.finish().unwrap(),
            );
        }
        for entry in fs::read_dir(self.export.join("licenses")).unwrap() {
            let entry = entry.unwrap();
            write(
                &agents.join("licenses").join(entry.file_name()),
                fs::read(entry.path()).unwrap(),
            );
        }
        std::os::unix::fs::symlink(
            "agents/licenses/README.md",
            contents.join("Resources/notice"),
        )
        .unwrap();
    }
}

fn error(result: Result<impl std::fmt::Debug>) -> String {
    format!("{:#}", result.unwrap_err())
}

#[test]
fn desktop_comparison_accepts_the_exact_export_and_rejects_stale_reports() {
    let w = World::new();
    let report = verify_desktop_agent(&w.app, &w.export, VERSION, w.public(), &run_probe).unwrap();
    assert_eq!(report, w.report());
    let exact = serde_json::to_value(w.report()).unwrap();
    type Edit = fn(&mut serde_json::Value);
    let cases: [(&str, Edit); 6] = [
        ("x86 hash", |r| {
            r["payloads"]["x86_64"]["sha256"] = "0".repeat(64).into()
        }),
        ("arm size", |r| r["payloads"]["aarch64"]["size"] = 1.into()),
        ("envelope", |r| {
            r["envelope"]["sha256"] = "0".repeat(64).into()
        }),
        ("unsigned", |r| r["envelope"] = serde_json::Value::Null),
        ("version", |r| r["envelope"]["version"] = "1.2.4".into()),
        ("extra field", |r| r["extra"] = true.into()),
    ];
    for (case, edit) in cases {
        let mut report = exact.clone();
        edit(&mut report);
        let printed = serde_json::to_vec(&report).unwrap();
        let result = verify_desktop_agent(&w.app, &w.export, VERSION, w.public(), &|_: &Path| {
            Ok(printed.clone())
        });
        assert!(result.is_err(), "{case}");
    }
    let failed = verify_desktop_agent(&w.app, &w.export, VERSION, w.public(), &|_: &Path| {
        bail!("probe failed")
    });
    assert!(error(failed).contains("probe failed"));
    // The installed probe must exit successfully with the argument it expects.
    assert!(run_probe(Path::new("/usr/bin/false")).is_err());
}

#[test]
fn desktop_comparison_rejects_changed_bundled_payloads_notices_and_resources() {
    let agents = |w: &World| w.app.join("Contents/Resources/agents");
    type Edit = fn(&World, &Path);
    let cases: [(&str, Edit); 5] = [
        ("payload", |_, agents| {
            let mut encoder =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
            encoder.write_all(b"other payload").unwrap();
            fs::write(
                agents.join("newport-agent-aarch64.gz"),
                encoder.finish().unwrap(),
            )
            .unwrap();
        }),
        ("notice", |_, agents| {
            fs::write(agents.join("licenses/README.md"), b"changed").unwrap()
        }),
        ("extra notice", |_, agents| {
            fs::write(agents.join("licenses/extra.txt"), b"extra").unwrap()
        }),
        ("extra resource", |_, agents| {
            fs::write(agents.join("newport-agent-riscv64.gz"), b"extra").unwrap()
        }),
        ("export envelope", |w, _| {
            let path = w.export.join(needle::FILENAME);
            let mut envelope: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            envelope["signature"] = "tampered".into();
            fs::write(path, serde_json::to_vec(&envelope).unwrap()).unwrap();
        }),
    ];
    for (case, edit) in cases {
        let w = World::new();
        edit(&w, &agents(&w));
        let result = verify_desktop_agent(&w.app, &w.export, VERSION, w.public(), &run_probe);
        assert!(result.is_err(), "{case}");
    }
}
