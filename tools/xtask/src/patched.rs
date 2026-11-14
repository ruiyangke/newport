//! Dependencies Newport patches. The repository keeps only the crates.io
//! release each crate comes from and the patches that change it; the patched
//! source is generated, never committed.
//!
//! `patches/patched.toml` pins each release by version and SHA-256. A crate's
//! patches are `patches/<crate>/NNN-<subject>.patch`, numbered 001 to 999 and
//! applied in that order, each with paths in the crate (`a/src/lib.rs`) and
//! opening with what it changes, why, and when it can be removed.
//! `patches/<crate>/NOTICE` states the changes for the third-party notices.
//! The crate is generated into `vendor/<crate>/`, which Git ignores.
//! Newport's root has a set, and so does `remote/` when it is checked out.
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

/// Holds the pins and a folder of patches for each crate.
const DIRECTORY: &str = "patches";
const MANIFEST: &str = "patches/patched.toml";
/// Holds the generated crates, one folder each.
const GENERATED: &str = "vendor";
/// Written last into a generated crate: its release, checksum and patches.
const STAMP: &str = ".newport-patched";
const DOWNLOAD_LIMIT: u64 = 64 * 1024 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pin {
    version: String,
    sha256: String,
}

/// One of a crate's patches: its number, file name and text.
#[derive(Debug)]
struct Patch {
    number: u16,
    name: String,
    text: String,
}

/// The number a patch's file name starts with: `NNN-<subject>.patch`, 001 to 999.
fn number(name: &str) -> Option<u16> {
    let (digits, subject) = name.strip_suffix(".patch")?.split_at_checked(3)?;
    let subject = subject.strip_prefix('-')?;
    if subject.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok().filter(|&number| number != 0)
}

/// `krate`'s patches in `folder`, in order.
fn patches(folder: &Path, krate: &str) -> Result<Vec<Patch>> {
    let entries = match fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut patches = Vec::new();
    for entry in entries {
        let name = entry?.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".patch") {
            continue;
        }
        let Some(number) = number(&name) else {
            bail!("{krate}/{name}: a patch is named NNN-<subject>.patch, numbered 001 to 999");
        };
        if let Some(other) = patches.iter().find(|patch: &&Patch| patch.number == number) {
            bail!(
                "{krate}/{name} and {krate}/{} share number {number:03}",
                other.name
            );
        }
        let text = fs::read_to_string(folder.join(&name))?;
        patches.push(Patch { number, name, text });
    }
    patches.sort_by_key(|patch| patch.number);
    Ok(patches)
}

fn pins(root: &Path) -> Result<BTreeMap<String, Pin>> {
    let path = root.join(MANIFEST);
    toml::from_str(&fs::read_to_string(&path)?)
        .with_context(|| format!("Invalid {}", path.display()))
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
    for (name, pin) in &pins(root)? {
        let folder = root.join(DIRECTORY).join(name);
        let patches = patches(&folder, name)?;
        let directory = root.join(GENERATED).join(name);
        let stamp = stamp(name, pin, &patches);
        if fs::read_to_string(directory.join(STAMP)).is_ok_and(|current| current == stamp) {
            continue;
        }
        let source = release(name, pin)?;
        generate(&directory, name, pin, &source, &patches, &stamp)?;
        println!(
            "Generated vendor/{name}: {name} {} with {} patch{}",
            pin.version,
            patches.len(),
            if patches.len() == 1 { "" } else { "es" }
        );
    }
    Ok(())
}

