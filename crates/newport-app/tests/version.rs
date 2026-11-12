#[test]
fn the_version_command_reports_semver_without_opening_a_profile() {
    let directory = tempfile::tempdir().unwrap();
    let profile = directory.path().join("untouched-profile");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_newport"))
        .arg("--version")
        .env("NEWPORT_DATA_DIR", &profile)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("Newport {}\n", env!("CARGO_PKG_VERSION"))
    );
    assert!(!profile.exists());
}
