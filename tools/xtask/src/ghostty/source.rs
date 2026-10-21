//! The pinned Ghostty checkout and its patched, immutable build copies. Each
//! revision and patch set gets its own directory, and the user's source
//! checkout is never edited.
use super::BuildEnv;
use crate::{
    command,
    util::{read_json, run, sha256_file, sha256_hex},
};
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize, Serializer};
use std::{
    collections::{BTreeMap, HashMap},
    ffi::OsStr,
    fs,
    path::{Component, Path, PathBuf},
    process::Stdio,
};

const MANIFEST: &str = ".newport-ghostty-source.json";
const PREPARATION_VERSION: u32 = 4;
const UPSTREAM: &str = "https://github.com/ghostty-org/ghostty.git";

#[derive(Serialize)]
struct Patch {
    name: String,
    hash: String,
}

#[derive(Serialize)]
struct Fingerprint<'a> {
    #[serde(rename = "preparationVersion")]
    preparation_version: u32,
    revision: &'a str,
    #[serde(rename = "patchSet")]
    patch_set: &'a [Patch],
}

#[derive(Serialize)]
struct NewManifest<'a> {
    revision: &'a str,
    fingerprint: &'a str,
    source: &'a str,
    #[serde(rename = "patchSet")]
    patch_set: &'a [Patch],
    #[serde(rename = "modifiedFiles", serialize_with = "ordered")]
    modified_files: &'a [(String, String)],
}

fn ordered<S: Serializer>(entries: &&[(String, String)], s: S) -> Result<S::Ok, S::Error> {
    s.collect_map(entries.iter().map(|(file, hash)| (file, hash)))
}

#[derive(Deserialize)]
struct Manifest {
    fingerprint: String,
    source: PathBuf,
    #[serde(rename = "modifiedFiles")]
    modified_files: BTreeMap<String, String>,
}

