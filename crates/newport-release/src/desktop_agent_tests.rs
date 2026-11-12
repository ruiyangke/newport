use super::*;
use crate::needle;
use base64::{Engine, engine::general_purpose::STANDARD};
use minisign::KeyPair;
use newport_protocol::product::{CORE_SERVICES, Identity, Product};
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
    layout: PackageLayout,
}

impl World {
    fn new(layout: PackageLayout) -> Self {
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
                version: Some(VERSION.into()),
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
                "schema": 2, "variant": "full", "development": false,
                "newport": {"commit": CORE, "dirty": false},
                "remote": {"commit": REMOTE, "dirty": false},
            }))
            .unwrap(),
        );
        super::super::agent_export::sign_full_agent_export(&export, VERSION, &public, &pair.sk)
            .unwrap();
        let world = Self {
            app: root.path().join(match layout {
                PackageLayout::Macos => "bundle/Newport.app",
                PackageLayout::Windows => "bundle/Newport",
            }),
            _root: root,
            public,
            export,
            layout,
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

    fn verify(&self) -> Result<AgentReport> {
        verify_desktop_agent_for(
            self.layout,
            &self.app,
            &self.export,
            VERSION,
            self.public(),
            &|path| {
                assert_eq!(path, self.layout.executable(&self.app));
                assert!(path.is_file());
                Ok(serde_json::to_vec(&self.report())?)
            },
        )
    }

    /// Package data is portable; native execution is checked separately.
    fn build_app(&self) {
        let _ = fs::remove_dir_all(&self.app);
        write(
            &self.layout.executable(&self.app),
            b"identity probe fixture",
        );
        let agents = self.layout.agents(&self.app);
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
    }
}

fn error(result: Result<impl std::fmt::Debug>) -> String {
    format!("{:#}", result.unwrap_err())
}

#[test]
fn agent_export_versions_must_match_each_other_and_the_signed_release() {
    for (x86, arm) in [
        ("1.2.4", "1.2.3"),
        ("1.2.4", "1.2.4"),
        ("812297d", "812297d"),
    ] {
        let w = World::new(PackageLayout::Macos);
        for (arch, version) in [("x86_64", x86), ("aarch64", arm)] {
            let path = w.export.join(format!("newport-agent-{arch}.product.json"));
            let mut identity: Identity = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            identity.version = Some(version.into());
            write(&path, serde_json::to_vec(&identity).unwrap());
        }
        assert!(verified_export(&w.export, VERSION, w.public()).is_err());
        let pair = KeyPair::generate_unencrypted_keypair().unwrap();
        let public = STANDARD.encode(pair.pk.to_box().unwrap().to_string());
        let before = fs::read(w.export.join(needle::FILENAME)).unwrap();
        assert!(
            super::super::agent_export::sign_full_agent_export(
                &w.export, VERSION, &public, &pair.sk
            )
            .is_err()
        );
        assert_eq!(fs::read(w.export.join(needle::FILENAME)).unwrap(), before);
    }
}

