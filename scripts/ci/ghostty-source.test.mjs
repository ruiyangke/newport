import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  renameSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, relative, isAbsolute, sep } from "node:path";
import { prepareGhosttySource } from "../prepare-ghostty-source.mjs";
import { GHOSTTY_CACHE, installZigDirectory, ZIG_VERSION } from "../with-ghostty.mjs";
import { root } from "../tooling.mjs";

test("nested Windows bootstrap keeps Zig's relative lib_dir usable from Cargo's build directory", () => {
  const directory = mkdtempSync(join(tmpdir(), "newport-zig nested-"));
  try {
    mkdirSync(join(directory, "scripts"));
    for (const file of ["with-ghostty.mjs", "tooling.mjs"])
      copyFileSync(join(root, "scripts", file), join(directory, "scripts", file));
    // A nested packaging wrapper sees the already patched library. Its fast
    // path returns the supplied path, just as prepareZigStandardLibrary does.
    writeFileSync(join(directory, "scripts/prepare-zig-stdlib.mjs"),
      "export const prepareZigStandardLibrary = ({ lib }) => lib;");
    writeFileSync(join(directory, "scripts/prepare-ghostty-source.mjs"),
      "export const prepareGhosttySource = ({ source }) => source;");
    const script = `
      import assert from "node:assert/strict";
      import childProcess from "node:child_process";
      import { syncBuiltinESMExports } from "node:module";
      import { mkdirSync, writeFileSync, readFileSync } from "node:fs";
      import { join, isAbsolute } from "node:path";
      Object.defineProperty(process, "platform", { value: "win32" });
      Object.defineProperty(process, "arch", { value: "x64" });
      const base = process.cwd();
      const lib = ".cache/newport/ghostty/prepared stdlib";
      mkdirSync(lib, { recursive: true });
      writeFileSync(join(lib, "sentinel"), "stdlib");
      const source = join(base, "source");
      mkdirSync(source);
      childProcess.spawnSync = (command, args) => {
        if (command === "rustc") return { status: 0, stdout: "host: x86_64-pc-windows-msvc" };
        if (command === "zig" && args[0] === "version") return { status: 0, stdout: "0.16.0" };
        if (command === "zig" && args[0] === "env") return { status: 0, stdout: '.lib_dir = ' + JSON.stringify(lib) + ',' };
        throw new Error("Unexpected build/download command: " + command);
      };
      syncBuiltinESMExports();
      const { ghosttyBuildEnv } = await import("./scripts/with-ghostty.mjs");
      const env = await ghosttyBuildEnv({ PATH: "", GHOSTTY_SOURCE_DIR: source, ZIG_LIB_DIR: join(base, lib) });
      assert.ok(isAbsolute(env.ZIG_LIB_DIR), "Zig library path must survive Cargo changing cwd");
      process.chdir(source);
      assert.equal(readFileSync(join(env.ZIG_LIB_DIR, "sentinel"), "utf8"), "stdlib");
    `;
    writeFileSync(join(directory, "check.mjs"), script);
    const result = spawnSync(process.execPath, ["check.mjs"], { cwd: directory, encoding: "utf8" });
    assert.equal(result.status, 0, result.stderr);
  } finally { rmSync(directory, { recursive: true, force: true }); }
});

test("build prerequisites stay outside Cargo's recursively pruned target cache", () => {
  const fromTarget = relative(join(root, "target"), GHOSTTY_CACHE);
  assert.ok(fromTarget.startsWith(`..${sep}`) || isAbsolute(fromTarget));
  const ignored = spawnSync("git", ["check-ignore", GHOSTTY_CACHE], { cwd: root });
  assert.equal(ignored.status, 0, "downloaded tools and source must remain untracked");
});

for (const executable of ["zig", "zig.exe"]) {
  test(`Zig repairs a partial restored cache (${executable})`, () => {
    const temp = mkdtempSync(join(tmpdir(), "newport-zig-install-"));
    try {
      const staged = join(temp, "staged");
      const install = join(temp, "install");
      mkdirSync(join(install, "lib"), { recursive: true });
      writeFileSync(join(install, "lib", "leftover"), "partial cache");
      mkdirSync(staged);
      writeFileSync(join(staged, executable), ZIG_VERSION);
      const version = (binary) => readFileSync(binary, "utf8");
      installZigDirectory(staged, install, executable, version);
      assert.equal(version(join(install, executable)), ZIG_VERSION);
      assert.equal(existsSync(join(install, "lib", "leftover")), false);
      assert.equal(existsSync(staged), false);
    } finally { rmSync(temp, { recursive: true, force: true }); }
  });
}

