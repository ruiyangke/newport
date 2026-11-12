//! Dependencies Newport patches. The repository keeps only each crate's
//! patch and the crates.io release it applies to; the patched source is
//! generated, never committed.
//!
//! `patches/patched.toml` pins each release by version and SHA-256, the patch
//! is `patches/<name>.patch`, and the source is generated into
//! `patches/<name>/`, which Git ignores. Newport's root has a set, and so does
//! `remote/` when it is checked out.
use crate::util;
use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

/// Holds the pins, the patches and the generated crates.
const DIRECTORY: &str = "patches";
const MANIFEST: &str = "patches/patched.toml";
/// Written last into a generated crate: its release, checksum and patch.
const STAMP: &str = ".newport-patched";
const DOWNLOAD_LIMIT: u64 = 64 * 1024 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pin {
    version: String,
    sha256: String,
}

fn manifest(root: &Path) -> Result<BTreeMap<String, Pin>> {
    let path = root.join(MANIFEST);
    toml::from_str(&fs::read_to_string(&path)?)
        .with_context(|| format!("Invalid {}", path.display()))
}

fn patch_path(root: &Path, name: &str) -> PathBuf {
    root.join(DIRECTORY).join(format!("{name}.patch"))
}

/// The roots with patched crates: Newport's, and `remote/` when present.
fn roots() -> Vec<PathBuf> {
    let root = util::root();
    let remote = root.join("remote");
    let mut roots = vec![root];
    if remote.join(MANIFEST).is_file() {
        roots.push(remote);
    }
    roots
}

/// Generates every patched crate that is missing or out of date.
pub fn ensure_all() -> Result<()> {
    for root in roots() {
        ensure_in(&root)?;
    }
    Ok(())
}

/// Generates `root`'s patched crates that are missing or out of date.
pub fn ensure_in(root: &Path) -> Result<()> {
    if !root.join(MANIFEST).is_file() {
        return Ok(());
    }
    for (name, pin) in manifest(root)? {
        let patch = fs::read(patch_path(root, &name))?;
        let directory = root.join(DIRECTORY).join(&name);
        let stamp = stamp(&name, &pin, &patch);
        if fs::read_to_string(directory.join(STAMP)).is_ok_and(|current| current == stamp) {
            continue;
        }
        let source = release(&name, &pin)?;
        generate(
            &directory,
            &name,
            &pin,
            &source,
            &patch_path(root, &name),
            &stamp,
        )?;
        println!(
            "Generated patches/{name}: {name} {} with its patch",
            pin.version
        );
    }
    Ok(())
}

/// Writes `name`'s patch from the changes made to its generated source.
pub fn save(name: &str) -> Result<()> {
    let Some(root) = roots()
        .into_iter()
        .find(|root| manifest(root).is_ok_and(|pins| pins.contains_key(name)))
    else {
        bail!("No patched crate named {name}");
    };
    let pins = manifest(&root)?;
    let pin = &pins[name];
    let directory = root.join(DIRECTORY).join(name);
    ensure!(
        directory.join(STAMP).is_file(),
        "patches/{name} isn't generated; run `cargo xtask patched` first"
    );
    let current = fs::read_to_string(patch_path(&root, name)).unwrap_or_default();
    let mut patch = description(&current).as_bytes().to_vec();
    patch.extend(diff(&release(name, pin)?, name, pin, &directory)?);
    fs::write(patch_path(&root, name), &patch)?;
    fs::write(directory.join(STAMP), stamp(name, pin, &patch))?;
    println!("Saved patches/{name}.patch");
    Ok(())
}

/// The text before a patch's first file: what it changes and why.
/// `git apply` ignores it.
fn description(patch: &str) -> &str {
    let mut offset = 0;
    for line in patch.split_inclusive('\n') {
        if line.starts_with("diff --git ") || line.starts_with("--- ") {
            break;
        }
        offset += line.len();
    }
    &patch[..offset]
}

fn stamp(name: &str, pin: &Pin, patch: &[u8]) -> String {
    format!(
        "{name} {} {} {}\n",
        pin.version,
        pin.sha256,
        util::sha256_hex(patch)
    )
}

