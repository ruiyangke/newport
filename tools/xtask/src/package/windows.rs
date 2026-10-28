//! Windows payload: the exact file allowlist, PE checks, the NSIS installer and the zip.
//! The installer is an unsigned candidate; it is never installed or published here.
use super::{Staged, copy_dir, copy_file, probe};
use crate::util::{self, Arch, Os, sha256_hex};
use crate::{build_info, grammars, pdfium, pe};
use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use std::{
    collections::BTreeSet,
    fs, io,
    path::{Component, Path, PathBuf},
    process::Command,
};

/// Payload files besides the grammars, relative to the app directory.
const FILES: &[&str] = &[
    "agents/newport-agent-x86_64.gz",
    "agents/newport-agent-aarch64.gz",
    "pdfium/pdfium.dll",
    "pdfium/LICENSE-PDFIUM",
    "Fonts/SymbolsNerdFontMono-Regular.ttf",
    "Fonts/NERD-FONTS-LICENSE",
    "Fonts/README.md",
    "LICENSE-GPUI-KIT",
    "Licenses/terminal/LICENSE-Alacritty",
    "Licenses/terminal/LICENSE-VTE",
    "Licenses/terminal/LICENSE-unicode-segmentation",
    "Licenses/terminal/LICENSE-unicode-width",
    "Licenses/terminal/README.md",
    "icon.ico",
    "newport-package.json",
];

/// Every file the Windows app directory holds, `/`-separated: `newport.exe`, the
/// grammars, then [`FILES`]. Nothing else may be packaged.
pub fn payload_files() -> Result<Vec<String>> {
    Ok(std::iter::once("newport.exe".to_owned())
        .chain(
            grammars::files(Os::Windows)?
                .into_iter()
                .map(|name| format!("grammars/{name}")),
        )
        .chain(FILES.iter().map(|f| (*f).to_owned()))
        .chain(super::notices::files()?)
        .collect())
}

/// Every directory holding a payload file, deepest first.
fn directories(files: &[String]) -> Vec<String> {
    let set: BTreeSet<String> = files
        .iter()
        .flat_map(|file| {
            let parts: Vec<&str> = file.split('/').collect();
            (1..parts.len())
                .map(|n| parts[..n].join("/"))
                .collect::<Vec<_>>()
        })
        .collect();
    let mut directories: Vec<String> = set.into_iter().collect();
    directories.sort_by(|a, b| {
        let depth = |p: &str| p.split('/').count();
        depth(b)
            .cmp(&depth(a))
            .then_with(|| a.to_lowercase().cmp(&b.to_lowercase()))
            .then_with(|| a.cmp(b))
    });
    directories
}

/// A payload file and its SHA-256.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Entry {
    pub path: String,
    pub hash: String,
}

/// An inspected app directory.
#[derive(Debug, Clone)]
pub struct Payload {
    pub app: PathBuf,
    pub arch: Arch,
    pub version: String,
    pub entries: Vec<Entry>,
    /// SHA-256 of the JSON entry list.
    pub fingerprint: String,
}

/// `<base>/<a>/<b>` for a `/`-separated relative path, with native separators.
fn join(base: &Path, relative: &str) -> PathBuf {
    relative.split('/').fold(base.to_owned(), |p, c| p.join(c))
}

/// A stable `major.minor.patch` with every part at most 65535.
fn numeric_version(version: &str) -> bool {
    let parts: Vec<&str> = version.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty()
                && part.bytes().all(|b| b.is_ascii_digit())
                && (part.len() == 1 || !part.starts_with('0'))
                && part.len() <= 5
                && part.parse::<u32>().is_ok_and(|n| n <= 65535)
        })
}

