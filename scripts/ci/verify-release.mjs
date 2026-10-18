import { appendFileSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { root, json, capture, isMain } from "../tooling.mjs";

// Release consumes a successful CI build, never a PR artifact or an older
// passing run that hides a newer failed/pending validation of the same commit.
export function verifiedCiBuild(repository, commit, api) {
  if (!/^[\w.-]+\/[\w.-]+$/.test(repository) || !/^[0-9a-f]{40}$/.test(commit))
    throw new Error("Expected a repository and exact release commit");
  const base = `repos/${repository}/actions`;
  const workflow = api(`${base}/workflows/ci.yml`);
  const trusted = (run) => run.workflow_id === workflow.id
    && run.head_sha === commit && run.head_branch === "main"
    && run.head_repository?.full_name === repository
    && ["push", "workflow_dispatch"].includes(run.event);
  const runs = api(`${base}/workflows/ci.yml/runs?head_sha=${commit}&branch=main&per_page=100`);
  const selected = runs.workflow_runs.filter(trusted).sort((a, b) => b.id - a.id)[0];
  if (!selected) throw new Error(`No main CI run for ${commit}; run CI before releasing`);
  // Refresh after listing so a re-run cannot reuse the previous conclusion.
  const run = api(`${base}/runs/${selected.id}`);
  if (!trusted(run) || run.id !== selected.id
      || run.status !== "completed" || run.conclusion !== "success")
    throw new Error(`CI ${selected.id} has not passed for ${commit}; finish CI before releasing`);
  const jobs = api(`${base}/runs/${run.id}/jobs?filter=latest&per_page=100`);
  if (!jobs.jobs.some((job) => job.name === "checks / All checks passed"
      && job.status === "completed" && job.conclusion === "success"))
    throw new Error(`CI ${run.id} is missing its successful all-checks gate`);
  const artifacts = api(`${base}/runs/${run.id}/artifacts?per_page=100`).artifacts
    .filter((artifact) => artifact.name === "linux-agents");
  if (artifacts.length !== 1 || artifacts[0].expired || !(artifacts[0].size_in_bytes > 0)
      || artifacts[0].workflow_run?.id !== run.id
      || artifacts[0].workflow_run?.head_sha !== commit)
    throw new Error(`CI ${run.id} has no valid linux-agents artifact; rerun CI to regenerate it`);
  return { runId: run.id, artifactId: artifacts[0].id };
}

if (isMain(import.meta.url)) {
  const tag = process.env.RELEASE_TAG || "";
  if (!/^v\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(tag))
    throw new Error("Expected a version tag such as v0.3.0");
  const versions = [
    "package.json",
    "package-lock.json",
    "resources/app.json",
  ].map((path) => json(join(root, path)).version);
  const cargo = readFileSync(join(root, "crates/newport-app/Cargo.toml"), "utf8")
    .split("[package]")[1]
    ?.split(/^\[/m)[0];
  versions.push(cargo?.match(/^version\s*=\s*"([^"]+)"/m)?.[1]);
  if (versions.some((version) => `v${version}` !== tag))
    throw new Error("Release tag and package versions must match");
  const commit = capture("git", ["rev-parse", `refs/tags/${tag}^{commit}`]);
  if (commit !== capture("git", ["rev-parse", "HEAD"]))
    throw new Error("Checkout must match the release tag");
  let output = `commit=${commit}\n`;
  if (process.argv.includes("--ci")) {
    const build = verifiedCiBuild(process.env.GITHUB_REPOSITORY || "", commit,
      (endpoint) => JSON.parse(capture("gh", ["api", endpoint])));
    output += `ci_run_id=${build.runId}\nagents_artifact_id=${build.artifactId}\n`;
    console.log(`Reusing successful CI ${build.runId} for ${commit}`);
  }
  if (process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, output);
}