#[test]
fn desktop_comparison_accepts_the_exact_export_and_rejects_stale_reports() {
    type Edit = fn(&mut serde_json::Value);
    let cases: [(&str, Edit); 10] = [
        ("schema", |r| r["schema"] = 2.into()),
        ("variant", |r| r["variant"] = "core".into()),
        ("x86 hash", |r| {
            r["payloads"]["x86_64"]["sha256"] = "0".repeat(64).into()
        }),
        ("arm size", |r| r["payloads"]["aarch64"]["size"] = 1.into()),
        ("envelope", |r| {
            r["envelope"]["sha256"] = "0".repeat(64).into()
        }),
        ("envelope size", |r| r["envelope"]["size"] = 1.into()),
        ("unsigned", |r| r["envelope"] = serde_json::Value::Null),
        ("version", |r| r["envelope"]["version"] = "1.2.4".into()),
        ("protocol", |r| {
            r["envelope"]["product"]["services"]["remote-apps"] = serde_json::json!([2])
        }),
        ("extra field", |r| r["extra"] = true.into()),
    ];
    for layout in [PackageLayout::Macos, PackageLayout::Windows] {
        let w = World::new(layout);
        assert_eq!(w.verify().unwrap(), w.report());
        let exact = serde_json::to_value(w.report()).unwrap();
        for (case, edit) in cases {
            let mut report = exact.clone();
            edit(&mut report);
            let printed = serde_json::to_vec(&report).unwrap();
            let result = verify_desktop_agent_for(
                layout,
                &w.app,
                &w.export,
                VERSION,
                w.public(),
                &|_: &Path| Ok(printed.clone()),
            );
            assert!(result.is_err(), "{layout:?}: {case}");
        }
        let failed = verify_desktop_agent_for(
            layout,
            &w.app,
            &w.export,
            VERSION,
            w.public(),
            &|_: &Path| bail!("probe failed"),
        );
        assert!(error(failed).contains("probe failed"));
    }
}

#[test]
fn desktop_comparison_rejects_changed_bundled_payloads_notices_and_resources() {
    type Edit = fn(&World, &Path);
    let cases: [(&str, Edit); 10] = [
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
        ("missing notice", |_, agents| {
            fs::remove_file(agents.join("licenses/newport-full.txt")).unwrap()
        }),
        ("notice directory", |_, agents| {
            fs::remove_file(agents.join("licenses/README.md")).unwrap();
            fs::create_dir(agents.join("licenses/README.md")).unwrap();
        }),
        ("extra notice", |_, agents| {
            fs::write(agents.join("licenses/extra.txt"), b"extra").unwrap()
        }),
        ("extra resource", |_, agents| {
            fs::write(agents.join("newport-agent-riscv64.gz"), b"extra").unwrap()
        }),
        ("missing payload", |_, agents| {
            fs::remove_file(agents.join("newport-agent-x86_64.gz")).unwrap()
        }),
        ("truncated payload", |_, agents| {
            fs::write(agents.join("newport-agent-x86_64.gz"), [0x1f, 0x8b]).unwrap()
        }),
        ("export product", |w, _| {
            let path = w.export.join("newport-agent-x86_64.product.json");
            let mut identity: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            identity["product"]["variant"] = "core".into();
            fs::write(path, serde_json::to_vec(&identity).unwrap()).unwrap();
        }),
        ("export envelope", |w, _| {
            let path = w.export.join(needle::FILENAME);
            let mut envelope: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            envelope["signature"] = "tampered".into();
            fs::write(path, serde_json::to_vec(&envelope).unwrap()).unwrap();
        }),
    ];
    for layout in [PackageLayout::Macos, PackageLayout::Windows] {
        for (case, edit) in cases {
            let w = World::new(layout);
            let report = serde_json::to_vec(&w.report()).unwrap();
            edit(&w, &layout.agents(&w.app));
            let result =
                verify_desktop_agent_for(layout, &w.app, &w.export, VERSION, w.public(), &|_| {
                    Ok(report.clone())
                });
            assert!(result.is_err(), "{layout:?}: {case}");
        }
    }
}

#[test]
fn package_layout_is_selected_explicitly() {
    for layout in [PackageLayout::Macos, PackageLayout::Windows] {
        let w = World::new(layout);
        let (executable, resources, other) = match layout {
            PackageLayout::Macos => (
                "Contents/MacOS/newport",
                "Contents/Resources/agents",
                PackageLayout::Windows,
            ),
            PackageLayout::Windows => ("newport.exe", "agents", PackageLayout::Macos),
        };
        assert_eq!(layout.executable(&w.app), w.app.join(executable));
        assert_eq!(layout.agents(&w.app), w.app.join(resources));
        let report = serde_json::to_vec(&w.report()).unwrap();
        assert!(
            verify_desktop_agent_for(other, &w.app, &w.export, VERSION, w.public(), &|_| {
                Ok(report.clone())
            })
            .is_err()
        );
        assert!(
            verify_desktop_agent_for(layout, &w.app, &w.export, "1.2.4", w.public(), &|_| {
                Ok(report.clone())
            })
            .is_err()
        );
    }
}

