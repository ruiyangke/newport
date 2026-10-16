import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { prepareGhosttySource } from "../prepare-ghostty-source.mjs";

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
