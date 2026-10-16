//! Removal and restoration through the shipped command surface.
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};
const BIN: &str = env!("CARGO_BIN_EXE_newport-agent");

struct Fixture {
    home: tempfile::TempDir,
    hash: String,
}
impl Fixture {
    fn new() -> Self {
        let value = Self::uninstalled();
        value.success(&[
            "deployment",
            "activate",
            &value.hash,
            "missing",
            "--operation",
            &uuid::Uuid::new_v4().to_string(),
            "--integrations",
        ]);
        value
    }
    fn uninstalled() -> Self {
        let value = Self {
            home: tempfile::tempdir().unwrap(),
            hash: Sha256::digest(fs::read(BIN).unwrap())
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect(),
        };
        fs::write(value.home.path().join(".bashrc"), "# personal settings\n").unwrap();
        value.success(&["deployment", "prepare", &value.hash]);
        value
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(BIN)
            .args(args)
            .env("HOME", self.home.path())
            .env("SHELL", "/bin/bash")
            .env_remove("ZDOTDIR")
            .env_remove("XDG_CONFIG_HOME")
            .output()
            .unwrap()
    }
    fn success(&self, args: &[&str]) -> Output {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }
    fn root(&self) -> PathBuf {
        self.home.path().join(".local/share/newport/needle")
    }
    fn remove(&self, id: &str) -> Output {
        self.run(&["deployment", "remove", &self.hash, "--operation", id])
    }
    fn state(output: &Output) -> String {
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()["state"]
            .as_str()
            .unwrap()
            .into()
    }
}

#[test]
fn binary_only_installation_removes_and_restores_without_touching_shell_files() {
    let fixture = Fixture::uninstalled();
    let shell = fixture.home.path().join(".bashrc");
    let alias = fixture.home.path().join(".local/bin/xclip");
    fs::write(&alias, "unrelated command").unwrap();
    let operation = uuid::Uuid::new_v4().to_string();
    let activate = [
        "deployment",
        "activate",
        &fixture.hash,
        "missing",
        "--operation",
        &operation,
    ];
    fixture.success(&activate);
    let journal_path = fixture.root().join("helpers/journal.json");
    let journal = fs::read(&journal_path).unwrap();
    let plan = fixture.success(&["deployment", "helper-removal-plan"]);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&plan.stdout).unwrap()["files"],
        serde_json::json!([])
    );
    fixture.success(&activate);
    assert_eq!(fs::read(&journal_path).unwrap(), journal);
    let id = uuid::Uuid::new_v4().to_string();
    assert_eq!(Fixture::state(&fixture.remove(&id)), "removed");
    assert!(!fixture
        .home
        .path()
        .join(".local/bin/newport-agent")
        .exists());
    assert!(!fixture.run(&["runtime", "--stdio"]).status.success());
    assert_eq!(fs::read_to_string(&shell).unwrap(), "# personal settings\n");
    assert_eq!(fs::read_to_string(&alias).unwrap(), "unrelated command");
    assert_eq!(
        Fixture::state(&fixture.success(&[
            "deployment",
            "restore-removal",
            &id,
            "--build",
            &fixture.hash
        ])),
        "restored"
    );
    assert!(fixture
        .home
        .path()
        .join(".local/bin/newport-agent")
        .exists());
    assert_eq!(fs::read_to_string(&shell).unwrap(), "# personal settings\n");
    assert_eq!(fs::read_to_string(&alias).unwrap(), "unrelated command");

    // Opting into helpers later establishes ownership above the empty baseline.
    fixture.success(&[
        "deployment",
        "helpers",
        &fixture.hash,
        "--operation",
        &uuid::Uuid::new_v4().to_string(),
    ]);
    assert!(fs::read_to_string(&shell)
        .unwrap()
        .contains("newport-agent"));
    assert_eq!(
        Fixture::state(&fixture.remove(&uuid::Uuid::new_v4().to_string())),
        "removed"
    );
    assert_eq!(fs::read_to_string(&shell).unwrap(), "# personal settings\n");
    assert_eq!(fs::read_to_string(&alias).unwrap(), "unrelated command");
}

