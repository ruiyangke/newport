//! The macOS distribution: Developer ID signing, notarization, stapling and the
//! signed update tarball with its manifest fragment.
use super::manifest;
use crate::{
    build_info, grammars, package, signing,
    util::{self, Os},
};
use anyhow::{bail, ensure, Context, Result};
use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use newport_release::SecretKey;
use serde::Deserialize;
use std::{
    env, fs,
    io::BufWriter,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

const TARGET: &str = "aarch64-apple-darwin";
const BUNDLE: &str = "Newport.app";

/// A command whose environment carries no release secret.
fn clean(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut c = util::command(program);
    util::without_release_secrets(&mut c);
    c
}

pub fn run(skip_build: bool) -> Result<()> {
    ensure!(
        cfg!(target_os = "macos"),
        "macOS distributions must be built on macOS"
    );
    let var = |name: &str| env::var(name).ok().filter(|v| !v.is_empty());
    let required = |name: &str| var(name).with_context(|| format!("{name} is required"));
    let identity = required("APPLE_SIGNING_IDENTITY")?;
    ensure!(
        identity.starts_with("Developer ID Application:"),
        "Distribution requires a Developer ID Application identity"
    );
    let profile = std::path::absolute(required("NEWPORT_PROVISIONING_PROFILE")?)?;
    let entitlements = std::path::absolute(required("NEWPORT_SIGNING_ENTITLEMENTS")?)?;
    let auth = notarization_auth(var)?;
    let key = util::signing_key()?;
    let public_key = util::updater_public_key()?;
    // Both credentials are in memory now. Clearing them from this process means
    // no build, build script or packaged app started below can inherit them.
    for name in util::RELEASE_SECRETS {
        env::remove_var(name);
    }
    env::set_var("CARGO_INCREMENTAL", "0");
    signing::check(&entitlements, &profile, &util::app()?.identifier)?;

    let package = package::run(&package::Options {
        target: Some(TARGET.into()),
        skip_build,
        ..Default::default()
    })?;
    let app = &package.app;
    let executable = app.join("Contents/MacOS/newport");
    build_info::read(
        &executable,
        build_info::Expect {
            os: Some("macos"),
            arch: Some("aarch64"),
            debug: Some(false),
        },
    )?;
    fs::copy(&profile, app.join("Contents/embedded.provisionprofile"))
        .context("Cannot embed the provisioning profile")?;
    // Nested code first, then the bundle. Only the host receives the
    // restricted Keychain entitlements and the matching profile.
    let runtime = ["--options", "runtime", "--timestamp"];
    for name in grammars::files(Os::Macos)?
        .iter()
        .filter(|n| n.ends_with(".dylib"))
    {
        signing::codesign(
            &app.join("Contents/Resources/grammars").join(name),
            &identity,
            &runtime,
        )?;
    }
    let entitlements = entitlements
        .to_str()
        .context("Non-UTF-8 entitlements path")?;
    let mut host = runtime.to_vec();
    host.extend(["--entitlements", entitlements]);
    signing::codesign(app, &identity, &host)?;
    verify_code_signature(app)?;
    util::run(clean(&executable).arg("--check-grammars"))?;

    let distribution = package.stage.join("distribution");
    fs::create_dir_all(&distribution)?;
    let submission = distribution.join("notarization.zip");
    ditto_zip(app, &submission)?;
    submit_notarization(&submission, &auth)?;
    util::run(clean("xcrun").args(["stapler", "staple"]).arg(app))?;
    // Revalidates notarization, Gatekeeper and architecture before signing the update.
    let update = distribution.join("update");
    package_update(app, &update, &public_key, &key)?;
    println!("Signed update candidate: {}", update.display());
    Ok(())
}

/// `notarytool` credentials: a keychain profile, else Apple ID, app password and team.
pub fn notarization_auth(var: impl Fn(&str) -> Option<String>) -> Result<Vec<String>> {
    if let Some(profile) = var("NEWPORT_NOTARY_PROFILE") {
        return Ok(vec!["--keychain-profile".into(), profile]);
    }
    let mut auth = Vec::new();
    for (name, flag) in [
        ("APPLE_ID", "--apple-id"),
        ("APPLE_PASSWORD", "--password"),
        ("APPLE_TEAM_ID", "--team-id"),
    ] {
        let Some(value) = var(name) else {
            bail!("Set NEWPORT_NOTARY_PROFILE or APPLE_ID, APPLE_PASSWORD and APPLE_TEAM_ID");
        };
        auth.extend([flag.to_owned(), value]);
    }
    Ok(auth)
}

/// Submit and wait. Errors never include the command line or its output,
/// which carry the notarization password.
fn submit_notarization(archive: &Path, auth: &[String]) -> Result<()> {
    #[derive(Deserialize)]
    struct Submission {
        id: Option<String>,
        status: Option<String>,
    }
    let output = clean("xcrun")
        .arg("notarytool")
        .arg("submit")
        .arg(archive)
        .args(auth)
        .args(["--wait", "--output-format", "json"])
        .stdin(Stdio::null())
        .output();
    let output = match output {
        Ok(output) if output.status.success() => output.stdout,
        _ => bail!("Notarization submission failed"),
    };
    let result: Submission =
        serde_json::from_slice(&output).context("notarytool printed invalid JSON")?;
    let status = result.status.as_deref().unwrap_or("unknown");
    ensure!(
        status == "Accepted",
        "Notarization was not accepted ({status}); see `xcrun notarytool log {}`",
        result.id.as_deref().unwrap_or("<id>")
    );
    Ok(())
}

fn verify_code_signature(app: &Path) -> Result<()> {
    util::run(
        clean("codesign")
            .args(["--verify", "--deep", "--strict"])
            .arg(app),
    )
}

/// `ditto -c -k --sequesterRsrc --keepParent`: a zip that keeps the bundle and its stapled ticket.
fn ditto_zip(app: &Path, zip: &Path) -> Result<()> {
    util::run(
        clean("ditto")
            .args(["-c", "-k", "--sequesterRsrc", "--keepParent"])
            .arg(app)
            .arg(zip),
    )
}

/// The signed update payload for a stapled app in `output`: the
/// `<name>.app.tar.gz` with `.sig`, the `latest.json` fragment and the `<name>.zip` download.
pub fn package_update(app: &Path, output: &Path, public_key: &str, key: &SecretKey) -> Result<()> {
    let info = util::read_plist(&app.join("Contents/Info.plist"))?;
    let info = info
        .as_dictionary()
        .context("Info.plist is not a dictionary")?;
    let string = |name: &str| {
        info.get(name)
            .and_then(plist::Value::as_string)
            .with_context(|| format!("Info.plist has no {name}"))
    };
    let version = string("CFBundleShortVersionString")?;
    let config = util::app()?;
    ensure!(
        version == config.version && string("CFBundleIdentifier")? == config.identifier,
        "App version/identifier does not match the release source"
    );
    verify_code_signature(app)?;
    util::run(clean("xcrun").args(["stapler", "validate"]).arg(app))?;
    util::run(
        clean("spctl")
            .args(["--assess", "--type", "execute"])
            .arg(app),
    )?;
    let executable = app
        .join("Contents/MacOS")
        .join(string("CFBundleExecutable")?);
    ensure!(
        util::capture(clean("lipo").arg("-archs").arg(&executable))? == "arm64",
        "This release pipeline currently packages Apple Silicon only"
    );
    fs::create_dir_all(output)?;
    let name = manifest::macos_name(version);
    let archive = output.join(format!("{name}.app.tar.gz"));
    write_bundle_archive(app, &archive)?;
    verify_bundle_archive(&archive)?;
    manifest::write_fragment(
        &archive,
        "darwin-aarch64",
        "latest.json",
        version,
        public_key,
        key,
    )?;
    ditto_zip(app, &output.join(format!("{name}.zip")))?;
    println!("Release artifacts: {}", output.display());
    Ok(())
}

/// Gzipped tar with a single `Newport.app` root. Symlinks stay links and modes
/// are kept; no extended attributes or AppleDouble sidecars are written.
pub fn write_bundle_archive(app: &Path, archive: &Path) -> Result<()> {
    let file = fs::File::create(archive)
        .with_context(|| format!("Cannot create {}", archive.display()))?;
    let mut tar = tar::Builder::new(GzEncoder::new(BufWriter::new(file), Compression::default()));
    tar.follow_symlinks(false);
    tar.append_dir_all(BUNDLE, app)
        .with_context(|| format!("Cannot archive {}", app.display()))?;
    let file = tar
        .into_inner()?
        .finish()?
        .into_inner()
        .map_err(|e| e.into_error())?;
    file.sync_all()?;
    Ok(())
}

/// Extract the archive and require the extracted bundle's code signature to verify.
fn verify_bundle_archive(archive: &Path) -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let app = unpack_bundle_archive(archive, temporary.path())?;
    verify_code_signature(&app).context("The update archive does not preserve the signed bundle")
}

