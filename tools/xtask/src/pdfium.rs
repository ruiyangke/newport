//! Pinned PDFium for Windows, verified by archive, library and license hashes.
use crate::util::{self, Arch, Platform, sha256_hex};
use crate::{pe, root};
use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

/// `resources/pdfium.json`.
#[derive(Debug, Deserialize)]
pub struct Pdfium {
    pub release: String,
    pub source: String,
    pub architectures: BTreeMap<String, Pin>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pin {
    pub archive: String,
    pub sha256: String,
    pub library_sha256: String,
    pub license_sha256: String,
}

pub fn pins() -> Result<Pdfium> {
    util::read_json(&root().join("resources/pdfium.json"))
}

/// Require the pinned library for `arch`, as a PE32+ DLL.
pub fn validate_library(bytes: &[u8], arch: Arch, pins: &Pdfium) -> Result<()> {
    let pin = pins.architectures.get(arch.as_str());
    ensure!(
        pin.is_some_and(|pin| sha256_hex(bytes) == pin.library_sha256),
        "PDF renderer does not match the pinned library"
    );
    pe::verify(bytes, arch, true)
}

/// The Windows architecture for an explicit target, else the Windows host.
fn windows_arch(target: Option<&str>) -> Result<Arch> {
    match target {
        Some("aarch64-pc-windows-msvc") => Ok(Arch::Aarch64),
        Some("x86_64-pc-windows-msvc") => Ok(Arch::X86_64),
        None if cfg!(windows) => Ok(Platform::host().arch),
        _ => bail!("Specify a Windows target for PDFium staging"),
    }
}

fn download(url: &str) -> Result<Vec<u8>> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(60)))
        .build()
        .into();
    let mut response = match agent.get(url).call() {
        Err(ureq::Error::StatusCode(status)) => bail!("PDFium download failed: {status}"),
        result => result.context("PDFium download failed")?,
    };
    response
        .body_mut()
        .with_config()
        .limit(256 * 1024 * 1024)
        .read_to_vec()
        .context("PDFium download failed")
}

/// True when `output` already holds the pinned library and license notice.
fn staged(output: &Path, arch: Arch, pins: &Pdfium, pin: &Pin) -> bool {
    let (Ok(library), Ok(license)) = (
        fs::read(output.join("pdfium.dll")),
        fs::read(output.join("LICENSE-PDFIUM")),
    ) else {
        return false;
    };
    validate_library(&library, arch, pins).is_ok() && sha256_hex(&license) == pin.license_sha256
}

/// Write `name` in `output` through a temporary sibling and a rename.
fn replace(output: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    let pending = output.join(format!("{name}.new-{}", std::process::id()));
    let result = fs::write(&pending, bytes).and_then(|()| fs::rename(&pending, output.join(name)));
    let _ = fs::remove_file(&pending);
    result.with_context(|| format!("Cannot stage {name}"))
}

