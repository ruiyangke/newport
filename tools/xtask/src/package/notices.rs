//! Upstream notices travel with the assets and grammar libraries they describe.
use super::copy_file;
use crate::util;
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::{collections::BTreeSet, fs, path::Path};

const SOURCES: &str = include_str!("../../../../resources/licenses/grammars/SOURCES.json");
const ICON_FILES: &str = include_str!("../../../../resources/licenses/icons/FILES.json");

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Inventory {
    schema: u32,
    packages: Vec<Package>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Package {
    name: String,
    version: String,
    repository: String,
    revision: String,
    crate_sha256: String,
    notices: Vec<Notice>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Notice {
    file: String,
    source: String,
    archive_path: Option<String>,
    sha256: String,
}

fn inventory() -> Result<Inventory> {
    serde_json::from_str(SOURCES).context("Invalid grammar notice inventory")
}

fn filename(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
}

fn icon_files() -> Result<Vec<String>> {
    let files: Vec<String> = serde_json::from_str(ICON_FILES)?;
    let mut unique = BTreeSet::new();
    for path in &files {
        ensure!(
            path.split_once('/').is_some_and(|(directory, name)| {
                matches!(directory, "file-icons" | "process-icons") && filename(name)
            }) && unique.insert(path),
            "Invalid or duplicate icon notice path"
        );
    }
    Ok(files)
}

fn grammar_files(inventory: &Inventory) -> Result<Vec<String>> {
    ensure!(
        inventory.schema == 1,
        "Unsupported grammar notice inventory"
    );
    let mut names = BTreeSet::new();
    let mut paths = BTreeSet::from(["README.md".to_owned(), "SOURCES.json".to_owned()]);
    for package in &inventory.packages {
        ensure!(
            package.name.starts_with("tree-sitter-")
                && filename(&package.name)
                && names.insert(&package.name)
                && !package.notices.is_empty(),
            "Invalid or duplicate grammar notice package: {}",
            package.name
        );
        for notice in &package.notices {
            ensure!(
                filename(&notice.file) && paths.insert(format!("{}/{}", package.name, notice.file)),
                "Invalid or duplicate grammar notice path"
            );
        }
    }
    Ok(paths.into_iter().collect())
}

pub(super) fn files() -> Result<Vec<String>> {
    Ok(grammar_files(&inventory()?)?
        .into_iter()
        .map(|p| format!("Licenses/grammars/{p}"))
        .chain([
            "Licenses/icons/README.md".to_owned(),
            "Licenses/icons/FILES.json".to_owned(),
        ])
        .chain(icon_files()?.iter().map(|p| format!("Licenses/icons/{p}")))
        .collect())
}

fn validate_grammars(root: &Path, inventory: &Inventory) -> Result<()> {
    let expected_files: BTreeSet<_> = grammar_files(inventory)?.into_iter().collect();
    let manifest: toml::Value = toml::from_str(&fs::read_to_string(
        root.join("crates/newport-grammars/Cargo.toml"),
    )?)?;
    let dependencies = manifest["dependencies"]
        .as_table()
        .context("Grammar dependency manifest has no dependencies")?;
    let expected_packages: BTreeSet<_> = dependencies
        .keys()
        .filter(|name| name.starts_with("tree-sitter-"))
        .map(String::as_str)
        .collect();
    ensure!(
        expected_packages == inventory.packages.iter().map(|p| p.name.as_str()).collect(),
        "Grammar notice inventory does not match grammar dependencies"
    );
    let lock: toml::Value = toml::from_str(&fs::read_to_string(root.join("Cargo.lock"))?)?;
    let locked = lock["package"]
        .as_array()
        .context("Cargo lockfile has no packages")?;
    let directory = root.join("resources/licenses/grammars");
    for package in &inventory.packages {
        let requirement = dependencies[&package.name]
            .as_str()
            .or_else(|| dependencies[&package.name].get("version")?.as_str())
            .context("Grammar dependency has no version")?;
        ensure!(
            semver::VersionReq::parse(requirement)?
                .matches(&semver::Version::parse(&package.version)?),
            "Grammar notice version does not match dependency {}",
            package.name
        );
        let versions: Vec<_> = locked
            .iter()
            .filter(|p| p.get("name").and_then(toml::Value::as_str) == Some(&package.name))
            .collect();
        ensure!(
            versions.len() == 1
                && versions[0].get("version").and_then(toml::Value::as_str)
                    == Some(&package.version)
                && versions[0].get("checksum").and_then(toml::Value::as_str)
                    == Some(&package.crate_sha256),
            "Grammar notice version or checksum does not match Cargo.lock: {}",
            package.name
        );
        ensure!(
            package.repository.starts_with("https://github.com/")
                && package.revision.len() == 40
                && package.revision.bytes().all(|b| b.is_ascii_hexdigit()),
            "Invalid grammar notice provenance: {}",
            package.name
        );
        for notice in &package.notices {
            ensure!(
                notice.source.starts_with("https://"),
                "Invalid notice source"
            );
            if let Some(path) = &notice.archive_path {
                ensure!(
                    path == &format!("{}-{}/{}", package.name, package.version, notice.file),
                    "Invalid notice archive path"
                );
            }
            let path = directory.join(&package.name).join(&notice.file);
            let bytes = fs::read(&path)
                .with_context(|| format!("Missing grammar notice: {}", path.display()))?;
            ensure!(
                util::sha256_hex(&bytes) == notice.sha256,
                "Grammar notice checksum mismatch: {}",
                path.display()
            );
        }
    }
    let actual_files: BTreeSet<_> = walkdir::WalkDir::new(&directory)
        .into_iter()
        .map(|entry| {
            let entry = entry?;
            ensure!(
                !entry.file_type().is_symlink(),
                "Symlink in grammar notices"
            );
            Ok(entry.file_type().is_file().then(|| {
                entry
                    .path()
                    .strip_prefix(&directory)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            }))
        })
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect();
    ensure!(
        actual_files == expected_files,
        "Grammar notice file inventory mismatch"
    );
    Ok(())
}

pub(super) fn validate(root: &Path) -> Result<()> {
    ensure!(
        fs::read(root.join("resources/licenses/grammars/SOURCES.json"))? == SOURCES.as_bytes(),
        "Grammar notice inventory changed; rebuild xtask"
    );
    ensure!(
        fs::read(root.join("resources/licenses/icons/FILES.json"))? == ICON_FILES.as_bytes(),
        "Icon notice inventory changed; rebuild xtask"
    );
    validate_grammars(root, &inventory()?)
}

pub(super) fn stage_icons(root: &Path, resources: &Path) -> Result<()> {
    let files = icon_files()?;
    let assets = root.join("crates/newport-gpui/assets");
    for path in [
        assets.clone(),
        assets.join("file-icons"),
        assets.join("process-icons"),
    ] {
        ensure!(
            fs::symlink_metadata(&path)?.is_dir(),
            "Expected an icon notice directory: {}",
            path.display()
        );
    }
    for name in &files {
        let source = assets.join(name);
        ensure!(
            fs::symlink_metadata(&source)?.is_file(),
            "Expected a regular icon notice: {}",
            source.display()
        );
    }
    for name in files {
        let destination = resources.join("Licenses/icons").join(&name);
        fs::create_dir_all(destination.parent().unwrap())?;
        copy_file(&assets.join(&name), &destination)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{package::copy_dir, root};

    fn fixture() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path();
        fs::create_dir_all(path.join("crates/newport-grammars")).unwrap();
        copy_file(&root().join("Cargo.lock"), &path.join("Cargo.lock")).unwrap();
        copy_file(
            &root().join("crates/newport-grammars/Cargo.toml"),
            &path.join("crates/newport-grammars/Cargo.toml"),
        )
        .unwrap();
        copy_dir(
            &root().join("resources/licenses/grammars"),
            &path.join("resources/licenses/grammars"),
        )
        .unwrap();
        copy_dir(
            &root().join("resources/licenses/icons"),
            &path.join("resources/licenses/icons"),
        )
        .unwrap();
        directory
    }

    #[test]
    fn pinned_grammars_and_packaged_icon_notices_are_complete() {
        validate(&root()).unwrap();
        let assets = root().join("crates/newport-gpui/assets");
        let actual: BTreeSet<_> = ["file-icons", "process-icons"]
            .into_iter()
            .flat_map(|dir| fs::read_dir(assets.join(dir)).unwrap())
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_none_or(|ext| ext != "svg"))
            .map(|path| {
                path.strip_prefix(&assets)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(actual, icon_files().unwrap().into_iter().collect());
    }

    #[test]
    fn grammar_notices_reject_missing_altered_and_unlisted_files() {
        let directory = fixture();
        let file = directory
            .path()
            .join("resources/licenses/grammars/tree-sitter-rust/LICENSE");
        let original = fs::read(&file).unwrap();
        fs::remove_file(&file).unwrap();
        assert!(
            validate(directory.path())
                .unwrap_err()
                .to_string()
                .contains("Missing grammar notice")
        );
        fs::write(&file, b"Changed notice").unwrap();
        assert!(
            validate(directory.path())
                .unwrap_err()
                .to_string()
                .contains("checksum mismatch")
        );
        fs::write(&file, original).unwrap();
        fs::write(file.with_file_name("unexpected.txt"), b"Not inventoried").unwrap();
        assert!(
            validate(directory.path())
                .unwrap_err()
                .to_string()
                .contains("file inventory mismatch")
        );
    }

    #[test]
    fn grammar_notices_reject_dependency_and_lockfile_drift() {
        let mut changed = inventory().unwrap();
        changed.packages.pop();
        assert!(
            validate_grammars(&root(), &changed)
                .unwrap_err()
                .to_string()
                .contains("dependencies")
        );
        let mut changed = inventory().unwrap();
        changed
            .packages
            .iter_mut()
            .find(|p| p.name == "tree-sitter-rust")
            .unwrap()
            .version = "0.24.1".into();
        assert!(
            validate_grammars(&root(), &changed)
                .unwrap_err()
                .to_string()
                .contains("dependency")
        );
        let mut changed = inventory().unwrap();
        changed.packages[0].crate_sha256 = "0".repeat(64);
        assert!(
            validate_grammars(&root(), &changed)
                .unwrap_err()
                .to_string()
                .contains("Cargo.lock")
        );
    }

    #[cfg(unix)]
    #[test]
    fn icon_notices_reject_source_file_and_directory_symlinks_before_staging() {
        use std::os::unix::fs::symlink;

        for linked in ["file-icons/LICENSE-symbols", "file-icons", "process-icons"] {
            let directory = tempfile::tempdir().unwrap();
            let fixture = directory.path().join("source");
            let assets = fixture.join("crates/newport-gpui/assets");
            for name in icon_files().unwrap() {
                let destination = assets.join(&name);
                fs::create_dir_all(destination.parent().unwrap()).unwrap();
                copy_file(
                    &root().join("crates/newport-gpui/assets").join(&name),
                    &destination,
                )
                .unwrap();
            }
            stage_icons(&fixture, &directory.path().join("control")).unwrap();

            let source = assets.join(linked);
            let target = directory.path().join("outside-assets");
            fs::rename(&source, &target).unwrap();
            symlink(&target, &source).unwrap();
            let staged = directory.path().join("stage");
            assert!(stage_icons(&fixture, &staged).is_err(), "{linked}");
            assert!(!staged.exists(), "Copied notices before rejecting {linked}");
        }
    }
}
