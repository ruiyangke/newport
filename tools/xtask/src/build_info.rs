//! `newport --build-info` checks against `resources/app.json`, and the macOS Info.plist.
use crate::{process::Process, util};
use anyhow::{bail, ensure, Context, Result};
use serde::Deserialize;
use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildInfo {
    pub schema: u32,
    pub product_name: String,
    pub identifier: String,
    pub version: String,
    pub os: String,
    pub arch: String,
    pub debug_assertions: bool,
}

/// Optional expectations beyond the product identity.
#[derive(Debug, Clone, Copy, Default)]
pub struct Expect<'a> {
    pub os: Option<&'a str>,
    pub arch: Option<&'a str>,
    pub debug: Option<bool>,
}

const OUTPUT_LIMIT: usize = 4096;

/// Stdout of `binary --build-info`, bounded in time and size.
fn output(binary: &Path, timeout: Duration) -> Result<Vec<u8>> {
    let deadline = Instant::now() + timeout;
    let mut child = Process::spawn(
        Command::new(binary)
            .arg("--build-info")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null()),
    )?;
    let stdout = child.0.stdout.take().context("No stdout")?;
    let reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(OUTPUT_LIMIT as u64 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    while !reader.is_finished() {
        ensure!(Instant::now() < deadline, "Timed out");
        thread::sleep(Duration::from_millis(10));
    }
    let bytes = reader
        .join()
        .map_err(|_| anyhow::anyhow!("Output reader panicked"))??;
    ensure!(bytes.len() <= OUTPUT_LIMIT, "Output too large");
    let status = child.wait(deadline.saturating_duration_since(Instant::now()))?;
    ensure!(status.success(), "Exited with {status}");
    Ok(bytes)
}

fn check(info: &BuildInfo, product: &util::App, expect: Expect) -> Result<()> {
    if info.schema != 1
        || info.product_name != product.product_name
        || info.identifier != product.identifier
        || info.version != product.version
        || expect.os.is_some_and(|os| info.os != os)
        || expect.arch.is_some_and(|arch| info.arch != arch)
        || expect
            .debug
            .is_some_and(|debug| info.debug_assertions != debug)
    {
        bail!("The app binary does not match the requested package; rebuild before packaging");
    }
    Ok(())
}

fn read_with(binary: &Path, expect: Expect, timeout: Duration) -> Result<BuildInfo> {
    let info: BuildInfo = output(binary, timeout)
        .and_then(|bytes| Ok(serde_json::from_slice(&bytes)?))
        .context("Cannot read the app binary's build metadata; rebuild before packaging")?;
    check(&info, &util::app()?, expect)?;
    Ok(info)
}

/// Run `<binary> --build-info` and require it to match the product and `expect`.
pub fn read(binary: &Path, expect: Expect) -> Result<BuildInfo> {
    read_with(binary, expect, Duration::from_secs(30))
}

fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// The bundle's Info.plist XML.
pub fn mac_info_plist(info: &BuildInfo) -> String {
    let identifier = xml(&info.identifier);
    let name = xml(&info.product_name);
    let version = xml(&info.version);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>{identifier}</string>
<key>CFBundleExecutable</key><string>newport</string>
<key>CFBundleName</key><string>{name}</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleIconFile</key><string>icon.icns</string>
<key>CFBundleVersion</key><string>{version}</string>
<key>CFBundleShortVersionString</key><string>{version}</string>
<key>ATSApplicationFontsPath</key><string>Fonts/</string>
<key>NSHighResolutionCapable</key><true/>
<key>LSMinimumSystemVersion</key><string>14.0</string>
</dict></plist>"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn info(product: &util::App) -> BuildInfo {
        BuildInfo {
            schema: 1,
            product_name: product.product_name.clone(),
            identifier: product.identifier.clone(),
            version: product.version.clone(),
            os: "macos".into(),
            arch: "aarch64".into(),
            debug_assertions: false,
        }
    }

    #[test]
    fn requires_the_product_identity_and_expectations() {
        let product = util::app().unwrap();
        let good = info(&product);
        check(&good, &product, Expect::default()).unwrap();
        check(
            &good,
            &product,
            Expect {
                os: Some("macos"),
                arch: Some("aarch64"),
                debug: Some(false),
            },
        )
        .unwrap();
        for expect in [
            Expect {
                os: Some("windows"),
                ..Expect::default()
            },
            Expect {
                arch: Some("x86_64"),
                ..Expect::default()
            },
            Expect {
                debug: Some(true),
                ..Expect::default()
            },
        ] {
            assert!(check(&good, &product, expect).is_err());
        }
        for change in [
            |i: &mut BuildInfo| i.schema = 2,
            |i: &mut BuildInfo| i.version.push('1'),
            |i: &mut BuildInfo| i.identifier.push('x'),
            |i: &mut BuildInfo| i.product_name.push('x'),
        ] {
            let mut bad = good.clone();
            change(&mut bad);
            let error = check(&bad, &product, Expect::default()).unwrap_err();
            assert!(error.to_string().contains("does not match"));
        }
    }

    #[cfg(unix)]
    fn script(directory: &Path, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = directory.join("newport");
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    #[cfg(unix)]
    #[test]
    fn reads_bounded_binary_output() {
        let product = util::app().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let json = serde_json::json!({
            "schema": 1,
            "productName": product.product_name,
            "identifier": product.identifier,
            "version": product.version,
            "os": "macos",
            "arch": "aarch64",
            "debugAssertions": true,
        });
        let binary = script(
            directory.path(),
            &format!("test \"$1\" = --build-info && echo '{json}'"),
        );
        let read = read(
            &binary,
            Expect {
                debug: Some(true),
                ..Expect::default()
            },
        )
        .unwrap();
        assert_eq!(read.arch, "aarch64");
        let timeout = Duration::from_secs(5);
        for body in ["exit 3", "echo not-json", "head -c 5000 /dev/zero"] {
            let binary = script(directory.path(), body);
            let error = read_with(&binary, Expect::default(), timeout).unwrap_err();
            assert!(
                error.to_string().contains("Cannot read the app binary"),
                "{body}"
            );
        }
        let binary = script(directory.path(), "exec sleep 10");
        let started = Instant::now();
        assert!(read_with(&binary, Expect::default(), Duration::from_millis(200)).is_err());
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn info_plist_escapes_values() {
        let product = util::app().unwrap();
        let mut value = info(&product);
        value.product_name = "A&B <\"'>".into();
        let plist = mac_info_plist(&value);
        let parsed = plist::Value::from_reader_xml(plist.as_bytes()).unwrap();
        let dict = parsed.as_dictionary().unwrap();
        assert_eq!(dict["CFBundleName"].as_string(), Some("A&B <\"'>"));
        assert_eq!(
            dict["CFBundleIdentifier"].as_string(),
            Some(product.identifier.as_str())
        );
        assert_eq!(
            dict["CFBundleShortVersionString"].as_string(),
            Some(product.version.as_str())
        );
        assert_eq!(dict["CFBundleExecutable"].as_string(), Some("newport"));
    }
}