/// The pinned `.crate` archive, from `target/patched` or crates.io.
fn release(name: &str, pin: &Pin) -> Result<Vec<u8>> {
    let cache = util::root()
        .join("target/patched")
        .join(format!("{name}-{}.crate", pin.version));
    if let Some(bytes) = fs::read(&cache)
        .ok()
        .filter(|bytes| util::sha256_hex(bytes) == pin.sha256)
    {
        return Ok(bytes);
    }
    let url = format!(
        "https://static.crates.io/crates/{name}/{name}-{}.crate",
        pin.version
    );
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(120)))
        .build()
        .into();
    let mut response = match agent.get(&url).call() {
        Err(ureq::Error::StatusCode(status)) => bail!("Downloading {name} failed: {status}"),
        result => result.with_context(|| format!("Downloading {name} failed"))?,
    };
    let bytes = response
        .body_mut()
        .with_config()
        .limit(DOWNLOAD_LIMIT)
        .read_to_vec()
        .with_context(|| format!("Downloading {name} failed"))?;
    verify(name, pin, &bytes)?;
    if let Some(parent) = cache.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&cache, &bytes)?;
    Ok(bytes)
}

fn verify(name: &str, pin: &Pin, bytes: &[u8]) -> Result<()> {
    ensure!(
        util::sha256_hex(bytes) == pin.sha256,
        "{name} {} doesn't match the SHA-256 in {MANIFEST}",
        pin.version
    );
    Ok(())
}

/// Unpacks the release under `parent`, outside any Git repository so that
/// `git apply` works as a plain patch tool there.
fn unpack(source: &[u8], name: &str, pin: &Pin, parent: &Path) -> Result<PathBuf> {
    verify(name, pin, source)?;
    tar::Archive::new(flate2::read::GzDecoder::new(source))
        .unpack(parent)
        .with_context(|| format!("Invalid {name} archive"))?;
    let directory = parent.join(format!("{name}-{}", pin.version));
    ensure!(
        directory.is_dir(),
        "The {name} archive has no {name}-{} folder",
        pin.version
    );
    Ok(directory)
}

fn generate(
    directory: &Path,
    name: &str,
    pin: &Pin,
    source: &[u8],
    patch: &Path,
    stamp: &str,
) -> Result<()> {
    let work = tempfile::tempdir()?;
    let patched = unpack(source, name, pin, work.path())?;
    // `git apply` refuses an empty patch, which `--save` writes once every
    // edit is undone.
    let empty = fs::read(patch)?.iter().all(u8::is_ascii_whitespace);
    if !empty {
        apply(name, pin, patch, &patched)?;
    }
    if directory.exists() {
        fs::remove_dir_all(directory)?;
    }
    copy_tree(&patched, directory)?;
    // Last, so that an interrupted copy is generated again.
    fs::write(directory.join(STAMP), stamp)?;
    Ok(())
}

fn apply(name: &str, pin: &Pin, patch: &Path, patched: &Path) -> Result<()> {
    util::run(
        util::command("git")
            .current_dir(patched)
            .args(["apply", "--whitespace=nowarn"])
            // Not `canonicalize`: on Windows it returns a `\\?\` path, which Git can't open.
            .arg(std::path::absolute(patch)?),
    )
    .with_context(|| {
        format!(
            "patches/{name}.patch doesn't apply to {name} {}",
            pin.version
        )
    })
}

