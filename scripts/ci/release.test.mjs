import { test } from "node:test";
import assert from "node:assert/strict";
import {
  mkdtempSync,
  mkdirSync,
  writeFileSync,
  readFileSync,
  copyFileSync,
  rmSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { execFileSync, spawnSync } from "node:child_process";
import { root } from "../tooling.mjs";
import { validate, profileEntitlements } from "../check-signing.mjs";
import { notarizationAuth } from "../notarize.mjs";
import { verifiedCiBuild, verifyReleaseOnlyChanges } from "./verify-release.mjs";

test("older CI is reusable only for release tooling, never changed application or CI tests", () => {
  const temp = mkdtempSync(join(tmpdir(), "newport-release-baseline-"));
  const git = (...args) => execFileSync("git", [
    "-c", "commit.gpgsign=false", "-c", "user.name=Test",
    "-c", "user.email=test@example.invalid", ...args,
  ], { cwd: temp, encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim();
  const write = (path, contents) => {
    mkdirSync(join(temp, path, ".."), { recursive: true });
    writeFileSync(join(temp, path), contents);
  };
  const commit = () => { git("add", "."); git("commit", "-m", "fixture"); return git("rev-parse", "HEAD"); };
  try {
    git("init");
    write("scripts/with-ghostty.mjs", "original");
    write(".github/workflows/checks.yml", "run: tests\nretention-days: 7\n");
    write("tools/agent/src/main.rs", "original");
    const baseline = commit();
    write("scripts/with-ghostty.mjs", "fixed build path");
    write(".github/workflows/checks.yml", "run: tests\nretention-days: 30\n");
    const release = commit();
    verifyReleaseOnlyChanges(baseline, release, git);
    assert.throws(() => verifyReleaseOnlyChanges(release, baseline, git));
    write(".github/workflows/checks.yml", "run: skip tests\nretention-days: 30\n");
    assert.throws(() => verifyReleaseOnlyChanges(baseline, commit(), git), /checks.yml changed/);
    git("checkout", release, "--", ".github/workflows/checks.yml");
    write("tools/agent/src/main.rs", "changed");
    assert.throws(() => verifyReleaseOnlyChanges(baseline, commit(), git), /main.rs changed/);
    git("checkout", baseline, "--", "tools/agent/src/main.rs");
    commit();
    mkdirSync(join(temp, "scripts/ci"));
    git("mv", "tools/agent/src/main.rs", "scripts/ci/verify-release.mjs");
    assert.throws(() => verifyReleaseOnlyChanges(baseline, commit(), git), /main.rs changed/);
  } finally { rmSync(temp, { recursive: true, force: true }); }
});

function ciFixture() {
  const commit = "a".repeat(40);
  const run = { id: 10, workflow_id: 2, head_sha: commit, head_branch: "main",
    head_repository: { full_name: "owner/repo" }, event: "push",
    status: "completed", conclusion: "success" };
  const state = {
    run, runs: [run],
    jobs: [{ name: "checks / All checks passed", status: "completed", conclusion: "success" }],
    artifacts: [{ id: 20, name: "linux-agents", expired: false, size_in_bytes: 100,
      workflow_run: { id: run.id, head_sha: commit } }],
  };
  const api = (endpoint) => {
    if (endpoint.endsWith("/workflows/ci.yml")) return { id: 2 };
    if (endpoint.includes("/workflows/ci.yml/runs?")) return { workflow_runs: state.runs };
    if (endpoint.includes("/jobs?filter=latest")) return { jobs: state.jobs };
    if (endpoint.includes("/artifacts?")) return { artifacts: state.artifacts };
    if (endpoint.endsWith(`/runs/${state.run.id}`)) return state.run;
    throw new Error(`Unexpected API request: ${endpoint}`);
  };
  return { state, check: () => verifiedCiBuild("owner/repo", commit, api) };
}

test("release reuses successful exact-commit CI and its immutable artifact ID", () => {
  const { state, check } = ciFixture();
  assert.deepEqual(check(), { runId: 10, artifactId: 20 });
  state.run.event = "workflow_dispatch";
  assert.deepEqual(check(), { runId: 10, artifactId: 20 });
});

for (const [name, change, error] of [
  ["no CI", (s) => { s.runs = []; }, /No main CI run/],
  ["different commit", (s) => { s.run.head_sha = "b".repeat(40); }, /No main CI run/],
  ["different workflow", (s) => { s.run.workflow_id = 3; }, /No main CI run/],
  ["branch build", (s) => { s.run.head_branch = "feature"; }, /No main CI run/],
  ["PR build", (s) => { s.run.event = "pull_request"; }, /No main CI run/],
  ["fork build", (s) => { s.run.head_repository.full_name = "fork/repo"; }, /No main CI run/],
  ["pending CI", (s) => { s.run.status = "in_progress"; }, /has not passed/],
  ["failed CI", (s) => { s.run.conclusion = "failure"; }, /has not passed/],
  ["skipped aggregate gate", (s) => { s.jobs[0].conclusion = "skipped"; }, /all-checks gate/],
  ["missing aggregate gate", (s) => { s.jobs = []; }, /all-checks gate/],
  ["expired artifact", (s) => { s.artifacts[0].expired = true; }, /no valid linux-agents/],
  ["missing artifact", (s) => { s.artifacts = []; }, /no valid linux-agents/],
  ["ambiguous artifacts", (s) => { s.artifacts.push({ ...s.artifacts[0], id: 21 }); }, /no valid linux-agents/],
  ["empty artifact", (s) => { s.artifacts[0].size_in_bytes = 0; }, /no valid linux-agents/],
  ["artifact from another commit", (s) => { s.artifacts[0].workflow_run.head_sha = "b".repeat(40); }, /no valid linux-agents/],
  ["artifact from another run", (s) => { s.artifacts[0].workflow_run.id = 9; }, /no valid linux-agents/],
]) {
  test(`release rejects ${name}`, () => {
    const { state, check } = ciFixture();
    change(state);
    assert.throws(check, error);
  });
}

test("an older success cannot mask a newer failing CI run", () => {
  const { state, check } = ciFixture();
  state.runs.push({ ...state.run, id: 9 });
  state.run.conclusion = "failure";
  assert.throws(check, /CI 10 has not passed/);
});

test("a rerun started after listing invalidates the previous CI success", () => {
  const { state, check } = ciFixture();
  state.run = { ...state.run, status: "in_progress", conclusion: null };
  assert.throws(check, /has not passed/);
});

test("notarization supports local keychain and CI credentials", () => {
  assert.deepEqual(notarizationAuth({ NEWPORT_NOTARY_PROFILE: "release" }), [
    "--keychain-profile",
    "release",
  ]);
  const ci = {
    APPLE_ID: "release@example.invalid",
    APPLE_PASSWORD: "fixture",
    APPLE_TEAM_ID: "TEAM",
  };
  assert.deepEqual(notarizationAuth(ci), [
    "--apple-id",
    ci.APPLE_ID,
    "--password",
    ci.APPLE_PASSWORD,
    "--team-id",
    ci.APPLE_TEAM_ID,
  ]);
  for (const name of Object.keys(ci)) {
    const missing = { ...ci };
    delete missing[name];
    assert.throws(
      () => notarizationAuth(missing),
      /Set NEWPORT_NOTARY_PROFILE/,
    );
  }
});
test("release retains the installed product and updater identity", () => {
  const product = JSON.parse(
    readFileSync(join(root, "resources/app.json"), "utf8"),
  );
  assert.equal(product.productName, "Newport");
  assert.equal(product.identifier, "app.newport");
  const cargo = readFileSync(
    join(root, "crates/newport-app/Cargo.toml"),
    "utf8",
  );
  assert.equal(cargo.match(/^version\s*=\s*"([^"]+)"/m)?.[1], product.version);
  const updater = JSON.parse(
    readFileSync(join(root, "resources/updater.json"), "utf8"),
  );
  assert.equal(updater.requireSignedVersion, true);
  assert.deepEqual(updater.endpoints, [
    "https://github.com/ruiyangke/newport/releases/latest/download/latest.json",
  ]);
  // Installed releases trust this key; rotating it would strand their updates.
  assert.equal(
    updater.pubkey,
    "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDc3MjIzQTY2RTM2QkJERDIKUldUU3ZXdmpaam9pZDhjZjFHN1ZGWmlSZ1crL2NoRUZYd0JUU0RGbE9tc1k4OWN6dnZkOFFnOTYK",
  );
  for (const path of ["icon.icns", "icon.ico", "icon.png", "tray.png"])
    assert.ok(
      readFileSync(join(root, "resources/icons", path)).length > 0,
      path,
    );
});

test("release requires matching versions and an immutable tag", () => {
  const temp = mkdtempSync(join(tmpdir(), "newport-release-"));
  try {
    mkdirSync(join(temp, "scripts/ci"), { recursive: true });
    mkdirSync(join(temp, "crates/newport-app"), { recursive: true });
    mkdirSync(join(temp, "resources"));
    for (const path of ["scripts/ci/verify-release.mjs", "scripts/tooling.mjs"])
      copyFileSync(join(root, path), join(temp, path));
    for (const path of [
      "package.json",
      "package-lock.json",
      "resources/app.json",
    ])
      writeFileSync(join(temp, path), JSON.stringify({ version: "1.2.3" }));
    const cargo = join(temp, "crates/newport-app/Cargo.toml");
    writeFileSync(cargo, '[package]\nversion = "1.2.3"\n');
    const git = (...args) =>
      execFileSync(
        "git",
        ["-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false", ...args],
        { cwd: temp, encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] },
      ).trim();
    git("init");
    git("add", ".");
    git(
      "-c",
      "user.name=Test",
      "-c",
      "user.email=test@example.invalid",
      "commit",
      "-m",
      "Fixture",
    );
    git("tag", "v1.2.3");
    const env = {
      ...process.env,
      RELEASE_TAG: "v1.2.3",
      GITHUB_OUTPUT: join(temp, "output"),
    };
    const check = () =>
      spawnSync(process.execPath, ["scripts/ci/verify-release.mjs"], {
        cwd: temp,
        env,
        encoding: "utf8",
      });
    assert.equal(check().status, 0);
    assert.equal(
      readFileSync(env.GITHUB_OUTPUT, "utf8"),
      `commit=${git("rev-parse", "HEAD")}\n`,
    );
    writeFileSync(cargo, '[package]\nversion = "1.2.4"\n');
    assert.match(check().stderr, /versions must match/);
    git("checkout", "--", "crates/newport-app/Cargo.toml");
    writeFileSync(
      join(temp, "resources/app.json"),
      JSON.stringify({ version: "1.2.4" }),
    );
    assert.match(check().stderr, /versions must match/);
    git("checkout", "--", "resources/app.json");
    git(
      "-c",
      "user.name=Test",
      "-c",
      "user.email=test@example.invalid",
      "commit",
      "--allow-empty",
      "-m",
      "Different commit",
    );
    assert.match(check().stderr, /must match the release tag/);
    env.RELEASE_TAG = "main";
    assert.match(check().stderr, /Expected a version tag/);
  } finally {
    rmSync(temp, { recursive: true, force: true });
  }
});
test("signing requires the Newport keychain group and authorized groups only", () => {
  const ent = {
    "com.apple.application-identifier": "TEAM.app.newport",
    "keychain-access-groups": ["TEAM.app.newport"],
  };
  const profile = {
    Entitlements: {
      "com.apple.application-identifier": "TEAM.app.newport",
      "keychain-access-groups": ["TEAM.*"],
    },
  };
  validate(ent, profile, "app.newport");
  for (const bad of [
    { ...ent, "com.apple.application-identifier": "TEAM.com.example.other" },
    ...[
      [],
      ["TEAM.com.example.other", "TEAM.app.newport"],
      ["OTHER.app.newport"],
      ["TEAM.app.newport", "OTHER.com.example.other"],
    ].map((groups) => ({ ...ent, "keychain-access-groups": groups })),
  ])
    assert.throws(() => validate(bad, profile, "app.newport"));
});

test(
  "profile extraction accepts certificate data and date fields",
  { skip: process.platform !== "darwin" },
  () => {
    const xml =
      '<?xml version="1.0"?><plist version="1.0"><dict><key>CreationDate</key><date>2026-01-01T00:00:00Z</date><key>Entitlements</key><dict><key>keychain-access-groups</key><array><string>TEAM.*</string></array></dict><key>DeveloperCertificates</key><array><data>YWJj</data></array></dict></plist>';
    assert.deepEqual(profileEntitlements(xml), {
      "keychain-access-groups": ["TEAM.*"],
    });
  },
);


test("desktop release verification does not enable private-key signing", () => {
  for (const target of ["newport-release", "newport-backend"]) {
    const tree = execFileSync("cargo", [
      "tree", "--locked", "-p", target, "--no-default-features",
      "--edges", "normal,build", "--prefix", "none",
    ], { cwd: root, encoding: "utf8" });
    assert.match(tree, /^minisign-verify v/m, target);
    assert.doesNotMatch(tree, /^minisign v/m, target);
    assert.doesNotMatch(tree, /^newport-needle-release v/m, target);
  }
  const manifest = readFileSync(join(root, "crates/newport-release/Cargo.toml"), "utf8");
  assert.match(manifest, /default = \[\]/);
  assert.match(manifest, /required-features = \["signing"\]/);
  const wrapper = readFileSync(join(root, "scripts/sign-update.mjs"), "utf8");
  assert.match(wrapper, /"newport-release", "--features", "signing"/);
});
