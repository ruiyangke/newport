use std::process::Command;

#[test]
fn the_cli_and_product_report_use_the_product_release_version() {
    let metadata: serde_json::Value =
        serde_json::from_str(include_str!("../../../resources/app.json")).unwrap();
    let version = metadata["version"].as_str().unwrap();
    assert!(newport_protocol::product::valid_version(version));
    assert_eq!(newport_agent::VERSION, version);
    let binary = env!("CARGO_BIN_EXE_newport-agent");
    let output = Command::new(binary).arg("--version").output().unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("newport-agent {version}\n")
    );
    let output = Command::new(binary)
        .args(["product", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let identity: newport_protocol::product::Identity =
        serde_json::from_slice(&output.stdout).unwrap();
    identity.validate().unwrap();
    assert_eq!(identity.version.as_deref(), Some(version));
}
