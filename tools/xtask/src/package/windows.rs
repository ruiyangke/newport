//! Windows payload: the exact file allowlist, PE checks, the NSIS installer and the zip.
//! The installer is an unsigned candidate; it is never installed or published here.
use super::{Staged, copy_dir, copy_file, probe};
use crate::util::{self, Arch, Os, sha256_hex};
use crate::{build_info, grammars, pdfium, pe};
use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use sha2::{Digest, Sha256};
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
    "LICENSE",
    "LICENSE-GPUI-KIT",
    "icon.ico",
    "newport-package.json",
];

/// Where the Full agent export's notices go.
const AGENT_NOTICES: &str = "agents/licenses/";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Composition {
    Core,
    Full,
}

impl Composition {
    fn from_metadata(metadata: &serde_json::Value) -> Result<Self> {
        match metadata.get("remoteApps") {
            // Packages predating this field contain only the Core payload.
            None | Some(serde_json::Value::Bool(false)) => Ok(Self::Core),
            Some(serde_json::Value::Bool(true)) => Ok(Self::Full),
            Some(_) => bail!("Package remoteApps must be a boolean"),
        }
    }
}

/// Every file the Windows app directory holds besides the Full agent's
/// notices, `/`-separated: `newport.exe`, the grammars, [`FILES`], then the
/// `Licenses`.
pub fn payload_files() -> Result<Vec<String>> {
    Ok(std::iter::once("newport.exe".to_owned())
        .chain(
            grammars::files(Os::Windows)?
                .into_iter()
                .map(|name| format!("grammars/{name}")),
        )
        .chain(FILES.iter().map(|f| (*f).to_owned()))
        .chain(super::license_files())
        .collect())
}

/// What `newport-package.json` lists: [`payload_files`], then the notices
/// of the Full agent export a Full package embeds.
fn package_files(agent_notices: &[String]) -> Result<Vec<String>> {
    let mut files = payload_files()?;
    files.extend(
        agent_notices
            .iter()
            .map(|name| format!("{AGENT_NOTICES}{name}")),
    );
    Ok(files)
}

/// The files `newport-package.json` lists, checked against [`package_files`]:
/// the payload in its order, then only for Full, the agent notices.
fn listed_files(metadata: &serde_json::Value, composition: Composition) -> Result<Vec<String>> {
    let files: Vec<String> = metadata
        .get("files")
        .cloned()
        .map(serde_json::from_value)
        .context("Package metadata must list its files")?
        .context("Package files must be a list of paths")?;
    let payload = payload_files()?;
    ensure!(
        files.get(..payload.len()) == Some(&payload[..]),
        "Package files differ from the Windows payload"
    );
    let notices = &files[payload.len()..];
    ensure!(
        notices.is_empty() == (composition == Composition::Core),
        "Full packages, and only they, carry the agent's notices"
    );
    let mut unique = BTreeSet::new();
    for path in notices {
        ensure!(
            path.strip_prefix(AGENT_NOTICES).is_some_and(filename) && unique.insert(path),
            "Invalid agent notice in the package files: {path}"
        );
    }
    Ok(files)
}