#[test]
fn binary_only_activation_preserves_existing_helper_ownership() {
    let fixture = Fixture::uninstalled();
    fixture.success(&["install"]);
    let journal = fixture.root().join("helpers/journal.json");
    let before = fs::read(&journal).unwrap();
    fixture.success(&[
        "deployment",
        "activate",
        &fixture.hash,
        "missing",
        "--operation",
        &uuid::Uuid::new_v4().to_string(),
    ]);
    assert_eq!(fs::read(&journal).unwrap(), before);
    assert_eq!(
        Fixture::state(&fixture.remove(&uuid::Uuid::new_v4().to_string())),
        "removed"
    );
    assert_eq!(
        fs::read_to_string(fixture.home.path().join(".bashrc")).unwrap(),
        "# personal settings\n"
    );
}

#[test]
fn binary_only_activation_never_reconstructs_lost_or_corrupt_helper_history() {
    for corrupt in [false, true] {
        let fixture = Fixture::uninstalled();
        fixture.success(&["install"]);
        let journal = fixture.root().join("helpers/journal.json");
        if corrupt {
            fs::write(&journal, "unrecognized history").unwrap();
        } else {
            fs::remove_file(&journal).unwrap();
        }
        let shell = fixture.home.path().join(".bashrc");
        let before = fs::read(&shell).unwrap();
        assert!(!fixture
            .run(&[
                "deployment",
                "activate",
                &fixture.hash,
                "missing",
                "--operation",
                &uuid::Uuid::new_v4().to_string()
            ])
            .status
            .success());
        assert!(!fixture
            .home
            .path()
            .join(".local/bin/newport-agent")
            .exists());
        assert!(!fixture.root().join("operations.json").exists());
        assert_eq!(fs::read(&shell).unwrap(), before);
        if corrupt {
            assert_eq!(
                fs::read_to_string(&journal).unwrap(),
                "unrecognized history"
            );
        } else {
            assert!(!journal.exists());
        }
    }
    let fixture = Fixture::uninstalled();
    fixture.success(&[
        "deployment",
        "activate",
        &fixture.hash,
        "missing",
        "--operation",
        &uuid::Uuid::new_v4().to_string(),
    ]);
    let journal = fixture.root().join("helpers/journal.json");
    fs::remove_file(&journal).unwrap();
    fixture.success(&[
        "deployment",
        "activate",
        &fixture.hash,
        &fixture.hash,
        "--operation",
        &uuid::Uuid::new_v4().to_string(),
    ]);
    assert!(
        !journal.exists(),
        "a reinstall must not manufacture empty history"
    );
    assert!(!fixture
        .remove(&uuid::Uuid::new_v4().to_string())
        .status
        .success());
    assert!(fixture
        .home
        .path()
        .join(".local/bin/newport-agent")
        .exists());
}