/// Run git with the build environment; a failure carries git's stderr.
fn git<S: AsRef<OsStr>>(env: &BuildEnv, args: &[S]) -> Result<String> {
    let output = env
        .apply(&mut command("git"))
        .args(args)
        .stdin(Stdio::null())
        .output()
        .context("Cannot start git")?;
    ensure!(
        output.status.success(),
        "git failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Shallow-fetch `revision` from upstream into `destination`.
pub fn fetch(destination: &Path, revision: &str, cache: &Path, env: &BuildEnv) -> Result<()> {
    eprintln!("Fetching Ghostty {revision}");
    let staging = tempfile::Builder::new()
        .prefix("source-download-")
        .tempdir_in(cache)
        .context("Cannot create the Ghostty download directory")?;
    let path = staging.path().as_os_str();
    run(env
        .apply(&mut command("git"))
        .args(["init".as_ref(), "--quiet".as_ref(), path]))?;
    run(env
        .apply(&mut command("git"))
        .arg("-C")
        .arg(path)
        .args(["fetch", "--depth", "1", UPSTREAM, revision]))?;
    run(env
        .apply(&mut command("git"))
        .arg("-C")
        .arg(path)
        .args(["checkout", "--quiet", "--detach", revision]))?;
    fs::rename(staging.path(), destination)
        .with_context(|| format!("Cannot store {}", destination.display()))
}

/// Patch text with Windows line endings canonicalized: git archive emits LF
/// sources even when the parent checkout uses CRLF.
fn canonical(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).replace("\r\n", "\n")
}

/// Every `+++ b/<path>` target in a patch, in order.
fn targets(patch: &str) -> Vec<&str> {
    patch
        .split(['\n', '\r', '\u{2028}', '\u{2029}'])
        .filter_map(|line| line.strip_prefix("+++ b/"))
        .filter(|file| !file.is_empty())
        .collect()
}

fn valid_target(file: &str) -> bool {
    !file.contains('\\')
        && !file.split('/').any(|part| part == "..")
        && Path::new(file)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}

fn validate(destination: &Path, fingerprint: &str) -> Result<()> {
    let manifest: Manifest = read_json(&destination.join(MANIFEST))?;
    ensure!(
        manifest.fingerprint == fingerprint,
        "Ghostty source cache fingerprint mismatch"
    );
    for (file, hash) in &manifest.modified_files {
        if sha256_file(&destination.join(file))? != *hash {
            bail!("Prepared Ghostty source was modified: {file}");
        }
    }
    Ok(())
}

/// The patched copy of `source` at `revision` with every `*.patch` in `patches`
/// applied, under `cache/patched-<revision>-<fingerprint>`. An existing copy is
/// revalidated against its manifest. `source` may itself be a prepared copy
/// (nested build wrappers pass one on); its pinned base is used instead.
pub fn prepare(
    source: &Path,
    revision: &str,
    patches: &Path,
    cache: &Path,
    env: &BuildEnv,
) -> Result<PathBuf> {
    let mut files = Vec::new();
    for entry in
        fs::read_dir(patches).with_context(|| format!("Cannot read {}", patches.display()))?
    {
        let name = entry?
            .file_name()
            .into_string()
            .ok()
            .context("Non-UTF-8 patch name")?;
        if name.ends_with(".patch") {
            files.push(name);
        }
    }
    files.sort();
    let mut patch_set = Vec::new();
    let mut texts = Vec::new();
    for name in &files {
        let text = canonical(&fs::read(patches.join(name))?);
        patch_set.push(Patch {
            name: name.clone(),
            hash: sha256_hex(text.as_bytes()),
        });
        texts.push(text);
    }
    let fingerprint = sha256_hex(
        serde_json::to_string(&Fingerprint {
            preparation_version: PREPARATION_VERSION,
            revision,
            patch_set: &patch_set,
        })?
        .as_bytes(),
    );
    let destination = cache.join(format!("patched-{revision}-{}", &fingerprint[..16]));
    let mut source = source.to_path_buf();
    if source.join(MANIFEST).exists() {
        source = read_json::<Manifest>(&source.join(MANIFEST))?.source;
    }
    if destination.exists() {
        validate(&destination, &fingerprint)?;
        return Ok(destination);
    }
    ensure!(
        git(
            env,
            &[
                "-C".as_ref(),
                source.as_os_str(),
                "rev-parse".as_ref(),
                "HEAD".as_ref()
            ]
        )? == revision,
        "Ghostty source revision does not match the Rust bindings"
    );
    eprintln!("Applying {} Ghostty patches", files.len());
    fs::create_dir_all(cache)?;
    let temporary = tempfile::Builder::new()
        .prefix("patch-source-")
        .tempdir_in(cache)
        .context("Cannot create the Ghostty patch directory")?;
    let archive = temporary.path().join("source.tar");
    let staged = temporary.path().join("source");
    fs::create_dir(&staged)?;
    // The build cache is nested inside Newport's Git checkout. Stop repository
    // discovery here or git apply silently skips paths outside that prefix.
    let mut patch_env = env.clone();
    patch_env.set("GIT_CEILING_DIRECTORIES", temporary.path());
    // Archive the pinned tree; local edits and untracked files are never copied.
    let mut output = std::ffi::OsString::from("--output=");
    output.push(&archive);
    git(
        env,
        &[
            "-C".as_ref(),
            source.as_os_str(),
            "archive".as_ref(),
            "--format=tar".as_ref(),
            output.as_os_str(),
            revision.as_ref(),
        ],
    )?;
    tar::Archive::new(fs::File::open(&archive)?)
        .unpack(&staged)
        .context("Cannot extract the Ghostty source archive")?;
    let mut modified: Vec<(String, String)> = Vec::new();
    for (name, text) in files.iter().zip(&texts) {
        let path = temporary.path().join(name);
        fs::write(&path, text)?;
        let apply = |check: bool| {
            let mut args = vec!["-C".as_ref(), staged.as_os_str(), "apply".as_ref()];
            if check {
                args.push("--check".as_ref());
            }
            args.push(path.as_os_str());
            git(&patch_env, &args)
        };
        apply(true)?;
        let files = targets(text);
        ensure!(
            files.iter().all(|f| valid_target(f)),
            "Invalid Ghostty patch path"
        );
        let mut before = HashMap::new();
        for file in &files {
            before.insert(*file, sha256_file(&staged.join(file))?);
        }
        apply(false)?;
        for file in files {
            let hash = sha256_file(&staged.join(file))?;
            if before[file] == hash {
                bail!("Ghostty patch was not applied: {name} ({file})");
            }
            match modified.iter_mut().find(|(f, _)| f == file) {
                Some(entry) => entry.1 = hash,
                None => modified.push((file.to_owned(), hash)),
            }
        }
    }
    let manifest = NewManifest {
        revision,
        fingerprint: &fingerprint,
        source: source.to_str().context("Non-UTF-8 Ghostty source path")?,
        patch_set: &patch_set,
        modified_files: &modified,
    };
    fs::write(
        staged.join(MANIFEST),
        serde_json::to_string_pretty(&manifest)?,
    )?;
    match fs::rename(&staged, &destination) {
        Ok(()) => Ok(destination),
        // A concurrent build may have prepared the identical source first.
        Err(_) if destination.exists() => prepare(&source, revision, patches, cache, env),
        Err(error) => Err(error).with_context(|| format!("Cannot store {}", destination.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        _directory: tempfile::TempDir,
        source: PathBuf,
        revision: String,
        patches: PathBuf,
        cache: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let directory = tempfile::Builder::new()
                .prefix("newport-ghostty-source-test-")
                .tempdir()
                .unwrap();
            let source = directory.path().join("source");
            fs::create_dir_all(source.join("src")).unwrap();
            fs::write(source.join("src/terminal.zig"), "original\n").unwrap();
            let git = |args: &[&str]| {
                let mut all = vec!["-C", source.to_str().unwrap()];
                all.extend(args);
                git(&BuildEnv::default(), &all).unwrap()
            };
            git(&["init", "--quiet"]);
            git(&["add", "src"]);
            git(&[
                "-c",
                "user.name=Newport Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--quiet",
                "-m",
                "fixture",
            ]);
            let revision = git(&["rev-parse", "HEAD"]);
            let patches = source.join("patches");
            fs::create_dir(&patches).unwrap();
            // Deliberately nest the cache inside a repository, just like target/.
            let cache = source.join("target");
            let fixture = Self {
                source,
                revision,
                patches,
                cache,
                _directory: directory,
            };
            fixture.patch("fixed");
            fixture
        }

        fn patch_path(&self) -> PathBuf {
            self.patches.join("001-test.patch")
        }

        fn patch(&self, text: &str) {
            fs::write(
                self.patch_path(),
                format!("--- a/src/terminal.zig\n+++ b/src/terminal.zig\n@@ -1 +1 @@\n-original\n+{text}\n"),
            )
            .unwrap();
        }

        fn prepare_from(&self, source: &Path) -> Result<PathBuf> {
            prepare(
                source,
                &self.revision,
                &self.patches,
                &self.cache,
                &BuildEnv::default(),
            )
        }

        fn prepare(&self) -> Result<PathBuf> {
            self.prepare_from(&self.source)
        }
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path.join("src/terminal.zig")).unwrap()
    }

    #[test]
    fn patches_apply_inside_a_parent_repository_without_changing_the_base() {
        let f = Fixture::new();
        fs::write(f.source.join("src/terminal.zig"), "local edit\n").unwrap();
        let prepared = f.prepare().unwrap();
        assert_eq!(read(&prepared), "fixed\n");
        assert_eq!(read(&f.source), "local edit\n");
        assert_eq!(f.prepare_from(&prepared).unwrap(), prepared);
    }

    #[test]
    fn changing_a_patch_rebuilds_from_the_pinned_base_through_a_nested_wrapper() {
        let f = Fixture::new();
        let first = f.prepare().unwrap();
        f.patch("second fix");
        let next = f.prepare_from(&first).unwrap();
        assert_ne!(next, first);
        assert_eq!(read(&next), "second fix\n");
        assert_eq!(read(&first), "fixed\n");
    }

    #[test]
    fn crlf_patches_apply_to_the_archived_lf_source() {
        let f = Fixture::new();
        let original = fs::read_to_string(f.patch_path()).unwrap();
        fs::write(f.patch_path(), original.replace('\n', "\r\n")).unwrap();
        let prepared = f.prepare().unwrap();
        assert_eq!(read(&prepared), "fixed\n");
        fs::write(f.patch_path(), original).unwrap();
        assert_eq!(f.prepare().unwrap(), prepared);
    }

    #[test]
    fn a_modified_prepared_source_fails_instead_of_compiling_different_code() {
        let f = Fixture::new();
        let prepared = f.prepare().unwrap();
        fs::write(prepared.join("src/terminal.zig"), "unexpected\n").unwrap();
        let error = f.prepare().unwrap_err().to_string();
        assert!(error.contains("source was modified"), "{error}");
    }

    #[test]
    fn a_patch_that_does_not_apply_fails_before_exposing_a_build_directory() {
        let f = Fixture::new();
        fs::write(
            f.patch_path(),
            "--- a/src/terminal.zig\n+++ b/src/terminal.zig\n@@ -1 +1 @@\n-missing\n+fixed\n",
        )
        .unwrap();
        let error = f.prepare().unwrap_err().to_string();
        assert!(
            error.contains("patch does not apply") || error.contains("patch failed"),
            "{error}"
        );
        let leftovers: Vec<_> = fs::read_dir(&f.cache).unwrap().collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn the_manifest_matches_the_shared_cache_format() {
        let f = Fixture::new();
        let prepared = f.prepare().unwrap();
        let text = fs::read_to_string(prepared.join(MANIFEST)).unwrap();
        let patch = sha256_hex(fs::read(f.patch_path()).unwrap().as_slice());
        let fingerprint = sha256_hex(
            format!(
                r#"{{"preparationVersion":4,"revision":"{}","patchSet":[{{"name":"001-test.patch","hash":"{patch}"}}]}}"#,
                f.revision
            )
            .as_bytes(),
        );
        assert!(prepared.ends_with(format!("patched-{}-{}", f.revision, &fingerprint[..16])));
        let expected = format!(
            "{{\n  \"revision\": \"{}\",\n  \"fingerprint\": \"{fingerprint}\",\n  \"source\": \"{}\",\n  \"patchSet\": [\n    {{\n      \"name\": \"001-test.patch\",\n      \"hash\": \"{patch}\"\n    }}\n  ],\n  \"modifiedFiles\": {{\n    \"src/terminal.zig\": \"{}\"\n  }}\n}}",
            f.revision,
            f.source.display(),
            sha256_hex(b"fixed\n"),
        );
        assert_eq!(text, expected);
    }

    #[test]
    fn patch_targets_stay_inside_the_source() {
        assert_eq!(
            targets("+++ b/src/a.zig\r\n+++ b/src/b.zig\n--- a/x\n+++ b/"),
            ["src/a.zig", "src/b.zig"]
        );
        assert!(valid_target("src/terminal/Screen.zig"));
        for file in [
            "../escape",
            "src/../../escape",
            "/etc/passwd",
            "src\\a.zig",
            "./a",
        ] {
            assert!(!valid_target(file), "{file}");
        }
    }
}