/// A plain file name, as an agent notice must have.
fn filename(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
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
    /// SHA-256 of the payload and installer ownership inventories.
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
/// `arch`, a semantic version and matching PE images; fingerprint the contents.
pub fn inspect(app: &Path, arch: Arch) -> Result<Payload> {
    let app = normalize(app)?;
    ensure!(
        fs::symlink_metadata(&app).is_ok_and(|m| m.is_dir() && !is_reparse(&m)),
        "Expected a real staged application directory"
    );
    let metadata_path = app.join("newport-package.json");
    let metadata_kind = fs::symlink_metadata(&metadata_path)?;
    ensure!(
        metadata_kind.is_file() && !is_reparse(&metadata_kind),
        "Expected regular package metadata"
    );
    let metadata: serde_json::Value = serde_json::from_slice(&fs::read(&metadata_path)?)
        .context("Invalid newport-package.json")?;
    let files = &listed_files(&metadata, Composition::from_metadata(&metadata)?)?;
    let directories = directories(files);
    let mut visited = BTreeSet::new();
    let mut pending = vec![(app.clone(), String::new())];
    while let Some((directory, prefix)) = pending.pop() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let name = entry.file_name();
            let relative = format!("{prefix}{}", name.to_string_lossy());
            let metadata = fs::symlink_metadata(entry.path())?;
            let kind = metadata.file_type();
            if kind.is_symlink() || is_reparse(&metadata) {
                bail!("Package symlinks and reparse points are not supported");
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
    let field = |name: &str| metadata.get(name).and_then(|v| v.as_str());
    ensure!(
        field("productName") == Some("Newport")
            && field("identifier") == Some("app.newport")
            && field("target") == Some(format!("windows-{arch}").as_str()),
        "Package identity or target does not match Newport"
    );
    let version = field("version")
        .filter(|v| semver::Version::parse(v).is_ok())
        .context("Packages require a semantic version")?
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
    let fingerprint = sha256_hex(&serde_json::to_vec(&(&entries, owned_paths(&entries)))?);
    Ok(Payload {
        app,
        arch,
        version,
        entries,
        fingerprint,
    })
}

/// What the installer owns: the payload, the notices a Full package carries
/// even when this one is Core (so replacing Full with Core, rollback and
/// uninstall account for them), and the uninstaller.
fn owned_paths(entries: &[Entry]) -> Vec<String> {
    let retired = crate::remote_apps::full_agent::notice_names()
        .map(|name| format!("{AGENT_NOTICES}{name}"))
        .filter(|notice| !entries.iter().any(|entry| entry.path == *notice));
    entries
        .iter()
        .map(|entry| entry.path.clone())
        .chain(retired)
        .chain(["uninstall.exe".to_owned()])
        .collect()
}

#[cfg(windows)]
fn is_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_reparse(_: &fs::Metadata) -> bool {
    false
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
    ensure!(
        numeric_version(&payload.version),
        "NSIS packages require a stable numeric release version"
    );
    let paths: Vec<String> = payload.entries.iter().map(|e| e.path.clone()).collect();
    let own = owned_paths(&payload.entries);
    let directories = directories(&own);
    // Back up the executable first, preventing launch of mixed-version resources;
    // promote it last. Each file has a flushed write-ahead recovery record.
    let promoted: Vec<&str> = paths
        .iter()
        .map(String::as_str)
        .chain(["uninstall.exe"])
        .filter(|p| *p != "newport.exe")
        .chain(["newport.exe"])
        .collect();
    let restored: Vec<&str> = own
        .iter()
        .map(String::as_str)
        .filter(|p| *p != "newport.exe")
        .chain(["newport.exe"])
        .collect();
    let native = |path: &str| path.replace('/', "\\");
    let call = |operation: &str, path: &str| {
        let id = own.iter().position(|p| p == path).expect("owned path");
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
            promoted.iter().map(|p| call("PromoteFile", p)).collect(),
        ),
        block(
            "RestorePayload",
            restored.iter().map(|p| call("RestoreFile", p)).collect(),
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
    ensure!(
        numeric_version(&payload.version),
        "NSIS packages require a stable numeric release version"
    );
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
    remote_apps: bool,
    /// Every file in the app directory, as [`package_files`] lists them.
    files: Vec<String>,
}

/// Zip `app` as `<app name>/…` into `archive`, replacing it atomically.
fn zip_directory(app: &Path, arch: Arch, archive: &Path) -> Result<()> {
    let payload = inspect(app, arch)?;
    let parent = archive.parent().context("Invalid archive path")?;
    let name = app
        .file_name()
        .context("Invalid app path")?
        .to_str()
        .context("Invalid application directory name")?;
    let file = tempfile::NamedTempFile::new_in(parent)?;
    let mut zip = zip::ZipWriter::new(io::BufWriter::new(file));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .large_file(true);
    zip.add_directory(format!("{name}/"), options)?;
    let paths = payload
        .entries
        .iter()
        .map(|entry| entry.path.clone())
        .collect::<Vec<_>>();
    for directory in directories(&paths).into_iter().rev() {
        zip.add_directory(format!("{name}/{directory}/"), options)?;
    }
    for entry in &payload.entries {
        zip.start_file(format!("{name}/{}", entry.path), options)?;
        let mut source = fs::File::open(join(app, &entry.path))?;
        let mut hash = Sha256::new();
        let mut buffer = [0; 64 * 1024];
        loop {
            let count = io::Read::read(&mut source, &mut buffer)?;
            if count == 0 {
                break;
            }
            hash.update(&buffer[..count]);
            io::Write::write_all(&mut zip, &buffer[..count])?;
        }
        let actual: String = hash
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        ensure!(
            actual == entry.hash,
            "Staged application changed while preparing the archive"
        );
    }
    let file = zip.finish()?.into_inner().map_err(|e| e.into_error())?;
    file.persist(archive)
        .with_context(|| format!("Cannot write {}", archive.display()))?;
    Ok(())
}

/// The notices a package embeds from its agent export: the export's own
/// list for Full, none for Core.
fn agent_notices(remote_apps: bool, export: Option<&Path>) -> Result<Vec<String>> {
    if !remote_apps {
        return Ok(Vec::new());
    }
    let export = export.context(
        "Full Windows packages list their agent export's notices; for --skip-build set NEWPORT_AGENT_RESOURCE_DIR",
    )?;
    crate::remote_apps::full_agent::exported_notices(export)
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
    let remote_apps = staged
        .expect
        .remote_apps
        .context("Package composition is required")?;
    let agent_notices = agent_notices(remote_apps, staged.agent_export)?;
    let metadata = PackageMetadata {
        product_name: &product.product_name,
        version: &product.version,
        identifier: &product.identifier,
        target: format!("windows-{arch}"),
        remote_apps,
        files: package_files(&agent_notices)?,
    };
    fs::write(
        layout.app.join("newport-package.json"),
        serde_json::to_string_pretty(&metadata)? + "\n",
    )?;
    for check in ["--check-needle", "--check-grammars", "--check-pdf"] {
        probe(&layout.executable, check)?;
    }
    if let Some(export) = staged.signed_agent_export {
        newport_release::desktop_agent::verify_desktop_agent_for(
            newport_release::desktop_agent::PackageLayout::Windows,
            &layout.app,
            export,
            &product.version,
            &util::updater_public_key()?,
            &newport_release::desktop_agent::run_windows_probe,
        )
        .context("The packaged desktop does not embed the requested signed agent export")?;
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
    zip_directory(&layout.app, arch, &archive)?;
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
        fixture_with_composition(arch, Composition::Core)
    }

    /// The notices a Full fixture carries, by file name.
    fn agent_notice_names() -> Vec<String> {
        crate::remote_apps::full_agent::notice_names()
            .map(str::to_owned)
            .collect()
    }

    /// The same, as package paths.
    fn full_notices() -> Vec<String> {
        agent_notice_names()
            .iter()
            .map(|name| format!("{AGENT_NOTICES}{name}"))
            .collect()
    }

    fn fixture_with_composition(arch: Arch, composition: Composition) -> Fixture {
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
        if composition == Composition::Full {
            fs::create_dir(app.join("agents/licenses")).unwrap();
            for name in full_notices() {
                fs::write(app.join(&name), format!("Fixture full notice: {name}\n")).unwrap();
            }
        }
        fs::create_dir_all(app.join("grammars")).unwrap();
        fs::create_dir_all(app.join("pdfium")).unwrap();
        fs::write(app.join("pdfium/pdfium.dll"), pe::fixture(arch, true)).unwrap();
        fs::write(app.join("pdfium/LICENSE-PDFIUM"), "Fixture license").unwrap();
        copy_dir(&root().join("resources/fonts"), &app.join("Fonts")).unwrap();
        crate::package::stage_icon_notices(&root(), &app).unwrap();
        fs::write(
            app.join("Licenses").join(crate::notices::FILE),
            "Fixture notices",
        )
        .unwrap();
        fs::write(app.join("LICENSE"), "Fixture license").unwrap();
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
                "remoteApps": composition == Composition::Full,
                "files": package_files(&if composition == Composition::Full {
                    agent_notice_names()
                } else {
                    Vec::new()
                })
                .unwrap(),
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
        assert!(files.contains(&format!("Licenses/{}", crate::notices::FILE)));
        assert!(files.contains(&"LICENSE".to_owned()));
        assert_eq!(directories.last().unwrap(), "pdfium");
        assert!(directories.contains(&"Licenses/icons/file-icons".to_owned()));
        assert!(directories.contains(&"Licenses/icons".to_owned()));
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
        fs::write(app.join("profiles.json"), "Must never package user data").unwrap();
        assert!(error(inspect(&app, Arch::X86_64)).contains("Unexpected package entry"));
        fs::remove_file(app.join("profiles.json")).unwrap();
        let metadata = fs::read(app.join("newport-package.json")).unwrap();
        let mut changed: serde_json::Value = serde_json::from_slice(&metadata).unwrap();
        changed["version"] = "1.2.3\"\n!system bad".into();
        fs::write(app.join("newport-package.json"), changed.to_string()).unwrap();
        assert!(error(inspect(&app, Arch::X86_64)).contains("semantic version"));
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
        assert_eq!(
            notices
                .iter()
                .map(|e| e.path.clone())
                .collect::<BTreeSet<_>>(),
            crate::package::license_files().into_iter().collect()
        );
        let source =
            installer_source(&payload, &directory.path().join("setup.exe"), &root()).unwrap();
        let removal = source
            .split("!macro RemovePayload\n")
            .nth(1)
            .unwrap()
            .split("!macroend")
            .next()
            .unwrap();
        for path in notices.iter().map(|e| e.path.as_str()).chain(["LICENSE"]) {
            let line = format!("Delete \"$INSTDIR\\{}\"", path.replace('/', "\\"));
            assert!(removal.contains(&line), "{line}");
        }
        let deepest = removal
            .find("RMDir \"$INSTDIR\\Licenses\\icons\\file-icons\"")
            .unwrap();
        assert!(deepest < removal.find("RMDir \"$INSTDIR\\Licenses\\icons\"").unwrap());
        assert!(
            removal.find("RMDir \"$INSTDIR\\Licenses\\icons\"").unwrap()
                < removal.find("RMDir \"$INSTDIR\\Licenses\"").unwrap()
        );
        assert!(!removal.to_lowercase().contains("rmdir /r"));
        for name in [
            format!("Licenses/{}", crate::notices::FILE),
            "Licenses/icons/file-icons/LICENSE-symbols".to_owned(),
            "LICENSE".to_owned(),
        ] {
            let notice = app.join(&name);
            let bytes = fs::read(&notice).unwrap();
            fs::remove_file(&notice).unwrap();
            assert!(
                error(inspect(&app, Arch::X86_64)).contains("Missing package file"),
                "{name}"
            );
            fs::write(&notice, bytes).unwrap();
        }
        fs::write(app.join("Licenses/personal.txt"), "Not owned by Newport").unwrap();
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
        let owned = count + full_notices().len();
        let backup = section("BackupPayload");
        assert_eq!(backup.len(), owned + 1);
        assert_eq!(backup[0], "  !insertmacro BackupFile 0 \"newport.exe\"");
        assert_eq!(
            backup[owned],
            format!("  !insertmacro BackupFile {owned} \"uninstall.exe\"")
        );
        for name in ["PromotePayload", "RestorePayload"] {
            let lines = section(name);
            let last = if name == "PromotePayload" {
                count
            } else {
                owned
            };
            assert_eq!(lines.len(), last + 1);
            assert!(lines[last].ends_with(" 0 \"newport.exe\""), "{name}");
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
        let outputs = vec![
            app.join("newport.exe"),
            app.join("nested/setup.exe"),
            directory
                .path()
                .join("Newport/../Newport/nested/deeper/setup.exe"),
        ];
        #[cfg(unix)]
        let outputs = {
            let mut outputs = outputs;
            std::os::unix::fs::symlink(&app, directory.path().join("alias")).unwrap();
            outputs.push(directory.path().join("alias/nested/setup.exe"));
            outputs
        };
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
        zip_directory(&app, Arch::X86_64, &archive).unwrap();
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

    #[test]
    fn full_agent_notices_survive_zip_and_installer_payload_generation() {
        let Fixture { directory, app } = fixture_with_composition(Arch::X86_64, Composition::Full);
        let payload = inspect(&app, Arch::X86_64).unwrap();
        let source =
            installer_source(&payload, &directory.path().join("setup.exe"), &root()).unwrap();
        let archive = directory.path().join("Newport.zip");
        zip_directory(&app, Arch::X86_64, &archive).unwrap();
        let mut zip = zip::ZipArchive::new(fs::File::open(&archive).unwrap()).unwrap();
        let actual: BTreeSet<_> = zip
            .file_names()
            .filter(|name| !name.ends_with('/'))
            .map(|name| name.strip_prefix("Newport/").unwrap().to_owned())
            .collect();
        assert_eq!(
            actual,
            package_files(&agent_notice_names())
                .unwrap()
                .into_iter()
                .collect()
        );
        for name in &full_notices() {
            let expected = fs::read(app.join(name)).unwrap();
            let entry = payload
                .entries
                .iter()
                .find(|entry| entry.path == *name)
                .unwrap();
            assert_eq!(entry.hash, sha256_hex(&expected));
            let native_path = app.join(name.replace('/', std::path::MAIN_SEPARATOR_STR));
            assert!(source.contains(&format!("  File {}", quote(&native_path).unwrap())));
            let mut bytes = Vec::new();
            io::Read::read_to_end(
                &mut zip.by_name(&format!("Newport/{name}")).unwrap(),
                &mut bytes,
            )
            .unwrap();
            assert_eq!(bytes, expected, "{name}");
        }
    }

    #[test]
    fn full_notice_inventory_rejects_missing_extra_and_nonboolean_composition_before_output() {
        let Fixture { directory, app } = fixture_with_composition(Arch::X86_64, Composition::Full);
        let archive = directory.path().join("Newport.zip");
        let installer = directory.path().join("setup.exe");
        fs::write(&archive, "previous zip").unwrap();
        fs::write(&installer, "previous installer").unwrap();
        let rejected = || {
            assert!(inspect(&app, Arch::X86_64).is_err());
            assert!(zip_directory(&app, Arch::X86_64, &archive).is_err());
            assert!(
                build_installer(&app, Arch::X86_64, &installer, Path::new("must-not-run")).is_err()
            );
            assert_eq!(fs::read(&archive).unwrap(), b"previous zip");
            assert_eq!(fs::read(&installer).unwrap(), b"previous installer");
        };
        for name in full_notices() {
            let path = app.join(name);
            let bytes = fs::read(&path).unwrap();
            fs::remove_file(&path).unwrap();
            rejected();
            fs::write(path, bytes).unwrap();
        }
        let extra = app.join("agents/licenses/private-source.rs");
        fs::write(&extra, "must not package arbitrary files").unwrap();
        rejected();
        fs::remove_file(extra).unwrap();
        let metadata = app.join("newport-package.json");
        let original: serde_json::Value =
            serde_json::from_slice(&fs::read(&metadata).unwrap()).unwrap();
        for invalid in [
            serde_json::Value::Null,
            serde_json::json!("true"),
            serde_json::json!(1),
            serde_json::json!(false),
        ] {
            let mut changed = original.clone();
            changed["remoteApps"] = invalid;
            fs::write(&metadata, serde_json::to_vec(&changed).unwrap()).unwrap();
            rejected();
        }
        let mut missing = original.clone();
        missing.as_object_mut().unwrap().remove("remoteApps");
        fs::write(&metadata, serde_json::to_vec(&missing).unwrap()).unwrap();
        rejected();
        fs::write(&metadata, serde_json::to_vec(&original).unwrap()).unwrap();
        inspect(&app, Arch::X86_64).unwrap();
    }

    #[test]
    fn listed_files_are_the_payload_then_only_a_full_packages_notices() {
        let payload = payload_files().unwrap();
        let full = package_files(&agent_notice_names()).unwrap();
        let metadata = |files: &[String]| serde_json::json!({ "files": files });
        assert_eq!(
            listed_files(&metadata(&payload), Composition::Core).unwrap(),
            payload
        );
        assert_eq!(
            listed_files(&metadata(&full), Composition::Full).unwrap(),
            full
        );
        assert!(listed_files(&metadata(&full), Composition::Core).is_err());
        assert!(listed_files(&metadata(&payload), Composition::Full).is_err());
        assert!(listed_files(&serde_json::json!({}), Composition::Core).is_err());
        assert!(
            listed_files(
                &serde_json::json!({ "files": "newport.exe" }),
                Composition::Core
            )
            .is_err()
        );
        let mut reordered = payload.clone();
        reordered.swap(0, 1);
        assert!(listed_files(&metadata(&reordered), Composition::Core).is_err());
        for notice in [
            "agents/licenses/../newport.exe",
            "agents/licenses/nested/notice",
            "agents/licenses/",
            "Licenses/extra",
        ] {
            let mut files = full.clone();
            files.push(notice.into());
            assert!(
                listed_files(&metadata(&files), Composition::Full).is_err(),
                "{notice}"
            );
        }
        let mut duplicate = full.clone();
        duplicate.push(full.last().unwrap().clone());
        assert!(listed_files(&metadata(&duplicate), Composition::Full).is_err());
    }

    #[test]
    fn a_notice_added_to_the_export_reaches_the_package_with_no_other_edit() {
        let Fixture { directory, app } = fixture_with_composition(Arch::X86_64, Composition::Full);
        // The build stages whatever the export holds; this one has a notice no
        // list in this repository names.
        let added = format!("{AGENT_NOTICES}zlib-LICENSE");
        fs::write(app.join(&added), "Fixture full notice: zlib\n").unwrap();
        let export = directory.path().join("export");
        fs::create_dir_all(export.join("licenses")).unwrap();
        fs::write(export.join("newport-agent-x86_64"), "Fixture payload").unwrap();
        for entry in fs::read_dir(app.join(AGENT_NOTICES)).unwrap() {
            let entry = entry.unwrap();
            fs::copy(
                entry.path(),
                export.join("licenses").join(entry.file_name()),
            )
            .unwrap();
        }
        crate::artifact::record(&export).unwrap();
        super::super::check_agent_notices(&app, &export).unwrap();
        let metadata_path = app.join("newport-package.json");
        let mut metadata: serde_json::Value =
            serde_json::from_slice(&fs::read(&metadata_path).unwrap()).unwrap();
        metadata["files"] =
            serde_json::json!(package_files(&agent_notices(true, Some(&export)).unwrap()).unwrap());
        fs::write(&metadata_path, metadata.to_string()).unwrap();
        let payload = inspect(&app, Arch::X86_64).unwrap();
        assert!(payload.entries.iter().any(|entry| entry.path == added));
        let source =
            installer_source(&payload, &directory.path().join("setup.exe"), &root()).unwrap();
        let native = added.replace('/', "\\");
        assert!(source.contains(&format!("  File {}", quote(&join(&app, &added)).unwrap())));
        assert!(source.contains(&format!("Delete \"$INSTDIR\\{native}\"")));
        let archive = directory.path().join("Newport.zip");
        zip_directory(&app, Arch::X86_64, &archive).unwrap();
        let mut zip = zip::ZipArchive::new(fs::File::open(&archive).unwrap()).unwrap();
        assert!(zip.by_name(&format!("Newport/{added}")).is_ok());
        assert!(error(agent_notices(true, None)).contains("NEWPORT_AGENT_RESOURCE_DIR"));
        assert!(agent_notices(false, None).unwrap().is_empty());
    }

    #[test]
    fn core_metadata_without_composition_remains_core_and_prereleases_can_be_zipped() {
        let Fixture { directory, app } = fixture(Arch::X86_64);
        let path = app.join("newport-package.json");
        let mut metadata: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        metadata.as_object_mut().unwrap().remove("remoteApps");
        metadata["version"] = "1.2.3-beta.1".into();
        fs::write(path, serde_json::to_vec(&metadata).unwrap()).unwrap();
        let payload = inspect(&app, Arch::X86_64).unwrap();
        assert_eq!(
            payload
                .entries
                .iter()
                .map(|entry| entry.path.clone())
                .collect::<Vec<_>>(),
            payload_files().unwrap()
        );
        zip_directory(&app, Arch::X86_64, &directory.path().join("preview.zip")).unwrap();
        let output = directory.path().join("setup.exe");
        assert!(
            error(build_installer(
                &app,
                Arch::X86_64,
                &output,
                Path::new("must-not-run")
            ))
            .contains("stable numeric")
        );
        assert!(installer_source(&payload, &output, &root()).is_err());
        assert!(!output.exists());
    }

    #[test]
    fn composition_changes_back_up_and_restore_known_notices_without_promoting_retired_files() {
        for composition in [Composition::Core, Composition::Full] {
            let Fixture { directory, app } = fixture_with_composition(Arch::X86_64, composition);
            let payload = inspect(&app, Arch::X86_64).unwrap();
            assert_ne!(
                payload.fingerprint,
                sha256_hex(&serde_json::to_vec(&payload.entries).unwrap()),
                "An older ownership inventory must not share the recovery fingerprint"
            );
            let source =
                installer_source(&payload, &directory.path().join("setup.exe"), &root()).unwrap();
            let body = |name: &str| {
                source
                    .split(&format!("!macro {name}\n"))
                    .nth(1)
                    .unwrap()
                    .split("\n!macroend")
                    .next()
                    .unwrap()
                    .to_owned()
            };
            let operations = |name: &str| -> std::collections::BTreeMap<usize, String> {
                body(name)
                    .lines()
                    .map(|line| {
                        let (_, operation) = line.trim().split_once(' ').unwrap();
                        let (_, arguments) = operation.split_once(' ').unwrap();
                        let (id, path) = arguments.split_once(' ').unwrap();
                        (
                            id.parse().unwrap(),
                            path.trim_matches('"').replace('\\', "/"),
                        )
                    })
                    .collect()
            };
            let backup = operations("BackupPayload");
            let restore = operations("RestorePayload");
            let promote = operations("PromotePayload");
            assert_eq!(backup, restore);
            assert_eq!(
                backup.keys().copied().collect::<Vec<_>>(),
                (0..backup.len()).collect::<Vec<_>>()
            );
            assert_eq!(backup[&0], "newport.exe");
            assert_eq!(
                backup.len(),
                package_files(&agent_notice_names()).unwrap().len() + 1
            );
            assert_eq!(promote.len(), payload.entries.len() + 1);
            for (id, path) in &promote {
                assert_eq!(&backup[id], path);
            }
            let remove = body("RemovePayload");
            let extract = body("ExtractPayload");
            for name in &full_notices() {
                assert!(backup.values().any(|path| path == name));
                assert_eq!(
                    promote.values().any(|path| path == name),
                    composition == Composition::Full
                );
                let native_path = app.join(name.replace('/', std::path::MAIN_SEPARATOR_STR));
                assert_eq!(
                    extract.contains(&quote(&native_path).unwrap()),
                    composition == Composition::Full
                );
                assert!(
                    remove.contains(&format!("Delete \"$INSTDIR\\{}\"", name.replace('/', "\\")))
                );
            }
            assert!(remove.contains("RMDir \"$INSTDIR\\agents\\licenses\""));
            assert!(!remove.to_ascii_lowercase().contains("rmdir /r"));
            assert!(!remove.contains('*'));
            assert!(!source.contains("personal.txt"));
            for name in ["PromotePayload", "RestorePayload"] {
                assert!(
                    body(name)
                        .lines()
                        .last()
                        .unwrap()
                        .ends_with(" 0 \"newport.exe\"")
                );
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn full_installer_compiler_receives_a_verified_notice_snapshot() {
        let Fixture { directory, app } = fixture_with_composition(Arch::X86_64, Composition::Full);
        let compiler = directory.path().join("compiler");
        crate::test_support::script(
            &compiler,
            r#"#!/bin/sh
source="$5"
notice=$(sed -n 's/^  File "\(.*agents\/licenses\/newport-full.txt\)"$/\1/p' "$source")
out=$(sed -n 's/^!define OUTPUT "\(.*\)"$/\1/p' "$source")
[ -n "$notice" ] && [ -n "$out" ] || exit 2
[ "$(cat "$notice")" = 'Fixture full notice: agents/licenses/newport-full.txt' ] || exit 3
printf 'fixture installer' > "$out"
"#,
        );
        let output = directory.path().join("setup.exe");
        build_installer(&app, Arch::X86_64, &output, &compiler).unwrap();
        assert_eq!(fs::read(&output).unwrap(), b"fixture installer");
        inspect(&app, Arch::X86_64).unwrap();
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
            for composition in [Composition::Core, Composition::Full] {
                let Fixture { directory, app } = fixture_with_composition(arch, composition);
                let binary = fs::read(app.join("newport.exe")).unwrap();
                let destination = directory.path().join("Newport setup.exe");
                fs::hard_link(app.join("newport.exe"), &destination).unwrap();
                let output =
                    build_installer(&app, arch, &destination, Path::new(&compiler)).unwrap();
                let bytes = fs::read(&output).unwrap();
                assert!(bytes.starts_with(b"MZ"));
                assert!(bytes.len() > 100_000);
                assert_eq!(fs::read(app.join("newport.exe")).unwrap(), binary);
            }
        }
    }

    #[test]
    fn uninstall_removes_remote_app_shortcuts_before_newport_and_keeps_the_profile() {
        let script = fs::read_to_string(root().join("resources/windows/installer.nsi")).unwrap();
        let uninstall = script
            .split("Section \"Uninstall\"\n")
            .nth(1)
            .unwrap()
            .split("SectionEnd")
            .next()
            .unwrap();
        let removal = uninstall
            .find(r#"ExecWait '"$INSTDIR\newport.exe" --remove-remote-apps' $0"#)
            .expect("Newport removes its remote apps' shortcuts");
        // Newport can't run once its executable is gone.
        let executable = uninstall.find(r#"Delete "$INSTDIR\newport.exe""#).unwrap();
        assert!(removal < executable);
        let fallback = &uninstall[removal..executable];
        assert!(fallback.contains(r#"Delete "$SMPROGRAMS\Newport Apps\*.lnk""#));
        assert!(fallback.contains(r#"RMDir "$SMPROGRAMS\Newport Apps""#));
        assert!(!uninstall.to_lowercase().contains("rmdir /r"));
        assert!(!uninstall.contains("remote-apps.json"));
    }
}