#[test]
fn cancelled_removal_is_durable_and_cannot_remove_files_later() {
    let fixture = Fixture::new();
    let shell = fixture.home.path().join(".bashrc");
    let original = fs::read(&shell).unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    // Cancellation of an undispatched intent is safe while a client is active.
    let mut runtime = Command::new(BIN)
        .args(["runtime", "--stdio"])
        .env("HOME", fixture.home.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    // A real client initializes and sends heartbeats. Leaving the runtime at
    // Hello only tests its five-second abandoned-handshake timeout on slow CI.
    use newport_agent::runtime_protocol as protocol;
    let mut input = runtime.stdin.take().unwrap();
    let mut output = runtime.stdout.take().unwrap();
    let hello: protocol::Hello = protocol::read(&mut output).unwrap();
    let request = |operation| protocol::Request {
        id: uuid::Uuid::new_v4().to_string(),
        runtime_id: hello.instance_id.clone(),
        operation,
    };
    protocol::write(
        &mut input,
        &request(protocol::Operation::Initialize {
            version: protocol::VERSION,
            session_id: uuid::Uuid::new_v4().to_string(),
            transport_generation: 1,
        }),
    )
    .unwrap();
    let reply: protocol::Reply = protocol::read(&mut output).unwrap();
    assert!(reply.error.is_none());
    let (stop, stopping) = std::sync::mpsc::channel();
    let heartbeat = std::thread::spawn(move || {
        while stopping.recv_timeout(std::time::Duration::from_secs(1))
            == Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        {
            protocol::write(
                &mut input,
                &protocol::Request {
                    id: uuid::Uuid::new_v4().to_string(),
                    runtime_id: hello.instance_id.clone(),
                    operation: protocol::Operation::Ping,
                },
            )
            .unwrap();
            let reply: protocol::Reply = protocol::read(&mut output).unwrap();
            assert!(reply.error.is_none());
        }
    });
    assert_eq!(
        Fixture::state(&fixture.success(&[
            "deployment",
            "restore-removal",
            &id,
            "--build",
            &fixture.hash
        ])),
        "restored"
    );
    assert!(runtime.try_wait().unwrap().is_none());
    assert_eq!(Fixture::state(&fixture.remove(&id)), "restored");
    assert_eq!(fs::read(&shell).unwrap(), original);
    assert!(fixture
        .home
        .path()
        .join(".local/bin/newport-agent")
        .exists());
    // End the client normally; dropping its input releases the runtime lease.
    stop.send(()).unwrap();
    heartbeat.join().unwrap();
    assert!(runtime.wait().unwrap().success());
    // Later operations remain independent; replay of either old action cannot
    // resurrect the entry point removed by a newer operation.
    let next = uuid::Uuid::new_v4().to_string();
    assert_eq!(Fixture::state(&fixture.remove(&next)), "removed");
    assert_eq!(
        Fixture::state(&fixture.success(&[
            "deployment",
            "restore-removal",
            &id,
            "--build",
            &fixture.hash
        ])),
        "restored"
    );
    assert_eq!(Fixture::state(&fixture.remove(&id)), "restored");
    assert!(!fixture
        .home
        .path()
        .join(".local/bin/newport-agent")
        .exists());
    assert_eq!(
        Fixture::state(&fixture.success(&[
            "deployment",
            "restore-removal",
            &next,
            "--build",
            &fixture.hash
        ])),
        "restored"
    );
    assert_eq!(fs::read(&shell).unwrap(), original);
    let wrong = "a".repeat(64);
    let bytes = fs::read(fixture.root().join("removals.json")).unwrap();
    assert!(!fixture
        .run(&["deployment", "restore-removal", &id, "--build", &wrong])
        .status
        .success());
    assert!(!fixture
        .run(&[
            "deployment",
            "restore-removal",
            &uuid::Uuid::nil().to_string(),
            "--build",
            &fixture.hash
        ])
        .status
        .success());
    assert_eq!(
        fs::read(fixture.root().join("removals.json")).unwrap(),
        bytes
    );
}

#[test]
fn removal_preserves_user_edits_blocks_readmission_and_restores_by_id() {
    let fixture = Fixture::new();
    let shell = fixture.home.path().join(".bashrc");
    let installed = fs::read_to_string(&shell).unwrap() + "# later settings\n";
    fs::write(&shell, &installed).unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let mut runtime = Command::new(BIN)
        .args(["runtime", "--stdio"])
        .env("HOME", fixture.home.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let hello: newport_agent::runtime_protocol::Hello =
        newport_agent::runtime_protocol::read(runtime.stdout.as_mut().unwrap()).unwrap();
    assert_eq!(hello.build_sha256, fixture.hash);
    let busy = fixture.remove(&id);
    assert!(!busy.status.success());
    assert!(String::from_utf8_lossy(&busy.stderr).contains("in use"));
    assert!(!fixture.root().join("removals.json").exists());
    runtime.kill().unwrap();
    runtime.wait().unwrap();
    let edited = installed.replace(" env ", " user-edited ");
    fs::write(&shell, &edited).unwrap();
    assert!(!fixture.remove(&id).status.success());
    assert!(!fixture.root().join("removals.json").exists());
    assert_eq!(fs::read_to_string(&shell).unwrap(), edited);
    fs::write(&shell, &installed).unwrap();

    assert_eq!(Fixture::state(&fixture.remove(&id)), "removed");
    assert_eq!(
        fs::read_to_string(&shell).unwrap(),
        "# personal settings\n# later settings\n"
    );
    assert!(!fixture
        .home
        .path()
        .join(".local/bin/newport-agent")
        .exists());
    assert!(fs::symlink_metadata(fixture.home.path().join(".local/bin/newport-browser")).is_err());
    assert!(fixture
        .root()
        .join("builds")
        .join(&fixture.hash)
        .join("newport-agent")
        .is_file());
    assert_eq!(Fixture::state(&fixture.remove(&id)), "removed");
    for args in [
        vec!["runtime", "--stdio"],
        vec!["install"],
        vec!["deployment", "prepare", &fixture.hash],
    ] {
        let result = fixture.run(&args);
        assert!(!result.status.success(), "{args:?}");
        assert!(String::from_utf8_lossy(&result.stderr).contains("removal is pending or completed"));
    }
    assert_eq!(
        Fixture::state(&fixture.success(&["deployment", "restore-removal", &id])),
        "restored"
    );
    assert_eq!(fs::read_to_string(&shell).unwrap(), installed);
    assert_eq!(
        fs::read_link(fixture.home.path().join(".local/bin/newport-browser")).unwrap(),
        Path::new("newport-agent")
    );
    assert_eq!(Fixture::state(&fixture.remove(&id)), "restored");
    let next = uuid::Uuid::new_v4().to_string();
    assert_eq!(Fixture::state(&fixture.remove(&next)), "removed");
    // An old completed restore cannot reactivate a subsequently removed build.
    assert_eq!(
        Fixture::state(&fixture.success(&["deployment", "restore-removal", &id])),
        "restored"
    );
    assert!(!fixture
        .home
        .path()
        .join(".local/bin/newport-agent")
        .exists());
}

#[test]
fn older_build_evidence_and_corrupt_admission_fail_without_target_changes() {
    let fixture = Fixture::new();
    let compatibility = fixture
        .root()
        .join("builds")
        .join(&fixture.hash)
        .join("lifecycle.json");
    fs::remove_file(&compatibility).unwrap();
    let before = fs::read(fixture.home.path().join(".bashrc")).unwrap();
    let result = fixture.remove(&uuid::Uuid::new_v4().to_string());
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("older-client compatibility"));
    assert_eq!(
        fs::read(fixture.home.path().join(".bashrc")).unwrap(),
        before
    );
    assert!(!fixture.root().join("removals.json").exists());
    // Explicit staging by this executable can establish only its own support.
    fixture.success(&["deployment", "prepare", &fixture.hash]);
    fs::write(&compatibility, b"unknown future format").unwrap();
    assert!(!fixture
        .run(&["deployment", "prepare", &fixture.hash])
        .status
        .success());
    assert_eq!(fs::read(&compatibility).unwrap(), b"unknown future format");
    fs::write(fixture.root().join("removals.json"), b"not a receipt").unwrap();
    for args in [vec!["runtime", "--stdio"], vec!["install"]] {
        assert!(!fixture.run(&args).status.success());
    }
    assert_eq!(
        fs::read(fixture.root().join("removals.json")).unwrap(),
        b"not a receipt"
    );
    assert_eq!(
        fs::read(fixture.home.path().join(".bashrc")).unwrap(),
        before
    );
}

#[test]
fn missing_evidence_or_later_edits_keep_removal_recovery_pending() {
    let fixture = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    assert_eq!(Fixture::state(&fixture.remove(&id)), "removed");
    let journal = fixture.root().join("helpers/journal.json");
    let evidence = fs::read(&journal).unwrap();
    fs::remove_file(&journal).unwrap();
    assert!(!fixture
        .run(&["deployment", "restore-removal", &id])
        .status
        .success());
    assert!(!journal.exists());
    assert_eq!(
        Fixture::state(&fixture.success(&["deployment", "removal", &id])),
        "restoring"
    );
    assert!(!fixture.run(&["runtime", "--stdio"]).status.success());
    fs::write(&journal, evidence).unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&journal, fs::Permissions::from_mode(0o600)).unwrap();
    let shell = fixture.home.path().join(".bashrc");
    let removed = fs::read(&shell).unwrap();
    fs::write(&shell, "# changed after removal\n").unwrap();
    assert!(!fixture
        .run(&["deployment", "restore-removal", &id])
        .status
        .success());
    assert_eq!(
        fs::read_to_string(&shell).unwrap(),
        "# changed after removal\n"
    );
    assert!(!fixture.run(&["runtime", "--stdio"]).status.success());
    fs::write(&shell, removed).unwrap();
    assert_eq!(
        Fixture::state(&fixture.success(&["deployment", "restore-removal", &id])),
        "restored"
    );
}

