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

test("native notarization supports local keychain and existing CI credentials", () => {
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
test("native release retains the installed product and updater identity", () => {
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
  // This key is already trusted by installed releases. Removing the old host
  // must not silently rotate the update signing identity.
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
