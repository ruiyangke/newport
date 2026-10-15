import { test } from "node:test";
import assert from "node:assert/strict";
import {
  copyFileSync,
  cpSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import { spawnSync } from "node:child_process";
import { root } from "../tooling.mjs";

// Exercise real packaging orchestration without downloading a compiler, compiling
// an application, signing a payload, or reading an installed user profile.
function fixture(run) {
  const directory = mkdtempSync(join(tmpdir(), "newport-ghostty-package-"));
  try {
    for (const name of ["scripts", "bin", "resources"])
      mkdirSync(join(directory, name));
    for (const name of ["package-native.mjs", "tooling.mjs"])
      copyFileSync(
        join(root, "scripts", name),
        join(directory, "scripts", name),
      );
    writeFileSync(join(directory, "resources/app.json"), '{"version":"0.0.0"}');
    writeFileSync(
      join(directory, "scripts/native-build-info.mjs"),
      "export function readBuildInfo() {} export function macInfoPlist() {}",
    );
    writeFileSync(
      join(directory, "scripts/package-windows-native.mjs"),
      "export function buildWindowsInstaller() {}",
    );
    writeFileSync(
      join(directory, "scripts/stage-pdfium.mjs"),
      "export function stagePdfium() {} export function validateLibrary() {}",
    );
    const record = `import { appendFileSync } from "node:fs";
      const record = (step, env) => appendFileSync(process.env.NEWPORT_TEST_LOG,
        JSON.stringify({ step, ghostty: env.GHOSTTY_SOURCE_DIR,
          optimize: env.LIBGHOSTTY_VT_SYS_OPTIMIZE,
          secret: env.NEWPORT_SIGNING_PRIVATE_KEY,
          password: env.APPLE_PASSWORD }) + "\\n");`;
    writeFileSync(
      join(directory, "scripts/with-ghostty.mjs"),
      `${record}
       export async function ghosttyBuildEnv(source) {
         record("bootstrap", source);
         return { ...source, GHOSTTY_SOURCE_DIR: "prepared-ghostty",
           LIBGHOSTTY_VT_SYS_OPTIMIZE: "ReleaseSafe" };
       }`,
    );
    writeFileSync(
      join(directory, "scripts/build-grammars.mjs"),
      `${record}\nrecord("grammars", process.env);`,
    );
    writeFileSync(
      join(directory, "bin/cargo"),
      `#!/usr/bin/env node
       const { appendFileSync } = require("node:fs");
       appendFileSync(process.env.NEWPORT_TEST_LOG, JSON.stringify({
         step: "cargo", args: process.argv.slice(2),
         ghostty: process.env.GHOSTTY_SOURCE_DIR,
         optimize: process.env.LIBGHOSTTY_VT_SYS_OPTIMIZE,
         secret: process.env.NEWPORT_SIGNING_PRIVATE_KEY,
         password: process.env.APPLE_PASSWORD,
       }) + "\\n");
       process.stderr.write("fixture stops before packaging artifacts");
       process.exit(42);`,
      { mode: 0o700 },
    );
    const log = join(directory, "commands.jsonl");
    writeFileSync(log, "");
    run(directory, log);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}
function execute(directory, log, args) {
  const result = spawnSync(
    process.execPath,
    ["scripts/package-native.mjs", ...args],
    {
      cwd: directory,
      encoding: "utf8",
      env: {
        ...process.env,
        CARGO_BUILD_TARGET: "",
        PATH: `${join(directory, "bin")}${delimiter}${process.env.PATH}`,
        NEWPORT_TEST_LOG: log,
        NEWPORT_SIGNING_PRIVATE_KEY: "test-private-key",
        APPLE_PASSWORD: "test-notary-password",
      },
    },
  );
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /fixture stops before packaging artifacts/);
  return readFileSync(log, "utf8")
    .trim()
    .split("\n")
    .filter(Boolean)
    .map(JSON.parse);
}

test(
  "native packaging bootstraps Ghostty and propagates compiler env without signing secrets",
  {
    skip: process.platform === "win32",
  },
  () =>
    fixture((directory, log) => {
      const records = execute(directory, log, ["--debug"]);
      assert.deepEqual(
        records.map(({ step }) => step),
        ["bootstrap", "grammars", "cargo"],
      );
      for (const record of records) {
        assert.equal(record.secret, undefined);
        assert.equal(record.password, undefined);
      }
      for (const record of records.slice(1)) {
        assert.equal(record.ghostty, "prepared-ghostty");
        assert.equal(record.optimize, "ReleaseSafe");
      }
      assert.deepEqual(records[2].args, [
        "build",
        "--locked",
        "-p",
        "newport-app",
      ]);
    }),
);

test(
  "prebuilt packaging skips the compiler and source bootstrap",
  {
    skip: process.platform === "win32",
  },
  () =>
    fixture((directory, log) => {
      const records = execute(directory, log, ["--skip-build"]);
      assert.equal(records.length, 1);
      assert.equal(records[0].step, "cargo");
      assert.equal(records[0].args[0], "metadata");
    }),
);

test(
  "prebuilt packages retain terminal license notices before platform signing",
  { skip: process.platform === "win32" },
  () =>
    fixture((directory, log) => {
      const output = join(directory, "target/debug");
      for (const path of [
        join(output, "grammars"),
        join(output, "agents"),
        join(directory, "resources/fonts"),
        join(directory, "resources/icons"),
        join(directory, "crates/newport-grammars"),
      ])
        mkdirSync(path, { recursive: true });
      writeFileSync(join(output, "newport"), "fixture, never executed");
      for (const arch of ["x86_64", "aarch64"])
        writeFileSync(join(output, `agents/newport-agent-${arch}.gz`), `compressed ${arch}`);
      writeFileSync(join(directory, "resources/icons/icon.icns"), "fixture");
      writeFileSync(
        join(directory, "crates/newport-grammars/LICENSE-GPUI-KIT"),
        "fixture",
      );
      cpSync(
        join(root, "resources/licenses"),
        join(directory, "resources/licenses"),
        {
          recursive: true,
        },
      );
      writeFileSync(
        join(directory, "scripts/native-build-info.mjs"),
        'export function readBuildInfo() { throw new Error("fixture stops after staging"); } export function macInfoPlist() {}',
      );
      const metadata = {
        target_directory: join(directory, "target"),
        packages: [{ name: "newport-app", version: "0.0.0" }],
      };
      writeFileSync(
        join(directory, "bin/cargo"),
        `#!/usr/bin/env node\nconsole.log(${JSON.stringify(JSON.stringify(metadata))});`,
        { mode: 0o700 },
      );
      const result = spawnSync(
        process.execPath,
        ["scripts/package-native.mjs", "--skip-build", "--debug"],
        {
          cwd: directory,
          encoding: "utf8",
          env: {
            ...process.env,
            CARGO_BUILD_TARGET: "",
            PATH: `${join(directory, "bin")}${delimiter}${process.env.PATH}`,
            NEWPORT_TEST_LOG: log,
          },
        },
      );
      assert.notEqual(result.status, 0);
      assert.match(result.stderr, /fixture stops after staging/);
      const staged = join(
        output,
        "native-package",
        ...(process.platform === "darwin"
          ? ["Newport.app", "Contents", "Resources"]
          : ["Newport"]),
        "Licenses",
        "terminal",
      );
      for (const arch of ["x86_64", "aarch64"])
        assert.equal(readFileSync(join(staged, "../../agents", `newport-agent-${arch}.gz`), "utf8"), `compressed ${arch}`);
      const original = join(root, "resources/licenses/terminal");
      const files = readdirSync(original, {
        recursive: true,
        withFileTypes: true,
      })
        .filter((entry) => entry.isFile())
        .map((entry) =>
          join(entry.parentPath, entry.name).slice(original.length + 1),
        );
      assert.ok(files.includes("LICENSE-Ghostty"));
      assert.ok(files.includes("LICENSE-libghostty-rs"));
      assert.ok(files.includes(join("uucode", "licenses", "LICENSE_unicode")));
      for (const file of files) {
        assert.deepEqual(
          readFileSync(join(staged, file)),
          readFileSync(join(original, file)),
          file,
        );
      }
    }),
);