/// The patch that turns the release into `directory`.
fn diff(source: &[u8], name: &str, pin: &Pin, directory: &Path) -> Result<Vec<u8>> {
    let work = tempfile::tempdir()?;
    fs::rename(
        unpack(source, name, pin, work.path())?,
        work.path().join("a"),
    )?;
    copy_tree(directory, &work.path().join("b"))?;
    fs::remove_file(work.path().join("b").join(STAMP))?;
    let output = util::command("git")
        .current_dir(work.path())
        .args(["diff", "--no-index", "--no-prefix", "--", "a", "b"])
        .stdin(Stdio::null())
        .output()
        .context("Cannot start git")?;
    // `git diff --no-index` exits 1 when the trees differ.
    ensure!(
        matches!(output.status.code(), Some(0 | 1)),
        "git diff failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(output.stdout)
}

/// Copies regular files and folders; Cargo's build output is left out.
fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    for entry in walkdir::WalkDir::new(from)
        .into_iter()
        .filter_entry(|entry| entry.depth() != 1 || entry.file_name() != "target")
    {
        let entry = entry?;
        let destination = to.join(entry.path().strip_prefix(from)?);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&destination)?;
        } else if entry.file_type().is_file() {
            fs::copy(entry.path(), &destination)?;
        } else {
            bail!("Patched crates hold only files: {}", entry.path().display());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn archive(files: &[(&str, &str)]) -> Vec<u8> {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        for (path, text) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(text.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, format!("demo-1.0.0/{path}"), text.as_bytes())
                .unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    fn pin(source: &[u8]) -> Pin {
        Pin {
            version: "1.0.0".into(),
            sha256: util::sha256_hex(source),
        }
    }

    #[test]
    fn a_saved_patch_regenerates_the_edited_crate() {
        let source = archive(&[("src/lib.rs", "pub fn one() {}\n"), ("README.md", "demo\n")]);
        let pin = pin(&source);
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("demo");
        let patch = root.path().join("demo.patch");
        fs::write(&patch, "").unwrap();
        // An empty patch generates the release as it is.
        generate(&directory, "demo", &pin, &source, &patch, "first\n").unwrap();
        assert_eq!(
            fs::read_to_string(directory.join("src/lib.rs")).unwrap(),
            "pub fn one() {}\n"
        );
        assert_eq!(
            fs::read_to_string(directory.join(STAMP)).unwrap(),
            "first\n"
        );

        fs::write(directory.join("src/lib.rs"), "pub fn two() {}\n").unwrap();
        fs::write(directory.join("src/new.rs"), "// added\n").unwrap();
        fs::remove_file(directory.join("README.md")).unwrap();
        let saved = diff(&source, "demo", &pin, &directory).unwrap();
        fs::File::create(&patch).unwrap().write_all(&saved).unwrap();

        fs::remove_dir_all(&directory).unwrap();
        generate(&directory, "demo", &pin, &source, &patch, "second\n").unwrap();
        assert_eq!(
            fs::read_to_string(directory.join("src/lib.rs")).unwrap(),
            "pub fn two() {}\n"
        );
        assert_eq!(
            fs::read_to_string(directory.join("src/new.rs")).unwrap(),
            "// added\n"
        );
        assert!(!directory.join("README.md").exists());
        assert_eq!(
            fs::read_to_string(directory.join(STAMP)).unwrap(),
            "second\n"
        );
    }

    #[test]
    fn a_release_with_another_checksum_is_refused() {
        let source = archive(&[("src/lib.rs", "pub fn one() {}\n")]);
        let mut pin = pin(&source);
        pin.sha256 = "0".repeat(64);
        let error = unpack(&source, "demo", &pin, tempfile::tempdir().unwrap().path())
            .unwrap_err()
            .to_string();
        assert!(error.contains("doesn't match the SHA-256"), "{error}");
    }

    #[test]
    fn a_patch_that_does_not_apply_leaves_no_generated_crate() {
        let source = archive(&[("src/lib.rs", "pub fn one() {}\n")]);
        let pin = pin(&source);
        let root = tempfile::tempdir().unwrap();
        let patch = root.path().join("demo.patch");
        fs::write(
            &patch,
            "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-pub fn missing() {}\n+pub fn two() {}\n",
        )
        .unwrap();
        let directory = root.path().join("demo");
        assert!(generate(&directory, "demo", &pin, &source, &patch, "stamp\n").is_err());
        assert!(!directory.exists());
    }

    #[test]
    fn a_patch_keeps_its_description_and_git_ignores_it() {
        let patch = "# Why\n\nIt retries.\n\n--- a/src/lib.rs\n+++ b/src/lib.rs\n";
        assert_eq!(description(patch), "# Why\n\nIt retries.\n\n");
        assert_eq!(description("diff --git a/x b/x\n"), "");

        let source = archive(&[("src/lib.rs", "pub fn one() {}\n")]);
        let pin = pin(&source);
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("demo.patch");
        fs::write(
            &path,
            "# Renames one to two.\n\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-pub fn one() {}\n+pub fn two() {}\n",
        )
        .unwrap();
        let directory = root.path().join("demo");
        generate(&directory, "demo", &pin, &source, &path, "stamp\n").unwrap();
        assert_eq!(
            fs::read_to_string(directory.join("src/lib.rs")).unwrap(),
            "pub fn two() {}\n"
        );
    }

    #[test]
    fn the_stamp_follows_the_patch() {
        let pin = Pin {
            version: "1.0.0".into(),
            sha256: "a".repeat(64),
        };
        assert_ne!(stamp("demo", &pin, b"one"), stamp("demo", &pin, b"two"));
        assert!(stamp("demo", &pin, b"one").starts_with("demo 1.0.0 "));
    }
}
