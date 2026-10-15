//! Standalone helper discovery and guarded recovery through the shipped CLI.
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};
const BIN: &str = env!("CARGO_BIN_EXE_newport-agent");

struct Fixture(tempfile::TempDir);
impl Fixture {
    fn new() -> Self {
        let fixture = Self(tempfile::tempdir().unwrap());
        fs::write(fixture.0.path().join(".bashrc"), "# personal settings\n").unwrap();
        let bin = fixture.0.path().join(".local/bin/newport-agent");
        fs::create_dir_all(bin.parent().unwrap()).unwrap();
        fs::copy(BIN, bin).unwrap();
        fixture
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(BIN)
            .args(args)
            .env("HOME", self.0.path())
            .env("SHELL", "/bin/bash")
            .env_remove("ZDOTDIR")
            .env_remove("XDG_CONFIG_HOME")
            .output()
            .unwrap()
    }
    fn success(&self, args: &[&str]) -> Output {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }
    fn history(&self) -> Value {
        serde_json::from_slice(&self.success(&["deployment", "helper-history"]).stdout).unwrap()
    }
    fn root(&self) -> PathBuf {
        self.0.path().join(".local/share/newport/needle")
    }
}

#[test]
fn standalone_history_requires_no_managed_installation_and_does_not_create_one() {
    let fixture = Fixture::new();
    let empty = fixture.history();
    assert_eq!(empty["ownershipRecorded"], false);
    assert!(!fixture.root().exists());
    fixture.success(&["install"]);
    let journal = fixture.root().join("helpers/journal.json");
    let original = fs::read(&journal).unwrap();
    let first = fixture.history();
    assert_eq!(first["ownershipRecorded"], true);
    assert_eq!(first["operations"].as_array().unwrap().len(), 1);
    assert_eq!(first["operations"][0]["origin"], "standalone");
    let id = first["operations"][0]["operationId"].as_str().unwrap();
    let fingerprint = first["operations"][0]["fingerprint"].as_str().unwrap();
    assert_eq!(fs::read(&journal).unwrap(), original);
    assert!(!fixture.root().join("activation.lock").exists());
    assert!(!fixture.root().join("operations.json").exists());
    assert_eq!(fixture.history(), first);
    assert!(!fixture
        .run(&["deployment", "restore-helper", id, &"0".repeat(64)])
        .status
        .success());
    assert_eq!(fs::read(&journal).unwrap(), original);
    let restored = fixture.success(&["deployment", "restore-helper", id, fingerprint]);
    let restored: Value = serde_json::from_slice(&restored.stdout).unwrap();
    assert_eq!(restored["state"], "rolledBack");
    assert_eq!(restored["fingerprint"], fingerprint);
    assert_eq!(
        fs::read_to_string(fixture.0.path().join(".bashrc")).unwrap(),
        "# personal settings\n"
    );
    assert!(!fixture.0.path().join(".local/bin/newport-browser").exists());
    fixture.success(&["deployment", "restore-helper", id, fingerprint]);
}

#[test]
fn managed_ids_are_excluded_and_personal_edits_require_file_review() {
    let fixture = Fixture::new();
    fixture.success(&["install"]);
    let first = fixture.history();
    let hash = first["buildSha256"].as_str().unwrap();
    let id = first["operations"][0]["operationId"].as_str().unwrap();
    let fingerprint = first["operations"][0]["fingerprint"].as_str().unwrap();
    let managed = uuid::Uuid::new_v4().to_string();
    fixture.success(&["deployment", "helpers", hash, "--operation", &managed]);
    assert_eq!(fixture.history()["operations"].as_array().unwrap().len(), 1);
    assert!(!fixture
        .run(&["deployment", "restore-helper", &managed, fingerprint])
        .status
        .success());
    let shell = fixture.0.path().join(".bashrc");
    let contents = fs::read_to_string(&shell).unwrap();
    let edited = contents.replace(" env ", " user-change ");
    assert_ne!(edited, contents);
    fs::write(&shell, &edited).unwrap();
    assert!(!fixture
        .run(&["deployment", "restore-helper", id, fingerprint])
        .status
        .success());
    assert_eq!(
        fixture.history()["operations"][0]["state"],
        "needsResolution"
    );
    let review: Value = serde_json::from_slice(
        &fixture
            .success(&["deployment", "review-helper", id, fingerprint])
            .stdout,
    )
    .unwrap();
    let file = review["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["path"] == shell.to_str().unwrap())
        .unwrap();
    let index = file["index"].as_u64().unwrap().to_string();
    fixture.success(&[
        "deployment",
        "keep-helper",
        id,
        fingerprint,
        &index,
        file["fingerprint"].as_str().unwrap(),
    ]);
    fixture.success(&["deployment", "restore-helper", id, fingerprint]);
    assert_eq!(fs::read_to_string(shell).unwrap(), edited);
}
