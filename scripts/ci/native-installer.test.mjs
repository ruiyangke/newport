import { test } from "node:test";
import assert from "node:assert/strict";
import {
  mkdtempSync,
  mkdirSync,
  writeFileSync,
  readFileSync,
  cpSync,
  rmSync,
  symlinkSync,
  linkSync,
  chmodSync,
  readdirSync,
} from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { spawnSync } from "node:child_process";
import {
  inspectPayload,
  installerSource,
  buildWindowsInstaller,
  verifyPe,
} from "../package-windows-native.mjs";
import { root } from "../tooling.mjs";

function pe(arch, dll = false) {
  const bytes = Buffer.alloc(256);
  bytes.write("MZ");
  bytes.writeUInt32LE(64, 0x3c);
  bytes.writeUInt32LE(0x4550, 64);
  bytes.writeUInt16LE(arch === "x86_64" ? 0x8664 : 0xaa64, 68);
  bytes.writeUInt16LE(2 | (dll ? 0x2000 : 0), 86);
  bytes.writeUInt16LE(0x20b, 88);
  return bytes;
}
function fixture(arch = "x86_64") {
  const directory = mkdtempSync(join(tmpdir(), "newport-installer-test-")),
    app = join(directory, "Newport");
  mkdirSync(join(app, "agents"), { recursive: true });
  for (const arch of ["x86_64", "aarch64"])
    writeFileSync(join(app, `agents/newport-agent-${arch}.gz`), "Fixture payload");
  mkdirSync(join(app, "grammars"), { recursive: true });
  mkdirSync(join(app, "pdfium"), { recursive: true });
  writeFileSync(join(app, "pdfium/pdfium.dll"), pe(arch, true));
  writeFileSync(join(app, "pdfium/LICENSE-PDFIUM"), "Fixture license");
  cpSync(join(root, "resources/fonts"), join(app, "Fonts"), {
    recursive: true,
  });
  cpSync(join(root, "resources/licenses"), join(app, "Licenses"), {
    recursive: true,
  });
  cpSync(join(root, "resources/icons/icon.ico"), join(app, "icon.ico"));
  writeFileSync(join(app, "LICENSE-GPUI-KIT"), "Fixture license");
  writeFileSync(join(app, "newport.exe"), pe(arch));
  writeFileSync(join(app, "grammars/newport_grammars.dll"), pe(arch, true));
  writeFileSync(
    join(app, "newport-package.json"),
    JSON.stringify({
      productName: "Newport",
      identifier: "app.newport",
      version: "1.2.3",
      target: `windows-${arch}`,
    }),
  );
  return { directory, app };
}