/// Require exactly the payload files as regular files, the Newport identity for
/// `arch`, a numeric version and matching PE images; fingerprint the contents.
pub fn inspect(app: &Path, arch: Arch) -> Result<Payload> {
    let files = &payload_files()?;
    let app = normalize(app)?;
    ensure!(
        fs::symlink_metadata(&app).is_ok_and(|m| m.is_dir()),
        "Expected a real staged application directory"
    );
    let directories = directories(files);
    let mut visited = BTreeSet::new();
    let mut pending = vec![(app.clone(), String::new())];
    while let Some((directory, prefix)) = pending.pop() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let name = entry.file_name();
            let relative = format!("{prefix}{}", name.to_string_lossy());
            let kind = fs::symlink_metadata(entry.path())?.file_type();
            if kind.is_symlink() {
                bail!("Package symlinks are not supported");
            }
            let known = name.to_str().is_some();
            if known && kind.is_dir() && directories.contains(&relative) {
                pending.push((entry.path(), format!("{relative}/")));
            } else if known && kind.is_file() && files.contains(&relative) {
                visited.insert(relative);
            } else {
                bail!("Unexpected package entry: {relative}");
            }
        }
    }
    if let Some(missing) = files.iter().find(|f| !visited.contains(*f)) {
        bail!("Missing package file: {missing}");
    }
    let metadata: serde_json::Value =
        serde_json::from_slice(&fs::read(app.join("newport-package.json"))?)
            .context("Invalid newport-package.json")?;
    let field = |name: &str| metadata.get(name).and_then(|v| v.as_str());
    ensure!(
        field("productName") == Some("Newport")
            && field("identifier") == Some("app.newport")
            && field("target") == Some(format!("windows-{arch}").as_str()),
        "Package identity or target does not match Newport"
    );
    let version = field("version")
        .filter(|v| numeric_version(v))
        .context("NSIS packages require a stable numeric release version")?
        .to_owned();
    pe::verify(&fs::read(app.join("newport.exe"))?, arch, false)?;
    for file in files
        .iter()
        .filter(|f| f.starts_with("grammars/") && f.ends_with(".dll"))
    {
        pe::verify(&fs::read(join(&app, file))?, arch, true)?;
    }
    pe::verify(&fs::read(join(&app, "pdfium/pdfium.dll"))?, arch, true)?;
    let entries = files
        .iter()
        .map(|path| {
            Ok(Entry {
                path: path.clone(),
                hash: util::sha256_file(&join(&app, path))?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let fingerprint = sha256_hex(serde_json::to_string(&entries)?.as_bytes());
    Ok(Payload {
        app,
        arch,
        version,
        entries,
        fingerprint,
    })
}

/// A path as an NSIS string literal. Build paths with preprocessor syntax are
/// refused instead of letting an NSIS macro expand an untrusted filesystem name.
fn quote(path: &Path) -> Result<String> {
    let value = path.to_str().context("Unsupported NSIS build path")?;
    // A canonical Windows drive path is written in its plain `C:\…` form.
    let value = match value.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => rest,
        _ => value,
    };
    ensure!(
        !value.contains(['\r', '\n', '\0', '$']),
        "Unsupported NSIS build path"
    );
    Ok(format!("\"{}\"", value.replace('"', "$\\\"")))
}

/// Lexically absolute and normalized, resolving `.` and `..` without touching the filesystem.
fn normalize(path: &Path) -> Result<PathBuf> {
    let mut normal = PathBuf::new();
    for component in std::path::absolute(path)?.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normal.pop();
            }
            other => normal.push(other),
        }
    }
    Ok(normal)
}

