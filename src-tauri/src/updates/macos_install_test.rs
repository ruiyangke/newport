//! Exercise the production updater against a disposable copy of an installed app.
use std::{
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};
use tauri::test::{mock_builder, mock_context, noop_assets};
use tauri_plugin_updater::UpdaterExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
#[ignore = "Requires NEWPORT_UPDATE_TEST_APP and NEWPORT_UPDATE_TEST_PACKAGE: a signed old app and signed, notarized update archive"]
async fn signed_macos_rebrand_installs_over_existing_bundle() {
    let source = PathBuf::from(std::env::var_os("NEWPORT_UPDATE_TEST_APP").unwrap());
    let package = PathBuf::from(std::env::var_os("NEWPORT_UPDATE_TEST_PACKAGE").unwrap());
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("Porthop.app");
    assert!(Command::new("ditto")
        .arg(&source)
        .arg(&destination)
        .status()
        .unwrap()
        .success());
    let old = bundle_info(&destination);
    let old_binary = old["CFBundleExecutable"].as_str().unwrap();
    let old_version = old["CFBundleShortVersionString"].as_str().unwrap();
    assert_eq!(old["CFBundleIdentifier"].as_str(), Some("ke.ry.porthop"));
    let config: serde_json::Value =
        serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let manifest = serde_json::json!({
        "version": config["version"],
        "platforms": {"darwin-aarch64": {
            "url": format!("http://{address}/package"),
            "signature": std::fs::read_to_string(format!("{}.sig", package.display())).unwrap().trim(),
        }},
    });
    let bytes = std::fs::read(package).unwrap();
    let server = tokio::spawn(async move {
        for body in [serde_json::to_vec(&manifest).unwrap(), bytes] {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                assert!(request.len() < 65536);
                request.push(socket.read_u8().await.unwrap());
            }
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            socket.write_all(&body).await.unwrap();
        }
    });
    let mut context = mock_context(noop_assets());
    context.package_info_mut().version = old_version.parse().unwrap();
    context.config_mut().plugins.0.insert(
        "updater".into(),
        serde_json::json!({
            "pubkey": config["plugins"]["updater"]["pubkey"],
            "requireSignedVersion": true,
            "dangerousInsecureTransportProtocol": true,
            "endpoints": [format!("http://{address}/latest.json")],
        }),
    );
    let app = mock_builder()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .build(context)
        .unwrap();
    let updater = app
        .updater_builder()
        .executable_path(destination.join("Contents/MacOS").join(old_binary))
        .target("darwin-aarch64")
        .timeout(Duration::from_secs(60))
        .build()
        .unwrap();
    let update = updater
        .check()
        .await
        .unwrap()
        .expect("Newport must be newer than the installed app");
    let bytes = update.download(|_, _| {}, || {}).await.unwrap();
    server.await.unwrap();
    tokio::task::spawn_blocking(move || update.install(bytes))
        .await
        .unwrap()
        .unwrap();
    let new = bundle_info(&destination);
    assert_eq!(new["CFBundleIdentifier"].as_str(), Some("app.newport"));
    assert_eq!(
        new["CFBundleShortVersionString"].as_str(),
        config["version"].as_str()
    );
    assert_eq!(new["CFBundleExecutable"].as_str(), Some("newport"));
    assert!(destination.join("Contents/MacOS/newport").is_file());
    assert!(!destination.join("Contents/MacOS").join(old_binary).exists());
    for (tool, args) in [
        ("codesign", vec!["--verify", "--deep", "--strict"]),
        ("xcrun", vec!["stapler", "validate"]),
        ("spctl", vec!["--assess", "--type", "execute"]),
    ] {
        assert!(
            Command::new(tool)
                .args(args)
                .arg(&destination)
                .status()
                .unwrap()
                .success(),
            "{tool} rejected the installed update"
        );
    }
}

fn bundle_info(app: &Path) -> serde_json::Value {
    let output = Command::new("plutil")
        .args(["-convert", "json", "-o", "-"])
        .arg(app.join("Contents/Info.plist"))
        .output()
        .unwrap();
    assert!(output.status.success());
    serde_json::from_slice(&output.stdout).unwrap()
}