/// Stage the pinned PDFium for `arch` in `output`, fetching the archive with `download`.
fn stage_into(
    output: &Path,
    arch: Arch,
    pins: &Pdfium,
    download: &dyn Fn(&str) -> Result<Vec<u8>>,
) -> Result<()> {
    let pin = pins
        .architectures
        .get(arch.as_str())
        .context("Unsupported PDFium architecture")?;
    let output = std::path::absolute(output)?;
    if let Ok(metadata) = fs::symlink_metadata(&output) {
        ensure!(
            metadata.is_dir(),
            "PDFium staging destination must be a real directory"
        );
    }
    if staged(&output, arch, pins, pin) {
        return Ok(());
    }
    let archive = download(&format!(
        "{}/releases/download/{}/{}",
        pins.source, pins.release, pin.archive
    ))?;
    ensure!(
        sha256_hex(&archive) == pin.sha256,
        "PDFium archive checksum mismatch"
    );
    let temporary = tempfile::Builder::new()
        .prefix("newport-pdfium-")
        .tempdir()?;
    let unpacked = temporary.path();
    tar::Archive::new(flate2::read::GzDecoder::new(archive.as_slice()))
        .unpack(unpacked)
        .context("Cannot unpack PDFium")?;
    let library =
        fs::read(unpacked.join("bin/pdfium.dll")).context("PDFium archive has no library")?;
    validate_library(&library, arch, pins)?;
    let mut licenses: Vec<String> = fs::read_dir(unpacked.join("licenses"))
        .context("PDFium archive has no licenses")?
        .map(|entry| Ok(format!("licenses/{}", entry?.file_name().to_string_lossy())))
        .collect::<Result<_>>()?;
    licenses.sort();
    licenses.insert(0, "LICENSE".into());
    let notice = licenses
        .iter()
        .map(|name| {
            let text =
                fs::read(unpacked.join(name)).with_context(|| format!("Cannot read {name}"))?;
            Ok(format!(
                "===== {name} =====\n{}\n",
                String::from_utf8_lossy(&text)
            ))
        })
        .collect::<Result<Vec<_>>>()?
        .join("\n");
    ensure!(
        sha256_hex(notice.as_bytes()) == pin.license_sha256,
        "PDFium license checksum mismatch"
    );
    // Stage complete files before replacing build output. Packages are validated
    // again before copying; this directory is never an installed application.
    fs::create_dir_all(&output)?;
    replace(&output, "LICENSE-PDFIUM", notice.as_bytes())?;
    replace(&output, "pdfium.dll", &library)
}