/// The NSIS script for an inspected payload, writing the installer to `output`
/// and including `<root>/resources/windows/installer.nsi`.
pub fn installer_source(payload: &Payload, output: &Path, root: &Path) -> Result<String> {
    let paths: Vec<String> = payload.entries.iter().map(|e| e.path.clone()).collect();
    let directories = directories(&paths);
    let own: Vec<&str> = paths
        .iter()
        .map(String::as_str)
        .chain(["uninstall.exe"])
        .collect();
    // Back up the executable first, preventing launch of mixed-version resources;
    // promote it last. Each file has a flushed write-ahead recovery record.
    let ordered: Vec<&str> = own
        .iter()
        .copied()
        .filter(|p| *p != "newport.exe")
        .chain(["newport.exe"])
        .collect();
    let native = |path: &str| path.replace('/', "\\");
    let call = |operation: &str, path: &str| {
        let id = own.iter().position(|p| *p == path).expect("owned path");
        format!("  !insertmacro {operation} {id} \"{}\"", native(path))
    };
    let block = |name: &str, lines: Vec<String>| {
        format!("!macro {name}\n{}\n!macroend\n", lines.join("\n"))
    };
    let mut extract = Vec::new();
    for path in &paths {
        let parent = path.rsplit_once('/').map_or("", |(parent, _)| parent);
        extract.push(format!("  SetOutPath \"$Stage\\{}\"", native(parent)));
        extract.push(format!("  File {}", quote(&join(&payload.app, path))?));
    }
    let remove = own
        .iter()
        .map(|path| format!("  Delete \"$INSTDIR\\{}\"", native(path)))
        // Remove only empty owned directories, deepest first. Unknown files
        // belong to the user and must survive uninstall.
        .chain(
            directories
                .iter()
                .map(|path| format!("  RMDir \"$INSTDIR\\{}\"", native(path))),
        )
        .collect();
    Ok([
        format!("!define VERSION \"{}\"", payload.version),
        format!("!define VERSION_QUAD \"{}.0\"", payload.version),
        format!("!define ARCH \"{}\"", payload.arch),
        format!("!define FINGERPRINT \"{}\"", payload.fingerprint),
        format!("!define OUTPUT {}", quote(&normalize(output)?)?),
        format!("!define ICON {}", quote(&payload.app.join("icon.ico"))?),
        block("ExtractPayload", extract),
        block(
            "BackupPayload",
            own.iter().map(|p| call("BackupFile", p)).collect(),
        ),
        block(
            "PromotePayload",
            ordered.iter().map(|p| call("PromoteFile", p)).collect(),
        ),
        block(
            "RestorePayload",
            ordered.iter().map(|p| call("RestoreFile", p)).collect(),
        ),
        block("RemovePayload", remove),
        format!(
            "!include {}",
            quote(&join(root, "resources/windows/installer.nsi"))?
        ),
    ]
    .join("\n"))
}

/// Build the installer for the staged `app` with `compiler` (`makensis`) and move it
/// to `output`. NSIS reads a verified private snapshot of the payload and writes a
/// fresh file beside `output`, which then replaces `output` atomically.
pub fn build_installer(app: &Path, arch: Arch, output: &Path, compiler: &Path) -> Result<PathBuf> {
    let payload = inspect(app, arch)?;
    let output = normalize(output)?;
    let parent = output.parent().context("Invalid installer output")?;
    // Resolve existing ancestors as well as the lexical path so a directory
    // symlink cannot redirect the output into the source application.
    let mut ancestor = parent;
    while !ancestor.exists() {
        ancestor = ancestor.parent().context("Invalid installer output")?;
    }
    let destination = fs::canonicalize(ancestor)?.join(output.strip_prefix(ancestor)?);
    ensure!(
        !destination.starts_with(fs::canonicalize(&payload.app)?),
        "Installer output must be outside the staged application"
    );
    fs::create_dir_all(parent)?;
    // Build alongside the destination so publication can use an atomic rename.
    // NSIS never writes to an existing output, which could alias a payload file.
    let temporary = tempfile::Builder::new()
        .prefix(".newport-nsis-")
        .tempdir_in(parent)?;
    let snapshot = temporary.path().join("payload");
    for entry in &payload.entries {
        let target = join(&snapshot, &entry.path);
        fs::create_dir_all(target.parent().expect("payload parent"))?;
        copy_file(&join(&payload.app, &entry.path), &target)?;
    }
    let captured = inspect(&snapshot, arch)?;
    ensure!(
        captured.fingerprint == payload.fingerprint,
        "Staged application changed while preparing the installer"
    );
    let source = temporary.path().join("installer.nsi");
    let installer = temporary.path().join("Newport-installer.exe");
    fs::write(
        &source,
        installer_source(&captured, &installer, &util::root())?,
    )?;
    let prefix = if cfg!(windows) { "/" } else { "-" };
    util::run(
        util::without_release_secrets(&mut Command::new(compiler))
            .args(["NOCONFIG", "NOCD", "WX", "V2"].map(|o| format!("{prefix}{o}")))
            .arg(&source),
    )?;
    ensure!(
        fs::symlink_metadata(&installer).is_ok_and(|m| m.is_file()),
        "NSIS did not produce an installer"
    );
    fs::rename(&installer, &output).context("Cannot publish the installer")?;
    Ok(output)
}