test("Zig rejects an invalid replacement before removing a partial cache", () => {
  const temp = mkdtempSync(join(tmpdir(), "newport-zig-install-"));
  try {
    const staged = join(temp, "staged");
    const install = join(temp, "install");
    mkdirSync(install);
    writeFileSync(join(install, "sentinel"), "keep");
    mkdirSync(staged);
    writeFileSync(join(staged, "zig"), "wrong version");
    assert.throws(() => installZigDirectory(staged, install, "zig",
      (binary) => readFileSync(binary, "utf8")), /Downloaded Zig version/);
    assert.equal(readFileSync(join(install, "sentinel"), "utf8"), "keep");
  } finally { rmSync(temp, { recursive: true, force: true }); }
});

test("Zig preserves an installation completed while the archive was downloading", () => {
  const temp = mkdtempSync(join(tmpdir(), "newport-zig-install-"));
  try {
    const staged = join(temp, "staged");
    const install = join(temp, "install");
    for (const directory of [staged, install]) {
      mkdirSync(directory);
      writeFileSync(join(directory, "zig"), ZIG_VERSION);
    }
    writeFileSync(join(install, "sentinel"), "keep");
    installZigDirectory(staged, install, "zig", (binary) => readFileSync(binary, "utf8"));
    assert.equal(readFileSync(join(install, "sentinel"), "utf8"), "keep");
    assert.equal(existsSync(staged), true);
  } finally { rmSync(temp, { recursive: true, force: true }); }
});

for (const code of ["EPERM", "EACCES", "EBUSY"]) {
  test(`Zig installation waits for a transient Windows ${code} lock`, () => {
    const temp = mkdtempSync(join(tmpdir(), "newport-zig-locked-"));
    try {
      const staged = join(temp, "staged");
      const install = join(temp, "install");
      mkdirSync(staged);
      writeFileSync(join(staged, "zig.exe"), ZIG_VERSION);
      let attempts = 0;
      const delays = [];
      installZigDirectory(staged, install, "zig.exe", (binary) => readFileSync(binary, "utf8"), {
        platform: "win32",
        rename: (...args) => {
          if (++attempts <= 2) throw Object.assign(new Error("compiler still in use"), { code });
          renameSync(...args);
        },
        wait: (ms) => delays.push(ms),
      });
      assert.equal(attempts, 3);
      assert.deepEqual(delays, [100, 100]);
      assert.equal(readFileSync(join(install, "zig.exe"), "utf8"), ZIG_VERSION);
      assert.equal(existsSync(staged), false);
    } finally { rmSync(temp, { recursive: true, force: true }); }
  });
}

for (const [platform, code, expectedAttempts] of [["win32", "EPERM", 51], ["win32", "EIO", 1], ["darwin", "EPERM", 1]]) {
  test(`Zig installation preserves the staging directory after ${platform} ${code}`, () => {
    const temp = mkdtempSync(join(tmpdir(), "newport-zig-denied-"));
    try {
      const staged = join(temp, "staged");
      const install = join(temp, "install");
      mkdirSync(staged);
      writeFileSync(join(staged, "zig.exe"), ZIG_VERSION);
      const failure = Object.assign(new Error("cannot move compiler"), { code });
      let attempts = 0;
      let waited = 0;
      assert.throws(() => installZigDirectory(staged, install, "zig.exe", (binary) => readFileSync(binary, "utf8"), {
        platform,
        rename: () => { attempts++; throw failure; },
        wait: (ms) => { waited += ms; },
      }), (error) => error === failure);
      assert.equal(attempts, expectedAttempts);
      assert.equal(waited, (expectedAttempts - 1) * 100);
      assert.equal(readFileSync(join(staged, "zig.exe"), "utf8"), ZIG_VERSION);
      assert.equal(existsSync(install), false);
    } finally { rmSync(temp, { recursive: true, force: true }); }
  });
}

function fixture(check) {
  const source = mkdtempSync(join(tmpdir(), "newport-ghostty-source-test-"));
  const git = (...args) => {
    const result = spawnSync("git", ["-C", source, ...args], {
      encoding: "utf8",
    });
    assert.equal(result.status, 0, result.stderr);
    return result.stdout.trim();
  };
  try {
    git("init", "--quiet");
    mkdirSync(join(source, "src"));
    writeFileSync(join(source, "src/terminal.zig"), "original\n");
    git("add", "src");
    git(
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
    );
    const revision = git("rev-parse", "HEAD");
    const patches = join(source, "patches");
    mkdirSync(patches);
    const patch = (text) =>
      writeFileSync(
        join(patches, "001-test.patch"),
        `--- a/src/terminal.zig\n+++ b/src/terminal.zig\n@@ -1 +1 @@\n-original\n+${text}\n`,
      );
    patch("fixed");
    // Deliberately nest the cache inside a repository, just like target/.
    const options = {
      source,
      revision,
      patches,
      cache: join(source, "target"),
      env: process.env,
    };
    check(options, patch);
  } finally {
    rmSync(source, { recursive: true, force: true });
  }
}

