//! macOS protected Keychain test inside a signed, provisioned test bundle.
//!
//! Only uniquely named fixture entries are touched; this neither launches
//! Newport nor reads an installed user profile.
use crate::{
    process::Process,
    signing::{self, DevSigning},
    util,
};
use anyhow::{Context, Result, bail, ensure};
use plist::{Dictionary, Value};
use serde::Deserialize;
use std::{fs, path::PathBuf, time::Duration};

const TEST: &str = "secrets::macos::tests::protected_keychain_stores_and_isolates_profile_keys";

/// One `cargo --message-format=json` line, as far as test executables need.
#[derive(Deserialize)]
struct Message {
    reason: String,
    target: Option<Target>,
    profile: Option<Profile>,
    executable: Option<PathBuf>,
}

#[derive(Deserialize)]
struct Target {
    name: String,
}

#[derive(Deserialize)]
struct Profile {
    test: bool,
}

/// The single `newport_platform` library test executable in Cargo's JSON messages.
pub fn test_executable(messages: &str) -> Result<PathBuf> {
    let mut executables = Vec::new();
    for line in messages.lines().filter(|l| !l.trim().is_empty()) {
        let message: Message = serde_json::from_str(line).context("Invalid Cargo message")?;
        if message.reason == "compiler-artifact"
            && message.target.is_some_and(|t| t.name == "newport_platform")
            && message.profile.is_some_and(|p| p.test)
        {
            executables.extend(message.executable);
        }
    }
    ensure!(
        executables.len() == 1,
        "Cargo must return exactly one platform library test executable"
    );
    Ok(executables.remove(0))
}

/// The test bundle's Info.plist.
fn info_plist(identifier: &str) -> Value {
    let mut d = Dictionary::new();
    for (key, value) in [
        ("CFBundleIdentifier", identifier),
        ("CFBundleExecutable", "keychain-test"),
        ("CFBundleName", "NewportKeychainTest"),
        ("CFBundlePackageType", "APPL"),
        ("CFBundleVersion", "1"),
    ] {
        d.insert(key.into(), value.into());
    }
    Value::Dictionary(d)
}

pub fn run() -> Result<()> {
    if !cfg!(target_os = "macos") {
        bail!("Protected-Keychain acceptance requires macOS");
    }
    let signing = DevSigning::load()?.context(
        "Set NEWPORT_SIGNING_IDENTITY, NEWPORT_PROVISIONING_PROFILE and \
         NEWPORT_SIGNING_ENTITLEMENTS, or .cargo/signing.toml",
    )?;
    ensure!(
        signing.identity != "-",
        "Provide an explicit signing identity, not ad hoc signing"
    );
    let identifier = util::app()?.identifier;
    signing::check(
        &signing.entitlements,
        &signing.provisioning_profile,
        &identifier,
    )?;
    let messages = util::capture(util::without_release_secrets(util::command("cargo").args(
        [
            "test",
            "--locked",
            "-p",
            "newport-platform",
            "--lib",
            "--no-run",
            "--message-format=json",
        ],
    )))?;
    let executable = test_executable(&messages)?;

    let directory = tempfile::Builder::new()
        .prefix("newport-keychain-acceptance-")
        .tempdir()?
        .keep();
    let app = directory.join("NewportKeychainTest.app");
    let contents = app.join("Contents");
    let binary = contents.join("MacOS/keychain-test");
    fs::create_dir_all(contents.join("MacOS"))?;
    fs::copy(&executable, &binary)
        .with_context(|| format!("Cannot copy {}", executable.display()))?;
    fs::copy(
        &signing.provisioning_profile,
        contents.join("embedded.provisionprofile"),
    )?;
    info_plist(&identifier).to_file_xml(contents.join("Info.plist"))?;
    // Local test only: no network timestamp, notary submission or installation.
    let entitlements = signing
        .entitlements
        .to_str()
        .context("The entitlements path is not valid UTF-8")?;
    signing::codesign(
        &app,
        &signing.identity,
        &[
            "--options",
            "runtime",
            "--timestamp=none",
            "--entitlements",
            entitlements,
        ],
    )?;
    signing::verify(&app)?;
    println!("Protected-Keychain test bundle: {}", app.display());
    let status = Process::spawn(util::without_release_secrets(
        util::command(&binary).args(["--exact", TEST, "--ignored", "--nocapture"]),
    ))?
    .wait(Duration::from_secs(60))?;
    ensure!(status.success(), "Protected-Keychain test failed: {status}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn exactly_one_platform_library_test_executable_is_selected() {
        let artifact = |name: &str, test: bool, exe: Option<&str>| {
            serde_json::json!({
                "reason": "compiler-artifact",
                "target": { "name": name },
                "profile": { "test": test },
                "executable": exe,
            })
            .to_string()
        };
        let messages = [
            r#"{"reason":"build-script-executed","package_id":"x"}"#.to_string(),
            artifact("newport_platform", false, None),
            artifact("newport_platform", true, Some("/t/newport_platform-1")),
            artifact("other", true, Some("/t/other-1")),
            r#"{"reason":"build-finished","success":true}"#.to_string(),
        ]
        .join("\n");
        assert_eq!(
            test_executable(&messages).unwrap(),
            Path::new("/t/newport_platform-1")
        );
        let two = format!(
            "{messages}\n{}",
            artifact("newport_platform", true, Some("/t/newport_platform-2"))
        );
        assert!(test_executable(&two).is_err());
        assert!(test_executable("").is_err());
    }

    #[test]
    fn the_test_bundle_identifies_newport() {
        let mut xml = Vec::new();
        info_plist("app.newport").to_writer_xml(&mut xml).unwrap();
        let parsed = Value::from_reader_xml(&xml[..]).unwrap();
        let d = parsed.as_dictionary().unwrap();
        assert_eq!(d["CFBundleIdentifier"].as_string(), Some("app.newport"));
        assert_eq!(d["CFBundleExecutable"].as_string(), Some("keychain-test"));
    }
}