/// `newport-package.json`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PackageMetadata<'a> {
    product_name: &'a str,
    version: &'a str,
    identifier: &'a str,
    target: String,
}

/// Zip `app` as `<app name>/…` into `archive`, replacing it atomically.
fn zip_directory(app: &Path, archive: &Path) -> Result<()> {
    let parent = archive.parent().context("Invalid archive path")?;
    let name = app
        .file_name()
        .context("Invalid app path")?
        .to_string_lossy();
    let file = tempfile::NamedTempFile::new_in(parent)?;
    let mut zip = zip::ZipWriter::new(io::BufWriter::new(file));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .large_file(true);
    for entry in walkdir::WalkDir::new(app).sort_by_file_name() {
        let entry = entry?;
        let relative = entry.path().strip_prefix(app)?;
        let parts: Vec<String> = relative
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        let path = std::iter::once(name.to_string())
            .chain(parts)
            .collect::<Vec<_>>()
            .join("/");
        if entry.file_type().is_dir() {
            zip.add_directory(path, options)?;
        } else if entry.file_type().is_file() {
            zip.start_file(path, options)?;
            io::copy(&mut fs::File::open(entry.path())?, &mut zip)?;
        } else {
            bail!("Package symlinks are not supported");
        }
    }
    let file = zip.finish()?.into_inner().map_err(|e| e.into_error())?;
    file.persist(archive)
        .with_context(|| format!("Cannot write {}", archive.display()))?;
    Ok(())
}

