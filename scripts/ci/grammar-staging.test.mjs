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
  readdirSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, delimiter } from "node:path";
import { spawnSync } from "node:child_process";
import { root } from "../tooling.mjs";
import { grammarLanguages, grammarFiles } from "../grammar-resources.mjs";

test(
  "grammar staging follows Cargo's environment target and explicit override",
  { skip: process.platform === "win32" },
  () => {
    const directory = mkdtempSync(join(tmpdir(), "newport-grammar-staging-"));
    try {
      mkdirSync(join(directory, "scripts"));
      mkdirSync(join(directory, "bin"));
      mkdirSync(join(directory, "resources"));
      copyFileSync(join(root, "scripts/grammar-resources.mjs"), join(directory, "scripts/grammar-resources.mjs"));
      copyFileSync(join(root, "resources/grammar-languages.json"), join(directory, "resources/grammar-languages.json"));
      for (const command of ["install_name_tool", "codesign"])
        writeFileSync(join(directory, "bin", command), "#!/bin/sh\nexit 0\n", { mode: 0o700 });
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
        '#!/bin/sh\ncase "$*" in *lang-zig*) if [ "$NEWPORT_TEST_FAIL" = 1 ]; then exit 88; fi;; esac\nif [ "$1" = metadata ]; then cat "$NEWPORT_TEST_METADATA"; else test "$1" = build; fi\n',
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
        const platform = target.includes("windows") ? "win32" : "darwin";
        const catalog = { schema: 1, grammars: grammarLanguages.map(name => ({ name })) };
        writeFileSync(join(output, platform === "win32" ? "newport-grammar-catalog.exe" : "newport-grammar-catalog"),
          `#!/usr/bin/env node\nconsole.log(${JSON.stringify(JSON.stringify(catalog))});\n`, { mode: 0o700 });
        mkdirSync(join(output, "grammars"));
        writeFileSync(join(output, "grammars", name), "stale monolith");
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
        assert.equal(existsSync(join(output, "grammars", name)), false);
        for (const file of grammarFiles(platform)) {
          assert.equal(
            readFileSync(join(output, "grammars", file), "utf8"),
            file === "catalog.json" ? JSON.stringify(catalog) : `Fixture for ${target}`,
          );
        }
        const failed = spawnSync(process.execPath,
          [join(directory, "scripts/build-grammars.mjs"), ...args], {
            cwd: directory, encoding: "utf8",
            env: { ...process.env, PATH: join(directory, "bin") + delimiter + process.env.PATH,
              NEWPORT_TEST_METADATA: metadata, CARGO_BUILD_TARGET: "aarch64-pc-windows-msvc",
              NEWPORT_TEST_FAIL: "1" },
          });
        assert.notEqual(failed.status, 0);
        assert.match(failed.stderr, /Grammar build failed/);
        assert.equal(readFileSync(join(output, "grammars/catalog.json"), "utf8"), JSON.stringify(catalog));
        assert.equal(readdirSync(output).some(name => name.startsWith("grammars-staging-")), false);
      }
      assert.equal(existsSync(join(directory, "target/debug/grammars")), false);
    } finally {
      rmSync(directory, { recursive: true, force: true });
    }
  },
);