test("Ghostty patches are applied inside a parent repository without changing the base", () => {
  fixture((options) => {
    writeFileSync(join(options.source, "src/terminal.zig"), "local edit\n");
    const prepared = prepareGhosttySource(options);
    assert.equal(
      readFileSync(join(prepared, "src/terminal.zig"), "utf8"),
      "fixed\n",
    );
    assert.equal(
      readFileSync(join(options.source, "src/terminal.zig"), "utf8"),
      "local edit\n",
    );
    assert.equal(
      prepareGhosttySource({ ...options, source: prepared }),
      prepared,
    );
  });
});

test("changing a patch rebuilds from the pinned base through a nested wrapper", () => {
  fixture((options, patch) => {
    const first = prepareGhosttySource(options);
    patch("second fix");
    const next = prepareGhosttySource({ ...options, source: first });
    assert.notEqual(next, first);
    assert.equal(
      readFileSync(join(next, "src/terminal.zig"), "utf8"),
      "second fix\n",
    );
    assert.equal(
      readFileSync(join(first, "src/terminal.zig"), "utf8"),
      "fixed\n",
    );
  });
});

test("CRLF patches from a Windows checkout apply to the archived LF source", () => {
  fixture((options) => {
    const patch = join(options.patches, "001-test.patch");
    const original = readFileSync(patch, "utf8");
    writeFileSync(patch, original.replace(/\n/g, "\r\n"));
    const prepared = prepareGhosttySource(options);
    assert.equal(readFileSync(join(prepared, "src/terminal.zig"), "utf8"), "fixed\n");
    writeFileSync(patch, original);
    assert.equal(prepareGhosttySource(options), prepared);
  });
});

test("a modified prepared source fails instead of silently compiling different code", () => {
  fixture((options) => {
    const prepared = prepareGhosttySource(options);
    writeFileSync(join(prepared, "src/terminal.zig"), "unexpected\n");
    assert.throws(() => prepareGhosttySource(options), /source was modified/);
  });
});

test("a patch that no longer applies fails before exposing a build directory", () => {
  fixture((options) => {
    writeFileSync(
      join(options.patches, "001-test.patch"),
      "--- a/src/terminal.zig\n+++ b/src/terminal.zig\n@@ -1 +1 @@\n-missing\n+fixed\n",
    );
    assert.throws(
      () => prepareGhosttySource(options),
      /patch.*does not apply|patch failed/,
    );
  });
});


test("Windows ARM uses the pinned x64 compiler without changing the build target", async () => {
  const { zigArchiveKey } = await import("../with-ghostty.mjs");
  assert.equal(zigArchiveKey("win32", "arm64"), "win32-x64");
  assert.equal(zigArchiveKey("win32", "x64"), "win32-x64");
  assert.equal(zigArchiveKey("darwin", "arm64"), "darwin-arm64");
});

test("GitHub steps receive the prepared Zig library as well as Ghostty", async () => {
  const { exportGitHubEnvironment } = await import("../with-ghostty.mjs");
  const temp = mkdtempSync(join(tmpdir(), "newport-ghostty-env-"));
  try {
    const destination = { GITHUB_ENV: join(temp, "env"), GITHUB_PATH: join(temp, "path") };
    exportGitHubEnvironment({
      GHOSTTY_SOURCE_DIR: "prepared-source", LIBGHOSTTY_VT_SYS_OPTIMIZE: "ReleaseSafe",
      GIT_CEILING_DIRECTORIES: "ceiling", ZIG_LIB_DIR: "prepared-stdlib",
      PATH: "pinned-zig", CARGO_BUILD_TARGET: "aarch64-pc-windows-msvc",
      RUSTUP_TOOLCHAIN: "1.98.0-x86_64-pc-windows-msvc",
    }, destination);
    const values = readFileSync(destination.GITHUB_ENV, "utf8");
    assert.match(values, /^ZIG_LIB_DIR=prepared-stdlib$/m);
    assert.match(values, /^GHOSTTY_SOURCE_DIR=prepared-source$/m);
    assert.equal(readFileSync(destination.GITHUB_PATH, "utf8"), "pinned-zig\n");
    assert.match(values, /^CARGO_BUILD_TARGET=aarch64-pc-windows-msvc$/m);
    assert.match(values, /^RUSTUP_TOOLCHAIN=1.98.0-x86_64-pc-windows-msvc$/m);
  } finally { rmSync(temp, { recursive: true, force: true }); }
});