/// Stage `pdfium.dll` and `LICENSE-PDFIUM` in `<target>/<profile>/pdfium`; returns that directory.
pub fn stage(release: bool, target: Option<&str>) -> Result<PathBuf> {
    let target = util::build_target(target);
    let arch = windows_arch(target.as_deref())?;
    let output = util::profile_directory(target.as_deref(), release)?.join("pdfium");
    let pins = pins()?;
    stage_into(&output, arch, &pins, &download)?;
    println!("Staged PDFium {}: {}", pins.release, output.display());
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_download(_: &str) -> Result<Vec<u8>> {
        panic!("must not download")
    }

    #[test]
    fn rejects_an_altered_archive_before_touching_staged_files() {
        let pins = pins().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("pdfium");
        fs::create_dir(&output).unwrap();
        fs::write(output.join("pdfium.dll"), "previous library").unwrap();
        fs::write(output.join("LICENSE-PDFIUM"), "previous notices").unwrap();
        let error = stage_into(&output, Arch::Aarch64, &pins, &|url| {
            assert!(url.ends_with("/pdfium-win-arm64.tgz"), "{url}");
            Ok(b"altered archive".to_vec())
        })
        .unwrap_err();
        assert!(error.to_string().contains("checksum mismatch"), "{error}");
        assert_eq!(
            fs::read_to_string(output.join("pdfium.dll")).unwrap(),
            "previous library"
        );
        assert_eq!(
            fs::read_to_string(output.join("LICENSE-PDFIUM")).unwrap(),
            "previous notices"
        );
        let error = validate_library(b"MZ", Arch::X86_64, &pins).unwrap_err();
        assert!(error.to_string().contains("pinned library"));
    }

    #[test]
    fn rejects_unsupported_targets_and_aliased_output_before_download() {
        let directory = tempfile::tempdir().unwrap();
        let empty = Pdfium {
            release: String::new(),
            source: String::new(),
            architectures: BTreeMap::new(),
        };
        let error = stage_into(
            &directory.path().join("out"),
            Arch::X86_64,
            &empty,
            &no_download,
        )
        .unwrap_err();
        assert!(error.to_string().contains("Unsupported"));
        for target in [
            "aarch64-apple-darwin",
            "x86_64-pc-windows-gnu",
            "armv7-pc-windows-msvc",
        ] {
            let error = windows_arch(Some(target)).unwrap_err();
            assert!(error.to_string().contains("Windows target"), "{target}");
        }
        assert_eq!(
            windows_arch(Some("aarch64-pc-windows-msvc")).unwrap(),
            Arch::Aarch64
        );
        assert_eq!(
            windows_arch(Some("x86_64-pc-windows-msvc")).unwrap(),
            Arch::X86_64
        );
        let file = directory.path().join("file");
        fs::write(&file, "").unwrap();
        let pins = pins().unwrap();
        let error = stage_into(&file, Arch::X86_64, &pins, &no_download).unwrap_err();
        assert!(error.to_string().contains("real directory"));
        #[cfg(unix)]
        {
            let destination = directory.path().join("destination");
            fs::create_dir(&destination).unwrap();
            std::os::unix::fs::symlink(&destination, directory.path().join("alias")).unwrap();
            let error = stage_into(
                &directory.path().join("alias"),
                Arch::X86_64,
                &pins,
                &no_download,
            )
            .unwrap_err();
            assert!(error.to_string().contains("real directory"));
        }
    }

    /// An archive in the release layout whose hashes are pinned by the returned `Pdfium`.
    fn fixture(arch: Arch) -> (Vec<u8>, Pdfium) {
        let library = pe::fixture(arch, true);
        let mut tar = tar::Builder::new(Vec::new());
        for (name, bytes) in [
            ("bin/pdfium.dll", library.as_slice()),
            ("LICENSE", b"main".as_slice()),
            ("licenses/zlib.txt", b"zlib".as_slice()),
            ("licenses/abseil.txt", b"abseil".as_slice()),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, name, bytes).unwrap();
        }
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut gz, &tar.into_inner().unwrap()).unwrap();
        let archive = gz.finish().unwrap();
        let notice = "===== LICENSE =====\nmain\n\n===== licenses/abseil.txt =====\nabseil\n\n===== licenses/zlib.txt =====\nzlib\n";
        let pins = Pdfium {
            release: "chromium/1".into(),
            source: "https://example.invalid".into(),
            architectures: BTreeMap::from([(
                arch.as_str().to_owned(),
                Pin {
                    archive: "pdfium.tgz".into(),
                    sha256: sha256_hex(&archive),
                    library_sha256: sha256_hex(&library),
                    license_sha256: sha256_hex(notice.as_bytes()),
                },
            )]),
        };
        (archive, pins)
    }

    #[test]
    fn stages_a_verified_archive_and_skips_valid_output() {
        let (archive, pins) = fixture(Arch::Aarch64);
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("pdfium");
        let downloads = std::cell::Cell::new(0);
        let download = |url: &str| {
            assert_eq!(
                url,
                "https://example.invalid/releases/download/chromium/1/pdfium.tgz"
            );
            downloads.set(downloads.get() + 1);
            Ok(archive.clone())
        };
        stage_into(&output, Arch::Aarch64, &pins, &download).unwrap();
        assert_eq!(
            fs::read(output.join("pdfium.dll")).unwrap(),
            pe::fixture(Arch::Aarch64, true)
        );
        assert!(
            fs::read_to_string(output.join("LICENSE-PDFIUM"))
                .unwrap()
                .starts_with("===== LICENSE =====\nmain\n")
        );
        let mut names: Vec<_> = fs::read_dir(&output)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, ["LICENSE-PDFIUM", "pdfium.dll"]);
        stage_into(&output, Arch::Aarch64, &pins, &download).unwrap();
        assert_eq!(downloads.get(), 1);
        fs::write(output.join("pdfium.dll"), "tampered").unwrap();
        stage_into(&output, Arch::Aarch64, &pins, &download).unwrap();
        assert_eq!(downloads.get(), 2);
        assert_eq!(
            fs::read(output.join("pdfium.dll")).unwrap(),
            pe::fixture(Arch::Aarch64, true)
        );
    }
}