test("native installer rejects mixed architecture, data files, symlinks and modified metadata", () => {
  const { directory, app } = fixture();
  try {
    const baseline = inspectPayload(app, "x86_64");
    assert.equal(baseline.version, "1.2.3");
    assert.throws(() => inspectPayload(app, "aarch64"), /target/);
    writeFileSync(
      join(app, "grammars/newport_grammars.dll"),
      pe("aarch64", true),
    );
    assert.throws(() => inspectPayload(app, "x86_64"), /architecture/);
    writeFileSync(
      join(app, "grammars/newport_grammars.dll"),
      pe("x86_64", true),
    );
    writeFileSync(
      join(app, "profiles.stronghold"),
      "Must never package user data",
    );
    assert.throws(
      () => inspectPayload(app, "x86_64"),
      /Unexpected package entry/,
    );
    rmSync(join(app, "profiles.stronghold"));
    const metadata = readFileSync(join(app, "newport-package.json"));
    writeFileSync(
      join(app, "newport-package.json"),
      JSON.stringify({
        productName: "Newport",
        identifier: "app.newport",
        version: '1.2.3"\n!system bad',
        target: "windows-x86_64",
      }),
    );
    assert.throws(
      () => inspectPayload(app, "x86_64"),
      /numeric release version/,
    );
    writeFileSync(join(app, "newport-package.json"), metadata);
    writeFileSync(join(app, "newport.exe"), Buffer.from("not a PE"));
    assert.throws(() => inspectPayload(app, "x86_64"), /Invalid PE/);
    writeFileSync(join(app, "newport.exe"), pe("x86_64"));
    if (process.platform !== "win32") {
      rmSync(join(app, "newport.exe"));
      symlinkSync("icon.ico", join(app, "newport.exe"));
      assert.throws(() => inspectPayload(app, "x86_64"), /symlinks/);
    }
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("installer requires both Needle resources and fingerprints their contents", () => {
  const { directory, app } = fixture();
  try {
    const baseline = inspectPayload(app, "x86_64").fingerprint;
    for (const arch of ["x86_64", "aarch64"]) {
      const path = join(app, `agents/newport-agent-${arch}.gz`);
      const original = readFileSync(path);
      rmSync(path);
      assert.throws(() => inspectPayload(app, "x86_64"), /Missing package file: agents/);
      writeFileSync(path, "different payload");
      assert.notEqual(inspectPayload(app, "x86_64").fingerprint, baseline);
      writeFileSync(path, original);
    }
  } finally { rmSync(directory, { recursive: true, force: true }); }
});

test("installer owns every staged license and preserves unknown files on uninstall", () => {
  const { directory, app } = fixture();
  try {
    const payload = inspectPayload(app, "x86_64");
    const notices = payload.entries.filter(({ path }) =>
      path.startsWith("Licenses/"),
    );
    const expected = readdirSync(join(root, "resources/licenses"), {
      recursive: true,
      withFileTypes: true,
    }).filter((entry) => entry.isFile());
    assert.equal(notices.length, expected.length);
    assert.ok(notices.some(({ path }) => path.endsWith("LICENSE_unicode")));
    const source = installerSource(payload, join(directory, "setup.exe"));
    const removal = source
      .split("!macro RemovePayload\n")[1]
      .split("!macroend")[0];
    for (const { path } of notices)
      assert.ok(
        removal.includes(`Delete "$INSTDIR\\${path.replaceAll("/", "\\")}"`),
      );
    assert.ok(
      removal.indexOf('RMDir "$INSTDIR\\Licenses\\terminal\\uucode\\licenses"') <
        removal.indexOf('RMDir "$INSTDIR\\Licenses"'),
    );
    assert.doesNotMatch(removal, /RMDir\s+\/r/i);
    const notice = join(app, "Licenses/terminal/LICENSE-Ghostty");
    const bytes = readFileSync(notice);
    rmSync(notice);
    assert.throws(() => inspectPayload(app, "x86_64"), /Missing package file/);
    writeFileSync(notice, bytes);
    writeFileSync(
      join(app, "Licenses/terminal/personal.txt"),
      "Not owned by Newport",
    );
    assert.throws(() => inspectPayload(app, "x86_64"), /Unexpected package entry/);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("PE validation handles truncated offsets and executable/library confusion", () => {
  const bytes = pe("x86_64");
  assert.throws(() => verifyPe(bytes, "x86_64", true), /type/);
  bytes.writeUInt32LE(0xffffffff, 0x3c);
  assert.throws(() => verifyPe(bytes, "x86_64"), /architecture/);
  assert.throws(() => verifyPe(Buffer.alloc(0), "x86_64"), /Invalid PE/);
});

test("installer output cannot replace the input application, including through a directory alias", () => {
  const { directory, app } = fixture();
  try {
    const binary = readFileSync(join(app, "newport.exe"));
    const outputs = [join(app, "newport.exe"), join(app, "nested/setup.exe")];
    if (process.platform !== "win32") {
      symlinkSync(app, join(directory, "alias"));
      outputs.push(join(directory, "alias/nested/setup.exe"));
    }
    for (const output of outputs)
      assert.throws(
        () =>
          buildWindowsInstaller({
            app,
            arch: "x86_64",
            output,
            compiler: "must-not-run",
          }),
        /outside the staged application/,
      );
    assert.deepEqual(readFileSync(join(app, "newport.exe")), binary);
    inspectPayload(app, "x86_64");
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test(
  "compiler reads a snapshot and a failed build preserves the previous installer",
  { skip: process.platform === "win32" },
  () => {
    const { directory, app } = fixture();
    try {
      const compiler = join(directory, "compiler.mjs");
      const output = join(directory, "setup.exe");
      writeFileSync(output, "previous installer");
      // Simulate another build replacing the staged executable after inspection.
      // The compiler must still see the original bytes in its private snapshot.
      writeFileSync(
        compiler,
        `#!/usr/bin/env node
import { readFileSync, writeFileSync } from 'node:fs';
import assert from 'node:assert/strict';
const source = readFileSync(process.argv.at(-1), 'utf8');
writeFileSync(${JSON.stringify(join(app, "newport.exe"))}, 'concurrent rebuild');
const executable = source.match(/File "([^"\\n]*newport\\.exe)"/)[1];
assert.notEqual(executable, ${JSON.stringify(join(app, "newport.exe"))});
assert.equal(readFileSync(executable).toString('ascii', 0, 2), 'MZ');
writeFileSync(source.match(/!define OUTPUT "([^"\\n]+)"/)[1], 'partial installer');
writeFileSync(${JSON.stringify(join(directory, "snapshot-verified"))}, 'ok');
process.exit(1);
`,
      );
      chmodSync(compiler, 0o755);
      assert.throws(() =>
        buildWindowsInstaller({ app, arch: "x86_64", output, compiler }),
      );
      assert.equal(readFileSync(output, "utf8"), "previous installer");
      assert.equal(
        readFileSync(join(directory, "snapshot-verified"), "utf8"),
        "ok",
      );
      assert.equal(
        readdirSync(directory).some((name) =>
          name.startsWith(".newport-nsis-"),
        ),
        false,
      );
    } finally {
      rmSync(directory, { recursive: true, force: true });
    }
  },
);

const compiler = process.env.NEWPORT_NSIS_COMPILER || "makensis";
const available =
  spawnSync(
    compiler,
    [process.platform === "win32" ? "/VERSION" : "-VERSION"],
    { encoding: "utf8" },
  ).status === 0;
test(
  "compile real NSIS installers for both payload architectures without executing them",
  { skip: !available },
  () => {
    for (const arch of ["x86_64", "aarch64"]) {
      const { directory, app } = fixture(arch);
      try {
        const binary = readFileSync(join(app, "newport.exe"));
        const destination = join(directory, "Newport setup.exe");
        linkSync(join(app, "newport.exe"), destination);
        const output = buildWindowsInstaller({
          app,
          arch,
          output: destination,
          compiler,
        });
        const bytes = readFileSync(output);
        assert.equal(bytes.toString("ascii", 0, 2), "MZ");
        assert.ok(bytes.length > 100000);
        assert.deepEqual(readFileSync(join(app, "newport.exe")), binary);
      } finally {
        rmSync(directory, { recursive: true, force: true });
      }
    }
  },
);