fn unpack_bundle_archive(archive: &Path, into: &Path) -> Result<PathBuf> {
    let mut tar = tar::Archive::new(GzDecoder::new(fs::File::open(archive)?));
    tar.set_preserve_permissions(true);
    tar.set_preserve_mtime(true);
    tar.unpack(into)
        .with_context(|| format!("Cannot extract {}", archive.display()))?;
    Ok(into.join(BUNDLE))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<_, _> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |name| map.get(name).cloned()
    }

    #[test]
    fn notarization_supports_a_keychain_profile_and_ci_credentials() {
        assert_eq!(
            notarization_auth(vars(&[("NEWPORT_NOTARY_PROFILE", "release")])).unwrap(),
            ["--keychain-profile", "release"]
        );
        let ci = [
            ("APPLE_ID", "release@example.invalid"),
            ("APPLE_PASSWORD", "fixture"),
            ("APPLE_TEAM_ID", "TEAM"),
        ];
        assert_eq!(
            notarization_auth(vars(&ci)).unwrap(),
            [
                "--apple-id",
                "release@example.invalid",
                "--password",
                "fixture",
                "--team-id",
                "TEAM"
            ]
        );
        for missing in 0..ci.len() {
            let mut partial = ci.to_vec();
            partial.remove(missing);
            let error = notarization_auth(vars(&partial)).unwrap_err().to_string();
            assert!(error.contains("Set NEWPORT_NOTARY_PROFILE"), "{error}");
            assert!(!error.contains("fixture"), "{error}");
        }
    }

    #[test]
    #[cfg(unix)]
    fn bundle_archives_keep_symlinks_modes_and_one_root() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("Build.app");
        let macos = app.join("Contents/MacOS");
        let deep = app.join(format!("Contents/Resources/{}", "nested/".repeat(30)));
        fs::create_dir_all(&macos).unwrap();
        fs::create_dir_all(&deep).unwrap();
        fs::write(macos.join("newport"), "binary").unwrap();
        fs::set_permissions(macos.join("newport"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(deep.join("data"), "resource").unwrap();
        fs::set_permissions(deep.join("data"), fs::Permissions::from_mode(0o640)).unwrap();
        std::os::unix::fs::symlink("MacOS/newport", app.join("Contents/Current")).unwrap();

        let archive = dir.path().join("update.app.tar.gz");
        write_bundle_archive(&app, &archive).unwrap();
        let mut tar = tar::Archive::new(GzDecoder::new(fs::File::open(&archive).unwrap()));
        for entry in tar.entries().unwrap() {
            let entry = entry.unwrap();
            let path = entry.path().unwrap().into_owned();
            assert!(path.starts_with(BUNDLE), "{}", path.display());
            assert!(!path.to_string_lossy().contains("._"), "{}", path.display());
        }

        let out = dir.path().join("out");
        let unpacked = unpack_bundle_archive(&archive, &out).unwrap();
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&unpacked.join("Contents/MacOS/newport")), 0o755);
        let data = unpacked.join(format!("Contents/Resources/{}data", "nested/".repeat(30)));
        assert_eq!(mode(&data), 0o640);
        assert_eq!(fs::read_to_string(data).unwrap(), "resource");
        let link = unpacked.join("Contents/Current");
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read_link(link).unwrap(), Path::new("MacOS/newport"));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn archived_bundles_keep_their_code_signature() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join(BUNDLE);
        fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        fs::create_dir_all(app.join("Contents/Resources")).unwrap();
        fs::write(
            app.join("Contents/Info.plist"),
            r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>app.newport.archive-test</string>
<key>CFBundleExecutable</key><string>newport</string>
</dict></plist>"#,
        )
        .unwrap();
        fs::copy("/usr/bin/true", app.join("Contents/MacOS/newport")).unwrap();
        fs::write(app.join("Contents/Resources/data.txt"), "resource").unwrap();
        std::os::unix::fs::symlink("data.txt", app.join("Contents/Resources/alias.txt")).unwrap();
        util::run(
            Command::new("codesign")
                .args(["--force", "--sign", "-"])
                .arg(&app)
                .stderr(Stdio::null()),
        )
        .unwrap();
        let archive = dir.path().join("update.app.tar.gz");
        write_bundle_archive(&app, &archive).unwrap();
        verify_bundle_archive(&archive).unwrap();

        fs::write(app.join("Contents/Resources/data.txt"), "tampered").unwrap();
        write_bundle_archive(&app, &archive).unwrap();
        assert!(verify_bundle_archive(&archive).is_err());
    }
}
