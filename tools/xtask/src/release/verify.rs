//! Release gate: the tag names every product version and the checked-out
//! commit. A release builds everything it ships from that commit.
use crate::util;
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::{env, fs, path::Path, process::Command};

pub fn run() -> Result<()> {
    let root = util::root();
    let git = |args: &[&str]| util::capture(Command::new("git").current_dir(&root).args(args));
    let commit = verify_checkout(&root, &env::var("RELEASE_TAG").unwrap_or_default(), git)?;
    if env::var_os("GITHUB_OUTPUT").is_some() {
        util::github_output("commit", &commit)?;
    }
    Ok(())
}

/// `v<major>.<minor>.<patch>[-<prerelease>]`.
fn is_version_tag(tag: &str) -> bool {
    let Some(version) = tag.strip_prefix('v') else {
        return false;
    };
    let (core, prerelease) = match version.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (version, None),
    };
    let numbers: Vec<_> = core.split('.').collect();
    numbers.len() == 3
        && numbers
            .iter()
            .all(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        && prerelease.is_none_or(|pre| {
            !pre.is_empty()
                && pre
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        })
}

/// Require the tag to name every product version and the checked-out commit; returns that commit.
pub fn verify_checkout(
    root: &Path,
    tag: &str,
    git: impl Fn(&[&str]) -> Result<String>,
) -> Result<String> {
    #[derive(Deserialize)]
    struct Versioned {
        version: String,
    }
    ensure!(is_version_tag(tag), "Expected a version tag such as v0.3.0");
    let app: Versioned = util::read_json(&root.join("resources/app.json"))?;
    let path = root.join("crates/newport-app/Cargo.toml");
    let cargo: toml::Table = fs::read_to_string(&path)
        .with_context(|| format!("Cannot read {}", path.display()))?
        .parse()
        .with_context(|| format!("Invalid TOML: {}", path.display()))?;
    let cargo = cargo
        .get("package")
        .and_then(|p| p.get("version"))
        .and_then(|v| v.as_str());
    ensure!(
        [Some(app.version.as_str()), cargo]
            .iter()
            .all(|v| v.is_some_and(|v| tag == format!("v{v}"))),
        "Release tag and package versions must match"
    );
    let commit = git(&["rev-parse", &format!("refs/tags/{tag}^{{commit}}")])?;
    ensure!(
        commit == git(&["rev-parse", "HEAD"])?,
        "Checkout must match the release tag"
    );
    Ok(commit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct Repo {
        dir: tempfile::TempDir,
    }

    impl Repo {
        fn new() -> Self {
            let repo = Self {
                dir: tempfile::tempdir().unwrap(),
            };
            repo.git(&["init", "-q"]).unwrap();
            repo
        }

        fn git(&self, args: &[&str]) -> Result<String> {
            let output = Command::new("git")
                .current_dir(self.dir.path())
                .args([
                    "-c",
                    "commit.gpgsign=false",
                    "-c",
                    "tag.gpgsign=false",
                    "-c",
                    "core.hooksPath=/dev/null",
                    "-c",
                    "user.name=Test",
                    "-c",
                    "user.email=test@example.invalid",
                ])
                .args(args)
                .env_remove("GIT_DIR")
                .env_remove("GIT_WORK_TREE")
                .output()?;
            ensure!(output.status.success(), "git {args:?} failed");
            Ok(String::from_utf8(output.stdout)?.trim().to_owned())
        }

        fn write(&self, path: &str, contents: &str) {
            let path: PathBuf = self.dir.path().join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, contents).unwrap();
        }

        fn commit(&self) -> String {
            self.git(&["add", "-A"]).unwrap();
            self.git(&["commit", "-q", "--allow-empty", "-m", "fixture"])
                .unwrap();
            self.git(&["rev-parse", "HEAD"]).unwrap()
        }
    }

    fn rejects(result: Result<()>, pattern: &str) {
        let error = format!("{:#}", result.unwrap_err());
        assert!(error.contains(pattern), "{error}");
    }

    #[test]
    fn release_requires_matching_versions_and_an_immutable_tag() {
        let repo = Repo::new();
        let git = |args: &[&str]| repo.git(args);
        let root = repo.dir.path();
        repo.write("resources/app.json", r#"{"version":"1.2.3"}"#);
        let cargo = "crates/newport-app/Cargo.toml";
        repo.write(cargo, "[package]\nversion = \"1.2.3\"\n");
        let head = repo.commit();
        repo.git(&["tag", "v1.2.3"]).unwrap();
        assert_eq!(verify_checkout(root, "v1.2.3", git).unwrap(), head);

        let check =
            |pattern: &str| rejects(verify_checkout(root, "v1.2.3", git).map(drop), pattern);
        repo.write(cargo, "[package]\nversion = \"1.2.4\"\n");
        check("versions must match");
        repo.write(
            cargo,
            "[package]\nname = \"newport-app\"\n[dependencies]\nversion = \"1.2.3\"\n",
        );
        check("versions must match");
        repo.git(&["checkout", "--", cargo]).unwrap();
        repo.write("resources/app.json", r#"{"version":"1.2.4"}"#);
        check("versions must match");
        repo.git(&["checkout", "--", "resources/app.json"]).unwrap();
        repo.commit();
        check("must match the release tag");
        for tag in ["main", "1.2.3", "v1.2", "v1.2.3-", "v1.2.3+build", "v1.2.x"] {
            rejects(
                verify_checkout(root, tag, git).map(drop),
                "Expected a version tag",
            );
        }
        assert!(is_version_tag("v1.2.3-rc.1"));
    }
}