/// Writes `path`, a crate's patch, from the edits made to the generated
/// crate: its difference from the release with the patches numbered below
/// it applied. The patch is the crate's last, a new one or the last one
/// again, whose description is kept.
pub fn save(path: &str) -> Result<()> {
    let path = util::root().join(path);
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let folder = path.parent().context("Name a patch file")?;
    let krate = folder
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let root = folder
        .parent()
        .filter(|parent| parent.ends_with(DIRECTORY))
        .and_then(Path::parent)
        .context("Name a patch as patches/<crate>/NNN-<subject>.patch")?;
    let number = number(&name).with_context(|| {
        format!("{name}: a patch is named NNN-<subject>.patch, numbered 001 to 999")
    })?;
    let pins = pins(root)?;
    let pin = pins
        .get(&krate)
        .with_context(|| format!("{MANIFEST} doesn't pin {krate}"))?;
    let directory = root.join(GENERATED).join(&krate);
    ensure!(
        directory.join(STAMP).is_file(),
        "Generate {krate} with `cargo xtask patched` and edit it first"
    );
    let mut patches = patches(folder, &krate)?;
    if let Some(later) = patches
        .iter()
        .find(|patch| patch.number > number || (patch.number == number && patch.name != name))
    {
        bail!(
            "{krate}/{} comes after or shares {name}'s number; save the edits as the crate's last patch",
            later.name
        );
    }
    let description = match patches.last() {
        Some(last) if last.name == name => description(&patches.pop().unwrap().text).to_owned(),
        _ => String::new(),
    };
    let base = tempfile::tempdir()?;
    let base_crate = base.path().join(&krate);
    generate(
        &base_crate,
        &krate,
        pin,
        &release(&krate, pin)?,
        &patches,
        "",
    )?;
    let change = String::from_utf8(diff(&base_crate, &directory)?).context("Patches are text")?;
    ensure!(!change.is_empty(), "{krate} has no edits to save");
    let text = description + &change;
    fs::write(&path, &text)?;
    patches.push(Patch {
        number,
        name: name.clone(),
        text,
    });
    // The generated crate is what its patches now make.
    fs::write(directory.join(STAMP), stamp(&krate, pin, &patches))?;
    println!("Saved {krate}/{name}");
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

fn stamp(name: &str, pin: &Pin, patches: &[Patch]) -> String {
    let mut hashed = Vec::new();
    for patch in patches {
        hashed.extend_from_slice(patch.name.as_bytes());
        hashed.push(0);
        hashed.extend_from_slice(patch.text.as_bytes());
        hashed.push(0);
    }
    format!(
        "{name} {} {} {}\n",
        pin.version,
        pin.sha256,
        util::sha256_hex(&hashed)
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

/// Generates `directory` from the release and `patches`.
fn generate(
    directory: &Path,
    krate: &str,
    pin: &Pin,
    source: &[u8],
    patches: &[Patch],
    stamp: &str,
) -> Result<()> {
    let work = tempfile::tempdir()?;
    let patched = unpack(source, krate, pin, work.path())?;
    for patch in patches {
        let file = work.path().join(&patch.name);
        fs::write(&file, &patch.text)?;
        apply(krate, pin, &patch.name, &file, &patched)?;
    }
    if directory.exists() {
        fs::remove_dir_all(directory)?;
    }
    copy_tree(&patched, directory)?;
    // Last, so that an interrupted copy is generated again.
    if !stamp.is_empty() {
        fs::write(directory.join(STAMP), stamp)?;
    }
    Ok(())
}

fn apply(krate: &str, pin: &Pin, name: &str, file: &Path, patched: &Path) -> Result<()> {
    util::run(
        util::command("git")
            .current_dir(patched)
            .args(["apply", "--whitespace=nowarn"])
            // Not `canonicalize`: on Windows it returns a `\\?\` path, which Git can't open.
            .arg(std::path::absolute(file)?),
    )
    .with_context(|| format!("{krate}/{name} doesn't apply to {krate} {}", pin.version))
}

/// The patch that turns `base` into `directory`, with paths in the crate.
fn diff(base: &Path, directory: &Path) -> Result<Vec<u8>> {
    let work = tempfile::tempdir()?;
    copy_tree(base, &work.path().join("a"))?;
    copy_tree(directory, &work.path().join("b"))?;
    for side in ["a", "b"] {
        let stamp = work.path().join(side).join(STAMP);
        if stamp.exists() {
            fs::remove_file(stamp)?;
        }
    }
    let output = util::command("git")
        .current_dir(work.path())
        .args([
            "diff",
            "--no-index",
            "--binary",
            "--no-prefix",
            "--",
            "a",
            "b",
        ])
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

    fn patch(name: &str, text: &str) -> Patch {
        Patch {
            number: number(name).unwrap(),
            name: name.into(),
            text: text.into(),
        }
    }

    fn rename(from: &str, to: &str) -> String {
        format!(
            "# Renames {from} to {to}.\n\n\
             diff --git a/src/lib.rs b/src/lib.rs\n\
             --- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-pub fn {from}() {{}}\n+pub fn {to}() {{}}\n"
        )
    }

    #[test]
    fn patches_apply_in_their_numbers_order() {
        let source = archive(&[("src/lib.rs", "pub fn one() {}\n")]);
        let folder = tempfile::tempdir().unwrap();
        // Written out of order; 010 sorts after 002 by number.
        fs::write(
            folder.path().join("010-three.patch"),
            rename("two", "three"),
        )
        .unwrap();
        fs::write(folder.path().join("002-two.patch"), rename("one", "two")).unwrap();
        fs::write(folder.path().join("NOTICE"), "notice\n").unwrap();
        let patches = patches(folder.path(), "demo").unwrap();
        assert_eq!(
            patches.iter().map(|patch| patch.number).collect::<Vec<_>>(),
            [2, 10]
        );
        let directory = folder.path().join(GENERATED).join("demo");
        generate(
            &directory,
            "demo",
            &pin(&source),
            &source,
            &patches,
            "stamp\n",
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(directory.join("src/lib.rs")).unwrap(),
            "pub fn three() {}\n"
        );
        assert_eq!(
            fs::read_to_string(directory.join(STAMP)).unwrap(),
            "stamp\n"
        );
    }

    #[test]
    fn patch_names_are_numbered_from_001_to_999() {
        assert_eq!(number("001-first.patch"), Some(1));
        assert_eq!(number("999-last.patch"), Some(999));
        for name in [
            "000-zero.patch",
            "1-short.patch",
            "0001-long.patch",
            "001.patch",
            "001-.patch",
            "abc-x.patch",
        ] {
            assert_eq!(number(name), None, "{name}");
        }
        let folder = tempfile::tempdir().unwrap();
        fs::write(folder.path().join("fix.patch"), "").unwrap();
        let error = patches(folder.path(), "demo").unwrap_err().to_string();
        assert!(error.contains("NNN-<subject>.patch"), "{error}");
    }

    #[test]
    fn two_patches_may_not_share_a_number() {
        let folder = tempfile::tempdir().unwrap();
        fs::write(folder.path().join("001-one.patch"), "").unwrap();
        fs::write(folder.path().join("001-other.patch"), "").unwrap();
        let error = patches(folder.path(), "demo").unwrap_err().to_string();
        assert!(error.contains("share number 001"), "{error}");
    }

    #[test]
    fn edits_diff_against_the_earlier_patches() {
        let source = archive(&[("src/lib.rs", "pub fn one() {}\n"), ("README.md", "demo\n")]);
        let pin = pin(&source);
        let root = tempfile::tempdir().unwrap();
        let first = vec![patch("001-two.patch", &rename("one", "two"))];
        let directory = root.path().join("edited");
        generate(&directory, "demo", &pin, &source, &first, "stamp\n").unwrap();
        fs::write(directory.join("src/lib.rs"), "pub fn four() {}\n").unwrap();
        fs::write(directory.join("src/new.rs"), "// added\n").unwrap();
        fs::remove_file(directory.join("README.md")).unwrap();

        let base = root.path().join("base");
        generate(&base, "demo", &pin, &source, &first, "").unwrap();
        let saved = String::from_utf8(diff(&base, &directory).unwrap()).unwrap();
        assert!(saved.starts_with("diff --git a/"), "{saved}");
        assert!(saved.contains("-pub fn two() {}"), "{saved}");

        let both = vec![
            first.into_iter().next().unwrap(),
            patch("002-four.patch", &saved),
        ];
        let regenerated = root.path().join("regenerated");
        generate(&regenerated, "demo", &pin, &source, &both, "stamp\n").unwrap();
        assert_eq!(
            fs::read_to_string(regenerated.join("src/lib.rs")).unwrap(),
            "pub fn four() {}\n"
        );
        assert_eq!(
            fs::read_to_string(regenerated.join("src/new.rs")).unwrap(),
            "// added\n"
        );
        assert!(!regenerated.join("README.md").exists());
    }

    #[test]
    fn a_description_ends_at_the_first_file() {
        assert_eq!(
            description(&rename("one", "two")),
            "# Renames one to two.\n\n"
        );
        assert_eq!(description("--- a/x\n"), "");
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
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join(GENERATED).join("demo");
        let patches = vec![patch("001-missing.patch", &rename("missing", "two"))];
        let error = generate(
            &directory,
            "demo",
            &pin(&source),
            &source,
            &patches,
            "stamp\n",
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("demo/001-missing.patch doesn't apply to demo 1.0.0"),
            "{error}"
        );
        assert!(!directory.exists());
    }

    #[test]
    fn the_stamp_follows_the_crate_s_patches() {
        let pin = Pin {
            version: "1.0.0".into(),
            sha256: "a".repeat(64),
        };
        let one = [patch("001-a.patch", "one")];
        assert_ne!(
            stamp("demo", &pin, &one),
            stamp("demo", &pin, &[patch("001-a.patch", "two")])
        );
        assert_ne!(
            stamp("demo", &pin, &one),
            stamp("demo", &pin, &[patch("002-a.patch", "one")])
        );
        assert!(stamp("demo", &pin, &[]).starts_with("demo 1.0.0 "));
    }
}
