//! Release gates, signing and update manifests.
mod macos;
mod manifest;
mod publish;
mod verify;

use crate::util::{self, Arch};
use anyhow::{bail, Context, Result};
use clap::Subcommand;
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Subcommand, Debug)]
pub enum Action {
    /// Check the tag against every version and the checkout.
    Verify,
    /// CI: certificate, provisioning profile and temporary keychain.
    SetupSigning,
    /// Developer ID signing, notarization, stapling and the macOS update payload.
    Macos {
        #[arg(long)]
        skip_build: bool,
        /// Compile Remote apps in, from the private newport-remote checkout.
        #[arg(long)]
        remote_apps: bool,
    },
    /// Signed Windows update payload and manifest fragment.
    WindowsUpdate {
        installer: PathBuf,
        /// x86_64 or aarch64.
        #[arg(long)]
        arch: String,
        #[arg(long)]
        app_executable: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Merge manifest fragments, verify every signature and write SHA256SUMS.
    Merge {
        latest: PathBuf,
        fragments: Vec<PathBuf>,
        #[arg(long)]
        version: String,
        #[arg(long)]
        output: PathBuf,
    },
    /// Verify resources/agents and sign the agent release.
    SignNeedle,
    /// Verify all three signed payloads against the production signing key.
    VerifyPackages {
        manifest: PathBuf,
        #[arg(long)]
        assets: PathBuf,
    },
    /// Verify and upload a release to R2, then atomically promote its update feed.
    Deploy {
        assets: PathBuf,
        #[arg(long, default_value = "newport-releases")]
        bucket: String,
        /// Verify local files and print the upload plan without credentials or network I/O.
        #[arg(long)]
        dry_run: bool,
    },
}

pub fn run(action: Action) -> Result<()> {
    match action {
        Action::Verify => verify::run(),
        Action::SetupSigning => crate::signing::setup_ci(),
        Action::Macos {
            skip_build,
            remote_apps,
        } => macos::run(skip_build, remote_apps),
        Action::WindowsUpdate {
            installer,
            arch,
            app_executable,
            output,
        } => windows_update(&installer, &arch, &app_executable, &output),
        Action::Merge {
            latest,
            fragments,
            version,
            output,
        } => merge(latest, fragments, &version, &output),
        Action::SignNeedle => sign_needle(),
        Action::Deploy {
            assets,
            bucket,
            dry_run,
        } => publish::run(&assets, &bucket, dry_run),
        Action::VerifyPackages { manifest, assets } => {
            let manifest: manifest::Manifest = util::read_json(&manifest)?;
            anyhow::ensure!(
                manifest.platforms.len() == manifest::TARGETS.len()
                    && manifest::TARGETS
                        .iter()
                        .all(|target| manifest.platforms.contains_key(*target)),
                "Expected all three update targets"
            );
            manifest::verify_packages(&manifest, &assets, &util::updater_public_key()?)?;
            println!("All update packages match the production signing key and release version");
            Ok(())
        }
    }
}

/// Copy the installer to `Newport-<v>-windows-<arch>-setup.exe` in `output`,
/// sign it and write the `windows-<arch>.json` fragment.
fn windows_update(
    installer: &Path,
    arch: &str,
    app_executable: &Path,
    output: &Path,
) -> Result<()> {
    let arch = match arch {
        "x86_64" => Arch::X86_64,
        "aarch64" => Arch::Aarch64,
        _ => bail!("--arch must be x86_64 or aarch64"),
    };
    let version = util::app()?.version;
    let key = util::signing_key()?;
    let executable = fs::read(app_executable)
        .with_context(|| format!("Cannot read {}", app_executable.display()))?;
    crate::pe::verify(&executable, arch, false)?;
    fs::create_dir_all(output)?;
    let payload = output.join(format!("Newport-{version}-windows-{arch}-setup.exe"));
    fs::copy(installer, &payload)
        .with_context(|| format!("Cannot copy {}", installer.display()))?;
    let fragment = manifest::write_fragment(
        &payload,
        &format!("windows-{arch}"),
        &format!("windows-{arch}.json"),
        &version,
        &util::updater_public_key()?,
        &key,
    )?;
    println!("Update fragment: {}", fragment.display());
    Ok(())
}

/// Merge `latest` and `fragments` into `output`, verify every payload signature
/// and write `SHA256SUMS`. Payloads are read from the output's directory.
fn merge(latest: PathBuf, fragments: Vec<PathBuf>, version: &str, output: &Path) -> Result<()> {
    let output = std::path::absolute(output)?;
    let assets = output.parent().context("Invalid --output")?;
    let name = output
        .file_name()
        .and_then(|n| n.to_str())
        .context("Invalid --output")?;
    let paths: Vec<_> = std::iter::once(latest).chain(fragments).collect();
    let manifest = manifest::merge(&paths, version, assets)?;
    manifest::verify_packages(&manifest, assets, &util::updater_public_key()?)?;
    manifest.write(&output)?;
    manifest::write_checksums(&manifest, assets, name)?;
    println!("Release manifest: {}", output.display());
    Ok(())
}

/// Bind the verified Linux agent payloads to this release version.
fn sign_needle() -> Result<()> {
    let agents = util::root().join("resources/agents");
    crate::artifact::verify(&agents)?;
    let key = util::signing_key()?;
    newport_release::sign_needle(
        &agents,
        &util::app()?.version,
        &util::updater_public_key()?,
        &key,
    )?;
    println!("Release signature written and verified");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn read(path: &str) -> String {
        fs::read_to_string(util::root().join(path)).unwrap()
    }

    #[test]
    fn release_retains_the_installed_product_and_updater_identity() {
        let app = util::app().unwrap();
        assert_eq!(app.product_name, "Newport");
        assert_eq!(app.identifier, "app.newport");
        let cargo: toml::Table = read("crates/newport-app/Cargo.toml").parse().unwrap();
        assert_eq!(
            cargo["package"]["version"].as_str(),
            Some(app.version.as_str())
        );
        let updater: Value = serde_json::from_str(&read("resources/updater.json")).unwrap();
        // Update signatures are always verified; the app reads only these two.
        assert_eq!(
            updater.as_object().unwrap().keys().collect::<Vec<_>>(),
            ["endpoints", "pubkey"]
        );
        assert_eq!(
            updater["endpoints"],
            serde_json::json!(["https://newport.app/updates/latest.json"])
        );
        // Installed releases trust this key; rotating it would strand their updates.
        assert_eq!(
            util::updater_public_key().unwrap(),
            "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDc3MjIzQTY2RTM2QkJERDIKUldUU3ZXdmpaam9pZDhjZjFHN1ZGWmlSZ1crL2NoRUZYd0JUU0RGbE9tc1k4OWN6dnZkOFFnOTYK"
        );
        for icon in ["icon.icns", "icon.ico", "icon.png", "tray.png"] {
            let path = util::root().join("resources/icons").join(icon);
            assert!(fs::metadata(&path).unwrap().len() > 0, "{icon}");
        }
    }

    #[test]
    fn desktop_release_verification_does_not_enable_private_key_signing() {
        for package in ["newport-release", "newport-backend"] {
            let tree = util::capture(util::command("cargo").args([
                "tree",
                "--locked",
                "-p",
                package,
                "--no-default-features",
                "--edges",
                "normal,build",
                "--prefix",
                "none",
            ]))
            .unwrap();
            let has = |name: &str| tree.lines().any(|l| l.starts_with(&format!("{name} v")));
            assert!(has("minisign-verify"), "{package}");
            assert!(!has("minisign"), "{package}");
            assert!(!has("newport-needle-release"), "{package}");
        }
        let manifest: toml::Table = read("crates/newport-release/Cargo.toml").parse().unwrap();
        assert_eq!(
            manifest["features"]["default"].as_array().map(Vec::len),
            Some(0)
        );
        let binary = &manifest["bin"].as_array().unwrap()[0];
        assert_eq!(
            binary["required-features"],
            toml::Value::Array(vec!["signing".into()])
        );
    }
}
