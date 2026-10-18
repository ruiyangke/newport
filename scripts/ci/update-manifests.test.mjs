import { test } from "node:test";
import assert from "node:assert/strict";
import {
  mkdtempSync,
  writeFileSync,
  readFileSync,
  copyFileSync,
  rmSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { merge, verifyPackages, writeChecksums } from "../merge-update-manifests.mjs";
import { root, json } from "../tooling.mjs";
function fixture(fn) {
  const temp = mkdtempSync(join(tmpdir(), "newport-manifests-"));
  try {
    fn(temp);
  } finally {
    rmSync(temp, { recursive: true, force: true });
  }
}
for (const scenario of [
  "valid",
  "missing platform",
  "mixed versions",
  "duplicate platform",
  "missing payload",
  "mismatched signature",
])
  test(scenario, () =>
    fixture((temp) => {
      const paths = ["darwin-aarch64", "windows-x86_64", "windows-aarch64"].map(
        (target) => {
          const name = `${target}.package`,
            path = join(temp, `${target}.json`);
          writeFileSync(join(temp, name), "package");
          writeFileSync(join(temp, `${name}.sig`), "test signature");
          writeFileSync(
            path,
            JSON.stringify({
              version: "1.2.3",
              pub_date: "2026-01-01T00:00:00Z",
              platforms: {
                [target]: {
                  signature: "test signature",
                  url: `https://github.com/ruiyangke/newport/releases/download/v1.2.3/${name}`,
                },
              },
            }),
          );
          return path;
        },
      );
      if (scenario === "valid") {
        assert.equal(
          Object.keys(merge(paths, "1.2.3", temp).platforms).length,
          3,
        );
        return;
      }
      if (scenario === "missing platform") paths.pop();
      if (scenario === "duplicate platform") paths.push(paths[0]);
      if (scenario === "missing payload")
        rmSync(join(temp, "windows-aarch64.package"));
      if (scenario === "mismatched signature")
        writeFileSync(join(temp, "windows-x86_64.package.sig"), "wrong");
      assert.throws(() =>
        merge(paths, scenario === "mixed versions" ? "1.2.4" : "1.2.3", temp),
      );
    }),
  );
const minisign = spawnSync("minisign", ["-v"]).status === 0;
for (const scenario of [
  "valid signature",
  "modified package",
  "wrong version",
  "wrong key",
])
  test(scenario, { skip: !minisign }, () =>
    fixture((temp) => {
      const source = join(root, "resources/test-fixtures/updater");
      copyFileSync(join(source, "payload.txt"), join(temp, "payload.txt"));
      let key = readFileSync(join(source, "public.key"), "utf8").trim();
      const manifest = {
        version: "2.0.0",
        platforms: {
          "windows-aarch64": {
            url: "https://github.com/ruiyangke/newport/releases/download/v2.0.0/payload.txt",
            signature: readFileSync(
              join(source, "payload.txt.sig"),
              "utf8",
            ).trim(),
          },
        },
      };
      if (scenario === "modified package")
        writeFileSync(join(temp, "payload.txt"), "tampered");
      if (scenario === "wrong version") manifest.version = "3.0.0";
      if (scenario === "wrong key")
        key = json(join(root, "resources/updater.json")).pubkey;
      if (scenario === "valid signature") verifyPackages(manifest, temp, key);
      else assert.throws(() => verifyPackages(manifest, temp, key));
    }),
  );

test("combined checksums cover every release target and exclude staging files", () =>
  fixture((temp) => {
    const manifest = { version: "1.2.3", platforms: {} };
    const contents = new Map([
      ["latest.json", "combined manifest"],
      ["Newport-1.2.3-macos-arm64.zip", "download archive"],
    ]);
    for (const target of ["darwin-aarch64", "windows-x86_64", "windows-aarch64"]) {
      const name = `${target}.package`;
      manifest.platforms[target] = { url: `https://github.com/ruiyangke/newport/releases/download/v1.2.3/${name}` };
      contents.set(name, `${target} payload`);
      contents.set(`${name}.sig`, `${target} signature`);
    }
    for (const [name, value] of contents) writeFileSync(join(temp, name), value);
    for (const name of ["private.key", "windows-x86_64.json", "SHA256SUMS-windows-x86_64", "SHA256SUMS"])
      writeFileSync(join(temp, name), "staging only");
    writeChecksums(manifest, temp);
    const expected = [...contents]
      .map(([name, content]) => `${createHash("sha256").update(content).digest("hex")}  ${name}\n`);
    assert.deepEqual(readFileSync(join(temp, "SHA256SUMS"), "utf8").trim().split("\n").sort(), expected.map(line => line.trim()).sort());
    rmSync(join(temp, "windows-x86_64.package"));
    assert.throws(() => writeChecksums(manifest, temp), /ENOENT/);
  }),
);
