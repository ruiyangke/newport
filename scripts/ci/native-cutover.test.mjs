import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { root } from "../tooling.mjs";

function legacyBuildDependencies(source) {
  // Check dependency paths and build commands, not the word "Tauri": existing
  // installations still trust the updater key kept under legacy secret names.
  const rules = [
    ["legacy host path", /\bsrc-tauri\b/],
    ["Tauri npm command", /\b(?:npm|pnpm|yarn)\s+(?:run\s+|exec\s+)?tauri\b/],
    ["Tauri Cargo command", /\bcargo\s+tauri\b/],
    ["Tauri CLI package", /@tauri-apps\/cli\b/],
    ["Tauri release action", /tauri-apps\/tauri-action@/],
  ];
  return rules
    .filter(([, pattern]) => pattern.test(source))
    .map(([label]) => label);
}

function sources(directory) {
  return readdirSync(join(root, directory), { withFileTypes: true }).flatMap(
    (entry) => {
      const path = `${directory}/${entry.name}`;
      if (entry.isDirectory()) return sources(path);
      return entry.isFile() &&
        /\.(?:ya?ml|m?js|sh|ps1)$/.test(entry.name) &&
        !/\.test\.mjs$/.test(entry.name)
        ? [path]
        : [];
    },
  );
}

test("CI and release tooling cannot restore a dependency on the retired desktop host", () => {
  assert.equal(existsSync(join(root, "src-tauri")), false);
  const manifest = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
  assert.equal(manifest.dependencies?.["@tauri-apps/cli"], undefined);
  assert.equal(manifest.devDependencies?.["@tauri-apps/cli"], undefined);
  for (const [name, command] of Object.entries(manifest.scripts))
    assert.deepEqual(legacyBuildDependencies(command), [], `npm run ${name}`);
  const paths = [...sources(".github/workflows"), ...sources("scripts")];
  // Include native packaging and its helpers, not just the workflow command.
  for (const required of [
    ".github/workflows/checks.yml",
    ".github/workflows/release.yml",
    "scripts/package-native.mjs",
    "scripts/package-windows-native.mjs",
    "scripts/release-native-macos.mjs",
    "scripts/ci/verify-release.mjs",
  ])
    assert.ok(paths.includes(required), `Missing guarded entry: ${required}`);
  const failures = paths.flatMap((path) =>
    legacyBuildDependencies(readFileSync(join(root, path), "utf8")).map(
      (reason) => `${path}: ${reason}`,
    ),
  );
  assert.deepEqual(failures, []);
});

test("retirement guard rejects old build routes but permits existing signing secret aliases", () => {
  for (const source of [
    "workspaces: src-tauri",
    "cargo test --manifest-path src-tauri/Cargo.toml",
    "path: src-tauri/target/release/update/*",
    'readFileSync(join(root, "src-tauri", "tauri.conf.json"))',
    "npm run tauri build -- --bundles nsis",
    "cargo tauri build",
    "npx @tauri-apps/cli build",
    "uses: tauri-apps/tauri-action@v1",
  ])
    assert.ok(legacyBuildDependencies(source).length, source);
  assert.deepEqual(
    legacyBuildDependencies(`
    TAURI_SIGNING_PRIVATE_KEY: \${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}
    TAURI_SIGNING_PRIVATE_KEY_PASSWORD: \${{ secrets.TAURI_SIGNING_PRIVATE_KEY_PASSWORD }}
    run: node scripts/release-native-macos.mjs
  `),
    [],
  );
});


test("desktop UI retirement leaves a dependency-free native root and independent test oracles", () => {
  for (const path of ["src", "index.html", "vite.config.ts", "playwright.config.ts", "playwright.production.config.ts", "playwright.files.config.ts"])
    assert.equal(existsSync(join(root, path)), false, path);
  const manifest = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
  const lock = JSON.parse(readFileSync(join(root, "package-lock.json"), "utf8"));
  assert.deepEqual(Object.keys(manifest.dependencies ?? {}), []);
  assert.deepEqual(Object.keys(manifest.devDependencies ?? {}), []);
  assert.deepEqual(Object.keys(lock.packages), [""]);
  assert.match(manifest.scripts.build, /cargo build.*-p newport-app/);
  assert.match(manifest.scripts.test, /native/);
  const checks = readFileSync(join(root, ".github/workflows/checks.yml"), "utf8");
  assert.doesNotMatch(checks, /playwright|name: frontend|path: dist/);
  assert.match(checks, /npm run test:git-cli-contract/);
  assert.match(checks, /needs: \[tooling, native-core, native-ui, agent-tests, agents, remote\]/);
  for (const path of ["tools/git-contract/package-lock.json", "crates/newport-client/tests/git_contract.rs", "crates/newport-gpui/scripts/package-lock.json", "resources/test-fixtures/legacy-desktop-release.json", "tests/remote/sshd_config"])
    assert.ok(existsSync(join(root, path)), path);
});