#[test]
fn windows_probe_uses_only_runtime_and_disposable_profile_environment() {
    let home = tempfile::tempdir().unwrap();
    let system_root = home.path().join("Windows");
    let executable = home.path().join("newport.exe");
    let command = windows_probe_command(&executable, home.path(), &system_root).unwrap();
    assert_eq!(command.get_program(), executable.as_os_str());
    assert_eq!(command.get_current_dir(), Some(home.path()));
    assert_eq!(
        command.get_args().collect::<Vec<_>>(),
        [std::ffi::OsStr::new(desktop::ARGUMENT)]
    );
    let environment: BTreeMap<_, _> = command
        .get_envs()
        .map(|(key, value)| {
            (
                key.to_str().unwrap().to_ascii_uppercase(),
                value.unwrap().to_owned(),
            )
        })
        .collect();
    let expected: BTreeSet<_> = [
        "SYSTEMROOT",
        "WINDIR",
        "PATH",
        "HOME",
        "USERPROFILE",
        "APPDATA",
        "LOCALAPPDATA",
        "TEMP",
        "TMP",
        "NEWPORT_DATA_DIR",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    assert_eq!(
        environment.keys().cloned().collect::<BTreeSet<_>>(),
        expected
    );
    for name in ["HOME", "USERPROFILE"] {
        assert_eq!(environment[name], home.path().as_os_str());
    }
    for name in ["APPDATA", "LOCALAPPDATA", "TEMP", "TMP", "NEWPORT_DATA_DIR"] {
        let path = Path::new(&environment[name]);
        assert!(path.starts_with(home.path()) && path != home.path());
    }
    assert_eq!(environment["SYSTEMROOT"], system_root.as_os_str());
    assert_eq!(environment["WINDIR"], system_root.as_os_str());
    assert_eq!(
        std::env::split_paths(&environment["PATH"]).collect::<Vec<_>>(),
        [system_root.join("System32"), system_root]
    );
    assert!(windows_probe_command(&executable, home.path(), Path::new("relative")).is_err());
}

#[cfg(unix)]
#[test]
fn macos_probe_and_default_layout_preserve_the_packaged_identity_contract() {
    use std::os::unix::fs::PermissionsExt;
    let w = World::new(PackageLayout::Macos);
    let executable = w.app.join("Contents/MacOS/newport");
    let report = serde_json::to_string(&w.report()).unwrap();
    write(
        &executable,
        format!("#!/bin/sh\n[ \"$1\" = --agent-identity ] || exit 97\nprintf '%s\\n' '{report}'\n"),
    );
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    std::os::unix::fs::symlink(
        "agents/licenses/README.md",
        w.app.join("Contents/Resources/notice"),
    )
    .unwrap();
    let actual = verify_desktop_agent(&w.app, &w.export, VERSION, w.public(), &run_probe).unwrap();
    assert_eq!(actual, w.report());
    assert!(run_probe(Path::new("/usr/bin/false")).is_err());
}

#[cfg(unix)]
#[test]
fn package_comparison_rejects_linked_agent_resources() {
    for layout in [PackageLayout::Macos, PackageLayout::Windows] {
        for relative in ["newport-agent-x86_64.gz", "licenses", "licenses/README.md"] {
            let w = World::new(layout);
            let path = layout.agents(&w.app).join(relative);
            let target = w.app.join("retained-original");
            fs::rename(&path, &target).unwrap();
            std::os::unix::fs::symlink(&target, &path).unwrap();
            assert!(w.verify().is_err(), "{layout:?}: {relative}");
        }
    }
}