#[test]
fn restoration_review_preserves_kept_files_and_releases_prior_ownership() {
    let fixture = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    assert_eq!(Fixture::state(&fixture.remove(&id)), "removed");
    let review_args = [
        "deployment",
        "review-removal",
        &id,
        "--build",
        &fixture.hash,
    ];
    assert!(
        !fixture.run(&review_args).status.success(),
        "review requires restoration intent"
    );
    let path = fixture.home.path().join(".bashrc");
    fs::write(&path, "# user replaced this file after removal\n").unwrap();
    assert!(!fixture
        .run(&[
            "deployment",
            "restore-removal",
            &id,
            "--build",
            &fixture.hash
        ])
        .status
        .success());
    let evidence = fixture.root().join("helpers/journal.json");
    let before = fs::read(&evidence).unwrap();
    let report: serde_json::Value =
        serde_json::from_slice(&fixture.success(&review_args).stdout).unwrap();
    assert_eq!(fs::read(&evidence).unwrap(), before, "review is read-only");
    let file = report["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["path"] == path.to_str().unwrap())
        .unwrap();
    assert_eq!(file["status"], "conflict");
    let index = file["index"].as_u64().unwrap().to_string();
    let stale = file["fingerprint"].as_str().unwrap();
    fs::write(&path, "# another edit since review\n").unwrap();
    assert!(!fixture
        .run(&[
            "deployment",
            "keep-removal",
            &id,
            "--build",
            &fixture.hash,
            &index,
            stale
        ])
        .status
        .success());
    let report: serde_json::Value =
        serde_json::from_slice(&fixture.success(&review_args).stdout).unwrap();
    let file = report["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["path"] == path.to_str().unwrap())
        .unwrap();
    let fingerprint = file["fingerprint"].as_str().unwrap();
    let wrong = "f".repeat(64);
    assert!(!fixture
        .run(&[
            "deployment",
            "keep-removal",
            &id,
            "--build",
            &wrong,
            &index,
            fingerprint
        ])
        .status
        .success());
    let other = uuid::Uuid::new_v4().to_string();
    assert!(!fixture
        .run(&[
            "deployment",
            "keep-removal",
            &other,
            "--build",
            &fixture.hash,
            &index,
            fingerprint
        ])
        .status
        .success());
    let keep_args = [
        "deployment",
        "keep-removal",
        &id,
        "--build",
        &fixture.hash,
        &index,
        fingerprint,
    ];
    fixture.success(&keep_args);
    fs::write(&path, "# final user content\n").unwrap();
    fixture.success(&keep_args); // Lost acknowledgements do not broaden or repeat a decision.
    assert_eq!(
        Fixture::state(&fixture.success(&[
            "deployment",
            "restore-removal",
            &id,
            "--build",
            &fixture.hash
        ])),
        "restored"
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), "# final user content\n");
    let plan: serde_json::Value = serde_json::from_slice(
        &fixture
            .success(&["deployment", "helper-removal-plan"])
            .stdout,
    )
    .unwrap();
    assert!(
        !plan["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["path"] == path.to_str().unwrap()),
        "kept restoration path must no longer belong to the earlier installation"
    );
    assert!(
        !fixture.run(&keep_args).status.success(),
        "completed restoration cannot accept a stale keep action"
    );
}
