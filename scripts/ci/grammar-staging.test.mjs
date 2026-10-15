import { test } from "node:test";
import assert from "node:assert/strict";
import {
  mkdtempSync,
  mkdirSync,
  writeFileSync,
  copyFileSync,
  readFileSync,
  rmSync,
  existsSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, delimiter } from "node:path";
import { spawnSync } from "node:child_process";
import { root } from "../tooling.mjs";

test(
  "grammar staging follows Cargo's environment target and explicit override",
  { skip: process.platform === "win32" },
  () => {
    const directory = mkdtempSync(join(tmpdir(), "newport-grammar-staging-"));
    try {
      mkdirSync(join(directory, "scripts"));
      mkdirSync(join(directory, "bin"));
      copyFileSync(
        join(root, "scripts/build-grammars.mjs"),
        join(directory, "scripts/build-grammars.mjs"),
      );
      const metadata = join(directory, "metadata.json");
      writeFileSync(
        metadata,
        JSON.stringify({ target_directory: join(directory, "target") }),
      );
      // Only replace Cargo discovery/build for this filesystem routing regression;
      // the real staging script runs unchanged and copies the expected artifact.
      writeFileSync(
        join(directory, "bin/cargo"),
        '#!/bin/sh\nif [ "$1" = metadata ]; then cat "$NEWPORT_TEST_METADATA"; else test "$1" = build; fi\n',
        { mode: 0o700 },
      );
      for (const [target, name, args] of [
        ["aarch64-pc-windows-msvc", "newport_grammars.dll", []],
        [
          "aarch64-apple-darwin",
          "libnewport_grammars.dylib",
          ["--target", "aarch64-apple-darwin"],
        ],
      ]) {
        const output = join(directory, "target", target, "debug");
        mkdirSync(output, { recursive: true });
        writeFileSync(join(output, name), `Fixture for ${target}`);
        const result = spawnSync(
          process.execPath,
          [join(directory, "scripts/build-grammars.mjs"), ...args],
          {
            cwd: directory,
            encoding: "utf8",
            env: {
              ...process.env,
              PATH: join(directory, "bin") + delimiter + process.env.PATH,
              NEWPORT_TEST_METADATA: metadata,
              CARGO_BUILD_TARGET: "aarch64-pc-windows-msvc",
            },
          },
        );
        assert.equal(result.status, 0, result.stderr);
        assert.equal(
          readFileSync(join(output, "grammars", name), "utf8"),
          `Fixture for ${target}`,
        );
      }
      assert.equal(existsSync(join(directory, "target/debug/grammars")), false);
    } finally {
      rmSync(directory, { recursive: true, force: true });
    }
  },
);
