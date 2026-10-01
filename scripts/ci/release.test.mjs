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
test("release requires matching versions and an immutable tag", () => {
  const temp = mkdtempSync(join(tmpdir(), "newport-release-"));
  try {
    mkdirSync(join(temp, "scripts/ci"), { recursive: true });
    mkdirSync(join(temp, "src-tauri"));
    for (const path of ["scripts/ci/verify-release.mjs", "scripts/tooling.mjs"])
      copyFileSync(join(root, path), join(temp, path));
    for (const path of [
      "package.json",
      "package-lock.json",
      "src-tauri/tauri.conf.json",
    ])
      writeFileSync(join(temp, path), JSON.stringify({ version: "1.2.3" }));
    const cargo = join(temp, "src-tauri/Cargo.toml");
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
    git("checkout", "--", "src-tauri/Cargo.toml");
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
test("signing retains only authorized migration keychain groups", () => {
  const ent = {
    "com.apple.application-identifier": "TEAM.app.newport",
    "keychain-access-groups": ["TEAM.app.newport", "TEAM.com.porthop.desktop"],
  };
  const profile = {
    Entitlements: {
      "com.apple.application-identifier": "TEAM.app.newport",
      "keychain-access-groups": ["TEAM.*"],
    },
  };
  validate(ent, profile, "app.newport");
  for (const bad of [
    { ...ent, "com.apple.application-identifier": "TEAM.ke.ry.porthop" },
    ...[
      ["TEAM.app.newport"],
      ["TEAM.app.newport", "TEAM.ke.ry.porthop"],
      ["TEAM.app.newport", "OTHER.com.porthop.desktop"],
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