/// Stage PDFium and the package metadata, run the self-checks, then build the
/// installer (with `nsis`) and the zip. Returns the zip and the installer.
pub(super) fn finish(staged: &Staged) -> Result<(PathBuf, Option<PathBuf>)> {
    let layout = staged.layout;
    let arch = staged.platform.arch;
    let pdfium = pdfium::stage(staged.release, staged.target)?;
    pdfium::validate_library(
        &fs::read(pdfium.join("pdfium.dll"))?,
        arch,
        &pdfium::pins()?,
    )?;
    copy_dir(&pdfium, &layout.resources.join("pdfium"))?;
    build_info::read(&layout.executable, staged.expect)?;
    copy_file(
        &staged.root.join("resources/icons/icon.ico"),
        &layout.app.join("icon.ico"),
    )?;
    let product = staged.product;
    let metadata = PackageMetadata {
        product_name: &product.product_name,
        version: &product.version,
        identifier: &product.identifier,
        target: format!("windows-{arch}"),
    };
    fs::write(
        layout.app.join("newport-package.json"),
        serde_json::to_string_pretty(&metadata)? + "\n",
    )?;
    for check in ["--check-needle", "--check-grammars", "--check-pdf"] {
        probe(&layout.executable, check)?;
    }
    let installer = if staged.nsis {
        let output = staged.stage.join(format!(
            "Newport-{}-windows-{arch}-setup.exe",
            product.version
        ));
        Some(build_installer(
            &layout.app,
            arch,
            &output,
            Path::new("makensis"),
        )?)
    } else {
        None
    };
    let archive = staged.stage.join("Newport-windows.zip");
    zip_directory(&layout.app, &archive)?;
    Ok((archive, installer))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::root;

    struct Fixture {
        directory: tempfile::TempDir,
        app: PathBuf,
    }

    fn fixture(arch: Arch) -> Fixture {
        let directory = tempfile::tempdir().unwrap();
        let app = directory.path().join("Newport");
        fs::create_dir_all(app.join("agents")).unwrap();
        for a in ["x86_64", "aarch64"] {
            fs::write(
                app.join(format!("agents/newport-agent-{a}.gz")),
                "Fixture payload",
            )
            .unwrap();
        }
        fs::create_dir_all(app.join("grammars")).unwrap();
        fs::create_dir_all(app.join("pdfium")).unwrap();
        fs::write(app.join("pdfium/pdfium.dll"), pe::fixture(arch, true)).unwrap();
        fs::write(app.join("pdfium/LICENSE-PDFIUM"), "Fixture license").unwrap();
        copy_dir(&root().join("resources/fonts"), &app.join("Fonts")).unwrap();
        copy_dir(&root().join("resources/licenses"), &app.join("Licenses")).unwrap();
        crate::package::notices::stage_icons(&root(), &app).unwrap();
        fs::copy(
            root().join("resources/icons/icon.ico"),
            app.join("icon.ico"),
        )
        .unwrap();
        fs::write(app.join("LICENSE-GPUI-KIT"), "Fixture license").unwrap();
        fs::write(app.join("newport.exe"), pe::fixture(arch, false)).unwrap();
        for name in grammars::files(Os::Windows).unwrap() {
            let bytes = if name.ends_with(".dll") {
                pe::fixture(arch, true)
            } else {
                b"{}".to_vec()
            };
            fs::write(app.join("grammars").join(name), bytes).unwrap();
        }
        fs::write(
            app.join("newport-package.json"),
            serde_json::json!({
                "productName": "Newport",
                "identifier": "app.newport",
                "version": "1.2.3",
                "target": format!("windows-{arch}"),
            })
            .to_string(),
        )
        .unwrap();
        Fixture { directory, app }
    }

    fn error(result: Result<impl std::fmt::Debug>) -> String {
        format!("{:#}", result.unwrap_err())
    }

    #[test]
    fn payload_lists_every_file_once_and_directories_deepest_first() {
        let files = payload_files().unwrap();
        assert_eq!(files[0], "newport.exe");
        assert!(files.contains(&"grammars/catalog.json".to_owned()));
        assert!(files.contains(&"grammars/newport_grammar_json.dll".to_owned()));
        assert_eq!(files.iter().collect::<BTreeSet<_>>().len(), files.len());
        let directories = directories(&files);
        assert!(directories.contains(&"Licenses/terminal".to_owned()));
        assert_eq!(directories.last().unwrap(), "pdfium");
        assert!(directories.contains(&"Licenses/icons/file-icons".to_owned()));
        assert!(directories.contains(&"Licenses/grammars/tree-sitter-rust".to_owned()));
        for (index, directory) in directories.iter().enumerate() {
            if let Some((parent, _)) = directory.rsplit_once('/') {
                assert!(directories.iter().position(|p| p == parent).unwrap() > index);
            }
        }
    }

    #[test]
    fn versions_must_be_stable_numeric_quads() {
        for good in ["0.0.0", "1.2.3", "65535.0.10"] {
            assert!(numeric_version(good), "{good}");
        }
        for bad in [
            "1.2",
            "1.2.3.4",
            "01.2.3",
            "1.2.3-beta",
            "65536.0.0",
            "1.2.+3",
            "",
            "99999999999.0.0",
        ] {
            assert!(!numeric_version(bad), "{bad}");
        }
    }

    #[test]
    fn rejects_mixed_architecture_data_files_symlinks_and_modified_metadata() {
        let Fixture {
            directory: _directory,
            app,
        } = fixture(Arch::X86_64);
        let baseline = inspect(&app, Arch::X86_64).unwrap();
        assert_eq!(baseline.version, "1.2.3");
        assert!(error(inspect(&app, Arch::Aarch64)).contains("target"));
        let grammar = app.join("grammars/newport_grammar_json.dll");
        fs::write(&grammar, pe::fixture(Arch::Aarch64, true)).unwrap();
        assert!(error(inspect(&app, Arch::X86_64)).contains("architecture"));
        fs::write(&grammar, pe::fixture(Arch::X86_64, true)).unwrap();
        fs::write(
            app.join("profiles.stronghold"),
            "Must never package user data",
        )
        .unwrap();
        assert!(error(inspect(&app, Arch::X86_64)).contains("Unexpected package entry"));
        fs::remove_file(app.join("profiles.stronghold")).unwrap();
        let metadata = fs::read(app.join("newport-package.json")).unwrap();
        fs::write(
            app.join("newport-package.json"),
            serde_json::json!({
                "productName": "Newport",
                "identifier": "app.newport",
                "version": "1.2.3\"\n!system bad",
                "target": "windows-x86_64",
            })
            .to_string(),
        )
        .unwrap();
        assert!(error(inspect(&app, Arch::X86_64)).contains("numeric release version"));
        fs::write(app.join("newport-package.json"), metadata).unwrap();
        fs::write(app.join("newport.exe"), "not a PE").unwrap();
        assert!(error(inspect(&app, Arch::X86_64)).contains("Invalid PE"));
        fs::write(app.join("newport.exe"), pe::fixture(Arch::X86_64, false)).unwrap();
        fs::create_dir(app.join("grammars/nested")).unwrap();
        assert!(
            error(inspect(&app, Arch::X86_64))
                .contains("Unexpected package entry: grammars/nested")
        );
        fs::remove_dir(app.join("grammars/nested")).unwrap();
        inspect(&app, Arch::X86_64).unwrap();
        #[cfg(unix)]
        {
            fs::remove_file(app.join("newport.exe")).unwrap();
            std::os::unix::fs::symlink("icon.ico", app.join("newport.exe")).unwrap();
            assert!(error(inspect(&app, Arch::X86_64)).contains("symlinks"));
            let alias = app.with_file_name("alias");
            std::os::unix::fs::symlink(&app, &alias).unwrap();
            assert!(error(inspect(&alias, Arch::X86_64)).contains("real staged application"));
        }
    }

    #[test]
    fn requires_both_needle_resources_and_fingerprints_their_contents() {
        let Fixture {
            directory: _directory,
            app,
        } = fixture(Arch::X86_64);
        let baseline = inspect(&app, Arch::X86_64).unwrap().fingerprint;
        assert_eq!(inspect(&app, Arch::X86_64).unwrap().fingerprint, baseline);
        for arch in ["x86_64", "aarch64"] {
            let path = app.join(format!("agents/newport-agent-{arch}.gz"));
            let original = fs::read(&path).unwrap();
            fs::remove_file(&path).unwrap();
            assert!(error(inspect(&app, Arch::X86_64)).contains("Missing package file: agents/"));
            fs::write(&path, "different payload").unwrap();
            assert_ne!(inspect(&app, Arch::X86_64).unwrap().fingerprint, baseline);
            fs::write(&path, original).unwrap();
        }
    }

    #[test]
    fn installer_owns_every_staged_license_and_preserves_unknown_files_on_uninstall() {
        let Fixture { directory, app } = fixture(Arch::X86_64);
        let payload = inspect(&app, Arch::X86_64).unwrap();
        let notices: Vec<_> = payload
            .entries
            .iter()
            .filter(|e| e.path.starts_with("Licenses/"))
            .collect();
        let base = root().join("resources/licenses");
        let expected: BTreeSet<_> = walkdir::WalkDir::new(&base)
            .into_iter()
            .map(Result::unwrap)
            .filter(|e| e.file_type().is_file())
            .map(|e| {
                format!(
                    "Licenses/{}",
                    e.path()
                        .strip_prefix(&base)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/")
                )
            })
            .chain(crate::package::notices::files().unwrap())
            .collect();
        assert_eq!(
            notices
                .iter()
                .map(|e| e.path.clone())
                .collect::<BTreeSet<_>>(),
            expected
        );
        assert!(notices.iter().any(|e| e.path.ends_with("LICENSE-Alacritty")));
        let source =
            installer_source(&payload, &directory.path().join("setup.exe"), &root()).unwrap();
        let removal = source
            .split("!macro RemovePayload\n")
            .nth(1)
            .unwrap()
            .split("!macroend")
            .next()
            .unwrap();
        for entry in &notices {
            let line = format!("Delete \"$INSTDIR\\{}\"", entry.path.replace('/', "\\"));
            assert!(removal.contains(&line), "{line}");
        }
        let deepest = removal
            .find("RMDir \"$INSTDIR\\Licenses\\terminal\"")
            .unwrap();
        assert!(deepest < removal.find("RMDir \"$INSTDIR\\Licenses\"").unwrap());
        assert!(!removal.to_lowercase().contains("rmdir /r"));
        for name in [
            "Licenses/terminal/LICENSE-Alacritty",
            "Licenses/icons/file-icons/LICENSE-symbols",
            "Licenses/grammars/tree-sitter-rust/LICENSE",
        ] {
            let notice = app.join(name);
            let bytes = fs::read(&notice).unwrap();
            fs::remove_file(&notice).unwrap();
            assert!(error(inspect(&app, Arch::X86_64)).contains("Missing package file"));
            fs::write(&notice, bytes).unwrap();
        }
        fs::write(
            app.join("Licenses/terminal/personal.txt"),
            "Not owned by Newport",
        )
        .unwrap();
        assert!(error(inspect(&app, Arch::X86_64)).contains("Unexpected package entry"));
    }

    #[test]
    fn installer_source_backs_up_the_executable_first_and_promotes_it_last() {
        let Fixture { directory, app } = fixture(Arch::Aarch64);
        let payload = inspect(&app, Arch::Aarch64).unwrap();
        let source =
            installer_source(&payload, &directory.path().join("setup.exe"), &root()).unwrap();
        let section = |name: &str| {
            source
                .split(&format!("!macro {name}\n"))
                .nth(1)
                .unwrap()
                .split("\n!macroend")
                .next()
                .unwrap()
                .lines()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        let count = payload.entries.len();
        let backup = section("BackupPayload");
        assert_eq!(backup.len(), count + 1);
        assert_eq!(backup[0], "  !insertmacro BackupFile 0 \"newport.exe\"");
        assert_eq!(
            backup[count],
            format!("  !insertmacro BackupFile {count} \"uninstall.exe\"")
        );
        for name in ["PromotePayload", "RestorePayload"] {
            let lines = section(name);
            assert_eq!(lines.len(), count + 1);
            assert!(lines[count].ends_with(" 0 \"newport.exe\""), "{name}");
            assert!(
                lines[0].ends_with(" 1 \"grammars\\catalog.json\""),
                "{name}"
            );
        }
        let extract = section("ExtractPayload");
        assert_eq!(extract[0], "  SetOutPath \"$Stage\\\"");
        assert_eq!(extract.len(), count * 2);
        assert!(source.starts_with(&format!(
            "!define VERSION \"1.2.3\"\n!define VERSION_QUAD \"1.2.3.0\"\n!define ARCH \"aarch64\"\n!define FINGERPRINT \"{}\"\n",
            payload.fingerprint
        )));
        assert!(source.ends_with("installer.nsi\""));
        let mut unsafe_payload = payload.clone();
        unsafe_payload.app = directory.path().join("$(bad)");
        assert!(
            error(installer_source(
                &unsafe_payload,
                &directory.path().join("setup.exe"),
                &root()
            ))
            .contains("Unsupported NSIS build path")
        );
        assert_eq!(
            quote(Path::new("C:\\a \"b\"")).unwrap(),
            "\"C:\\a $\\\"b$\\\"\""
        );
    }

    #[test]
    fn installer_output_cannot_replace_the_input_application_including_through_an_alias() {
        let Fixture { directory, app } = fixture(Arch::X86_64);
        let binary = fs::read(app.join("newport.exe")).unwrap();
        let mut outputs = vec![
            app.join("newport.exe"),
            app.join("nested/setup.exe"),
            directory
                .path()
                .join("Newport/../Newport/nested/deeper/setup.exe"),
        ];
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&app, directory.path().join("alias")).unwrap();
            outputs.push(directory.path().join("alias/nested/setup.exe"));
        }
        for output in outputs {
            let message = error(build_installer(
                &app,
                Arch::X86_64,
                &output,
                Path::new("must-not-run"),
            ));
            assert!(
                message.contains("outside the staged application"),
                "{message}"
            );
        }
        assert_eq!(fs::read(app.join("newport.exe")).unwrap(), binary);
        inspect(&app, Arch::X86_64).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn compiler_reads_a_snapshot_and_a_failed_build_preserves_the_previous_installer() {
        let Fixture { directory, app } = fixture(Arch::X86_64);
        let compiler = directory.path().join("compiler");
        let output = directory.path().join("setup.exe");
        let verified = directory.path().join("snapshot-verified");
        fs::write(&output, "previous installer").unwrap();
        // Another build replaces the staged executable after inspection; the
        // compiler must still see the original bytes in its private snapshot.
        let script = format!(
            r#"#!/bin/sh
source="$5"
printf 'concurrent rebuild' > '{staged}'
exe=$(sed -n 's/^  File "\(.*newport\.exe\)"$/\1/p' "$source")
out=$(sed -n 's/^!define OUTPUT "\(.*\)"$/\1/p' "$source")
[ -n "$exe" ] && [ "$exe" != '{staged}' ] || exit 2
[ "$(head -c 2 "$exe")" = MZ ] || exit 3
printf 'partial installer' > "$out"
printf ok > '{verified}'
exit 1
"#,
            staged = app.join("newport.exe").display(),
            verified = verified.display(),
        );
        crate::test_support::script(&compiler, &script);
        let message = error(build_installer(&app, Arch::X86_64, &output, &compiler));
        assert!(message.contains("failed"), "{message}");
        assert_eq!(fs::read_to_string(&output).unwrap(), "previous installer");
        assert_eq!(fs::read_to_string(&verified).unwrap(), "ok");
        assert!(!fs::read_dir(directory.path()).unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".newport-nsis-")
        }));
    }

    #[test]
    fn zips_the_app_directory_under_its_name() {
        let Fixture { directory, app } = fixture(Arch::X86_64);
        let archive = directory.path().join("Newport-windows.zip");
        fs::write(&archive, "previous").unwrap();
        zip_directory(&app, &archive).unwrap();
        let mut zip = zip::ZipArchive::new(fs::File::open(&archive).unwrap()).unwrap();
        let mut names: Vec<String> = zip
            .file_names()
            .filter(|n| !n.ends_with('/'))
            .map(|n| n.strip_prefix("Newport/").unwrap().to_owned())
            .collect();
        names.sort();
        let mut expected = payload_files().unwrap();
        expected.sort();
        assert_eq!(names, expected);
        let mut file = zip.by_name("Newport/newport.exe").unwrap();
        let mut bytes = Vec::new();
        io::Read::read_to_end(&mut file, &mut bytes).unwrap();
        assert_eq!(bytes, pe::fixture(Arch::X86_64, false));
    }

    /// Compiles real installers when `makensis` (or `NEWPORT_NSIS_COMPILER`) can
    /// compile a minimal script on this host.
    #[test]
    fn compiles_real_nsis_installers_for_both_architectures_without_running_them() {
        let compiler = std::env::var_os("NEWPORT_NSIS_COMPILER")
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| "makensis".into());
        let probe = tempfile::tempdir().unwrap();
        let script = probe.path().join("probe.nsi");
        fs::write(
            &script,
            format!(
                "OutFile {}\nSection\nSectionEnd\n",
                quote(&probe.path().join("probe.exe")).unwrap()
            ),
        )
        .unwrap();
        let option = if cfg!(windows) {
            "/NOCONFIG"
        } else {
            "-NOCONFIG"
        };
        if !util::succeeds(Command::new(&compiler).arg(option).arg(&script)) {
            eprintln!(
                "skipped: {} cannot compile on this host",
                compiler.to_string_lossy()
            );
            return;
        }
        for arch in [Arch::X86_64, Arch::Aarch64] {
            let Fixture { directory, app } = fixture(arch);
            let binary = fs::read(app.join("newport.exe")).unwrap();
            let destination = directory.path().join("Newport setup.exe");
            fs::hard_link(app.join("newport.exe"), &destination).unwrap();
            let output = build_installer(&app, arch, &destination, Path::new(&compiler)).unwrap();
            let bytes = fs::read(&output).unwrap();
            assert!(bytes.starts_with(b"MZ"));
            assert!(bytes.len() > 100_000);
            assert_eq!(fs::read(app.join("newport.exe")).unwrap(), binary);
        }
    }
}
