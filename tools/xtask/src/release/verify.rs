//! Release gates: the tag matches every version and the checkout, and a release
//! reuses only a successful main CI run of the same or a release-tooling-only ancestor.
use crate::util;
use anyhow::{ensure, Context, Result};
use serde::{de::DeserializeOwned, Deserialize};
use std::{env, fs, path::Path, process::Command};

/// Files whose changes cannot alter what CI builds or tests.
const RELEASE_ONLY_FILES: &[&str] = &[".github/workflows/release.yml"];
/// Directories of release-only tooling. The rest of tools/xtask builds, stages
/// and tests what CI validates, so changing it requires a fresh CI run.
const RELEASE_ONLY_DIRECTORIES: &[&str] = &["tools/xtask/src/release/"];
/// CI workflow whose artifact retention may change without invalidating a run.
const CHECKS_WORKFLOW: &str = ".github/workflows/checks.yml";
const ALL_CHECKS_JOB: &str = "checks / All checks passed";
const AGENTS_ARTIFACT: &str = "linux-agents";

/// The CI run a release consumes and its Linux agent artifact.
#[derive(Debug, PartialEq, Eq)]
pub struct CiBuild {
    pub run_id: u64,
    pub artifact_id: u64,
}

pub fn run(ci: bool) -> Result<()> {
    let root = util::root();
    let git = |args: &[&str]| util::capture(Command::new("git").current_dir(&root).args(args));
    let commit = verify_checkout(&root, &env::var("RELEASE_TAG").unwrap_or_default(), git)?;
    let mut outputs = vec![("commit", commit.clone())];
    if ci {
        let ci_commit = env::var("RELEASE_CI_COMMIT")
            .ok()
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| commit.clone());
        if ci_commit != commit {
            verify_release_only_changes(&ci_commit, &commit, git)?;
        }
        let repository = env::var("GITHUB_REPOSITORY").unwrap_or_default();
        let build = verified_ci_build(&repository, &ci_commit, |endpoint| {
            let json = util::capture(util::command("gh").args(["api", endpoint]))?;
            serde_json::from_str(&json).context("gh api printed invalid JSON")
        })?;
        println!(
            "Release {commit} uses successful CI {} at {ci_commit}",
            build.run_id
        );
        outputs.extend([
            ("ci_commit", ci_commit),
            ("ci_run_id", build.run_id.to_string()),
            ("agents_artifact_id", build.artifact_id.to_string()),
        ]);
    }
    if env::var_os("GITHUB_OUTPUT").is_some() {
        for (name, value) in outputs {
            util::github_output(name, &value)?;
        }
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

fn is_commit(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
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

/// `checks.yml` with every `retention-days:` value removed.
fn without_retention(text: &str) -> String {
    text.split('\n')
        .map(|line| {
            let key = "retention-days:";
            let indent = line.len() - line.trim_start().len();
            match line[indent..].strip_prefix(key) {
                Some(value)
                    if !value.trim().is_empty()
                        && value.trim().bytes().all(|b| b.is_ascii_digit()) =>
                {
                    &line[..indent + key.len()]
                }
                _ => line,
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Require `commit` to descend from `baseline` and differ only in release tooling.
pub fn verify_release_only_changes(
    baseline: &str,
    commit: &str,
    git: impl Fn(&[&str]) -> Result<String>,
) -> Result<()> {
    ensure!(
        is_commit(baseline) && is_commit(commit),
        "Expected exact CI and release commits"
    );
    git(&["merge-base", "--is-ancestor", baseline, commit])
        .context("The CI commit is not an ancestor of the release commit")?;
    let changed = git(&[
        "diff",
        "--no-renames",
        "--name-only",
        "-z",
        baseline,
        commit,
    ])?;
    for path in changed.split('\0').filter(|p| !p.is_empty()) {
        if RELEASE_ONLY_FILES.contains(&path)
            || RELEASE_ONLY_DIRECTORIES.iter().any(|d| path.starts_with(d))
        {
            continue;
        }
        if path == CHECKS_WORKFLOW {
            let show =
                |rev: &str| git(&["show", &format!("{rev}:{path}")]).map(|t| without_retention(&t));
            if show(baseline)? == show(commit)? {
                continue;
            }
        }
        anyhow::bail!("Cannot reuse older CI: {path} changed; run CI for the release commit");
    }
    Ok(())
}

#[derive(Deserialize)]
struct Workflow {
    id: u64,
}

#[derive(Deserialize)]
struct Runs {
    workflow_runs: Vec<Run>,
}

#[derive(Deserialize, Clone)]
struct Run {
    id: u64,
    workflow_id: u64,
    head_sha: String,
    head_branch: Option<String>,
    head_repository: Option<Repository>,
    event: String,
    status: Option<String>,
    conclusion: Option<String>,
}

#[derive(Deserialize, Clone)]
struct Repository {
    full_name: String,
}

#[derive(Deserialize)]
struct Jobs {
    jobs: Vec<Job>,
}

#[derive(Deserialize)]
struct Job {
    name: String,
    status: Option<String>,
    conclusion: Option<String>,
}

#[derive(Deserialize)]
struct Artifacts {
    artifacts: Vec<Artifact>,
}

#[derive(Deserialize)]
struct Artifact {
    id: u64,
    name: String,
    expired: bool,
    size_in_bytes: u64,
    workflow_run: Option<ArtifactRun>,
}

#[derive(Deserialize)]
struct ArtifactRun {
    id: Option<u64>,
    head_sha: Option<String>,
}

fn is_repository(value: &str) -> bool {
    let part = |p: &str| {
        !p.is_empty()
            && p.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
    };
    value
        .split_once('/')
        .is_some_and(|(owner, name)| part(owner) && part(name))
}

/// The newest main CI run for `commit`, required to have passed with its
/// all-checks gate and exactly one live `linux-agents` artifact. An older
/// success never masks a newer failed or pending run of the same commit.
pub fn verified_ci_build(
    repository: &str,
    commit: &str,
    api: impl Fn(&str) -> Result<serde_json::Value>,
) -> Result<CiBuild> {
    fn get<T: DeserializeOwned>(
        api: &impl Fn(&str) -> Result<serde_json::Value>,
        endpoint: &str,
    ) -> Result<T> {
        serde_json::from_value(api(endpoint)?)
            .with_context(|| format!("Unexpected GitHub API response: {endpoint}"))
    }
    ensure!(
        is_repository(repository) && is_commit(commit),
        "Expected a repository and exact release commit"
    );
    let base = format!("repos/{repository}/actions");
    let workflow: Workflow = get(&api, &format!("{base}/workflows/ci.yml"))?;
    let trusted = |run: &Run| {
        run.workflow_id == workflow.id
            && run.head_sha == commit
            && run.head_branch.as_deref() == Some("main")
            && run.head_repository.as_ref().map(|r| r.full_name.as_str()) == Some(repository)
            && matches!(run.event.as_str(), "push" | "workflow_dispatch")
    };
    let runs: Runs = get(
        &api,
        &format!("{base}/workflows/ci.yml/runs?head_sha={commit}&branch=main&per_page=100"),
    )?;
    let selected = runs
        .workflow_runs
        .into_iter()
        .filter(|r| trusted(r))
        .max_by_key(|r| r.id)
        .with_context(|| format!("No main CI run for {commit}; run CI before releasing"))?;
    // Refresh after listing so a re-run cannot reuse the previous conclusion.
    let run: Run = get(&api, &format!("{base}/runs/{}", selected.id))?;
    ensure!(
        trusted(&run)
            && run.id == selected.id
            && run.status.as_deref() == Some("completed")
            && run.conclusion.as_deref() == Some("success"),
        "CI {} has not passed for {commit}; finish CI before releasing",
        selected.id
    );
    let jobs: Jobs = get(
        &api,
        &format!("{base}/runs/{}/jobs?filter=latest&per_page=100", run.id),
    )?;
    ensure!(
        jobs.jobs.iter().any(|job| job.name == ALL_CHECKS_JOB
            && job.status.as_deref() == Some("completed")
            && job.conclusion.as_deref() == Some("success")),
        "CI {} is missing its successful all-checks gate",
        run.id
    );
    let artifacts: Artifacts = get(
        &api,
        &format!("{base}/runs/{}/artifacts?per_page=100", run.id),
    )?;
    let artifacts: Vec<_> = artifacts
        .artifacts
        .into_iter()
        .filter(|a| a.name == AGENTS_ARTIFACT)
        .collect();
    match artifacts.as_slice() {
        [artifact]
            if !artifact.expired
                && artifact.size_in_bytes > 0
                && artifact.workflow_run.as_ref().is_some_and(|w| {
                    w.id == Some(run.id) && w.head_sha.as_deref() == Some(commit)
                }) =>
        {
            Ok(CiBuild {
                run_id: run.id,
                artifact_id: artifact.id,
            })
        }
        _ => anyhow::bail!(
            "CI {} has no valid linux-agents artifact; rerun CI to regenerate it",
            run.id
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
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
    fn older_ci_is_reusable_only_for_release_tooling() {
        let repo = Repo::new();
        let git = |args: &[&str]| repo.git(args);
        repo.write("tools/xtask/src/release/mod.rs", "original");
        repo.write(CHECKS_WORKFLOW, "run: tests\n    retention-days: 7\n");
        repo.write("tools/agent/src/main.rs", "original");
        repo.write("tools/xtask/src/native.rs", "original");
        let baseline = repo.commit();
        repo.write("tools/xtask/src/release/mod.rs", "fixed release step");
        repo.write(".github/workflows/release.yml", "on: workflow_dispatch\n");
        repo.write(CHECKS_WORKFLOW, "run: tests\n    retention-days: 30\n");
        let release = repo.commit();
        verify_release_only_changes(&baseline, &release, git).unwrap();
        rejects(
            verify_release_only_changes(&release, &baseline, git),
            "not an ancestor",
        );

        repo.write(CHECKS_WORKFLOW, "run: skip tests\n    retention-days: 30\n");
        rejects(
            verify_release_only_changes(&baseline, &repo.commit(), git),
            "checks.yml changed",
        );
        repo.git(&["checkout", &release, "--", CHECKS_WORKFLOW])
            .unwrap();

        repo.write("tools/agent/src/main.rs", "changed");
        rejects(
            verify_release_only_changes(&baseline, &repo.commit(), git),
            "main.rs changed",
        );
        repo.git(&["checkout", &baseline, "--", "tools/agent/src/main.rs"])
            .unwrap();

        repo.write("tools/xtask/src/native.rs", "changed");
        rejects(
            verify_release_only_changes(&baseline, &repo.commit(), git),
            "native.rs changed",
        );
        repo.git(&["checkout", &baseline, "--", "tools/xtask/src/native.rs"])
            .unwrap();

        repo.write(
            "tools/xtask/src/release.rs",
            "outside the release directory",
        );
        rejects(
            verify_release_only_changes(&baseline, &repo.commit(), git),
            "release.rs changed",
        );
        repo.git(&["rm", "-q", "tools/xtask/src/release.rs"])
            .unwrap();
        repo.commit();

        repo.git(&[
            "mv",
            "tools/agent/src/main.rs",
            "tools/xtask/src/release/verify.rs",
        ])
        .unwrap();
        rejects(
            verify_release_only_changes(&baseline, &repo.commit(), git),
            "main.rs changed",
        );

        rejects(
            verify_release_only_changes("main", &release, git),
            "exact CI and release commits",
        );
    }

    #[test]
    fn retention_days_are_the_only_ignored_checks_change() {
        assert_eq!(
            without_retention("a\n  retention-days: 7\r\nb"),
            without_retention("a\n  retention-days:30\nb")
        );
        assert_ne!(
            without_retention("retention-days: 7 # note"),
            without_retention("retention-days: 8 # note")
        );
        assert_ne!(
            without_retention("x: retention-days: 7"),
            without_retention("x: retention-days: 8")
        );
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

    struct Ci {
        workflow: Value,
        runs: Vec<Value>,
        run: Value,
        jobs: Vec<Value>,
        artifacts: Vec<Value>,
    }

    const COMMIT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const OTHER: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn ci() -> Ci {
        let run = json!({
            "id": 10, "workflow_id": 2, "head_sha": COMMIT, "head_branch": "main",
            "head_repository": {"full_name": "owner/repo"}, "event": "push",
            "status": "completed", "conclusion": "success",
        });
        Ci {
            workflow: json!({"id": 2}),
            runs: vec![run.clone()],
            run,
            jobs: vec![
                json!({"name": ALL_CHECKS_JOB, "status": "completed", "conclusion": "success"}),
            ],
            artifacts: vec![json!({
                "id": 20, "name": "linux-agents", "expired": false, "size_in_bytes": 100,
                "workflow_run": {"id": 10, "head_sha": COMMIT},
            })],
        }
    }

    impl Ci {
        /// Apply a change to the listed run and its refreshed copy.
        fn both(mut self, change: impl Fn(&mut Value)) -> Self {
            change(&mut self.run);
            self.runs.iter_mut().for_each(&change);
            self
        }

        fn check(&self) -> Result<CiBuild> {
            verified_ci_build("owner/repo", COMMIT, |endpoint| {
                let base = "repos/owner/repo/actions";
                Ok(if endpoint == format!("{base}/workflows/ci.yml") {
                    self.workflow.clone()
                } else if endpoint.starts_with(&format!(
                    "{base}/workflows/ci.yml/runs?head_sha={COMMIT}&branch=main"
                )) {
                    json!({"workflow_runs": self.runs})
                } else if endpoint.contains("/jobs?filter=latest") {
                    json!({"jobs": self.jobs})
                } else if endpoint.contains("/artifacts?") {
                    json!({"artifacts": self.artifacts})
                } else if endpoint == format!("{base}/runs/{}", self.run["id"]) {
                    self.run.clone()
                } else {
                    anyhow::bail!("Unexpected API request: {endpoint}")
                })
            })
        }
    }

    #[test]
    fn release_reuses_successful_exact_commit_ci_and_its_artifact() {
        let expected = CiBuild {
            run_id: 10,
            artifact_id: 20,
        };
        assert_eq!(ci().check().unwrap(), expected);
        let dispatch = ci().both(|r| r["event"] = json!("workflow_dispatch"));
        assert_eq!(dispatch.check().unwrap(), expected);
    }

    #[test]
    fn release_rejects_untrusted_or_unfinished_ci() {
        type Change = fn(Ci) -> Ci;
        let cases: &[(&str, Change, &str)] = &[
            (
                "no CI",
                |mut c| {
                    c.runs.clear();
                    c
                },
                "No main CI run",
            ),
            (
                "different commit",
                |c| c.both(|r| r["head_sha"] = json!(OTHER)),
                "No main CI run",
            ),
            (
                "different workflow",
                |c| c.both(|r| r["workflow_id"] = json!(3)),
                "No main CI run",
            ),
            (
                "branch build",
                |c| c.both(|r| r["head_branch"] = json!("feature")),
                "No main CI run",
            ),
            (
                "PR build",
                |c| c.both(|r| r["event"] = json!("pull_request")),
                "No main CI run",
            ),
            (
                "fork build",
                |c| c.both(|r| r["head_repository"]["full_name"] = json!("fork/repo")),
                "No main CI run",
            ),
            (
                "pending CI",
                |c| c.both(|r| r["status"] = json!("in_progress")),
                "has not passed",
            ),
            (
                "failed CI",
                |c| c.both(|r| r["conclusion"] = json!("failure")),
                "has not passed",
            ),
            (
                "skipped gate",
                |mut c| {
                    c.jobs[0]["conclusion"] = json!("skipped");
                    c
                },
                "all-checks gate",
            ),
            (
                "missing gate",
                |mut c| {
                    c.jobs.clear();
                    c
                },
                "all-checks gate",
            ),
            (
                "expired artifact",
                |mut c| {
                    c.artifacts[0]["expired"] = json!(true);
                    c
                },
                "no valid linux-agents",
            ),
            (
                "missing artifact",
                |mut c| {
                    c.artifacts.clear();
                    c
                },
                "no valid linux-agents",
            ),
            (
                "ambiguous artifacts",
                |mut c| {
                    let mut copy = c.artifacts[0].clone();
                    copy["id"] = json!(21);
                    c.artifacts.push(copy);
                    c
                },
                "no valid linux-agents",
            ),
            (
                "empty artifact",
                |mut c| {
                    c.artifacts[0]["size_in_bytes"] = json!(0);
                    c
                },
                "no valid linux-agents",
            ),
            (
                "artifact from another commit",
                |mut c| {
                    c.artifacts[0]["workflow_run"]["head_sha"] = json!(OTHER);
                    c
                },
                "no valid linux-agents",
            ),
            (
                "artifact from another run",
                |mut c| {
                    c.artifacts[0]["workflow_run"]["id"] = json!(9);
                    c
                },
                "no valid linux-agents",
            ),
        ];
        for (name, change, pattern) in cases {
            let error = format!("{:#}", change(ci()).check().unwrap_err());
            assert!(error.contains(pattern), "{name}: {error}");
        }
        let error = verified_ci_build("owner/repo/extra", COMMIT, |_| unreachable!()).unwrap_err();
        assert!(error.to_string().contains("Expected a repository"));
        let error = verified_ci_build("owner/repo", "HEAD", |_| unreachable!()).unwrap_err();
        assert!(error.to_string().contains("Expected a repository"));
    }

    #[test]
    fn an_older_success_cannot_mask_a_newer_failing_run() {
        let mut c = ci();
        let mut older = c.run.clone();
        older["id"] = json!(9);
        c.runs.push(older);
        c.run["conclusion"] = json!("failure");
        c.runs[0]["conclusion"] = json!("failure");
        let error = c.check().unwrap_err().to_string();
        assert!(error.contains("CI 10 has not passed"), "{error}");
    }

    #[test]
    fn a_rerun_started_after_listing_invalidates_the_previous_success() {
        let mut c = ci();
        c.run["status"] = json!("in_progress");
        c.run["conclusion"] = Value::Null;
        let error = c.check().unwrap_err().to_string();
        assert!(error.contains("has not passed"), "{error}");
    }
}
