import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, writeFileSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { verifyArtifact } from "./artifact.mjs";

const root = fileURLToPath(new URL("../../", import.meta.url));
test("reject stale, changed, incomplete and extra artifact contents", () => {
  const directory = mkdtempSync(join(tmpdir(), "newport-artifact-"));
  const path = relative(root, directory);
  const record = () =>
    execFileSync(process.execPath, [
      join(root, "scripts/ci/artifact.mjs"),
      "record",
      path,
    ]);
  try {
    writeFileSync(join(directory, "payload"), "original");
    assert.throws(() => verifyArtifact(path));
    record();
    verifyArtifact(path);
    writeFileSync(join(directory, "payload"), "changed");
    assert.throws(() => verifyArtifact(path), /does not match/);
    writeFileSync(join(directory, "payload"), "original");
    writeFileSync(join(directory, "extra"), "unexpected");
    assert.throws(() => verifyArtifact(path), /does not match/);
    rmSync(join(directory, "extra"));
    const manifest = JSON.parse(
      readFileSync(join(directory, "newport-build.json"), "utf8"),
    );
    manifest.commit = "0".repeat(40);
    writeFileSync(
      join(directory, "newport-build.json"),
      JSON.stringify(manifest),
    );
    assert.throws(() => verifyArtifact(path), /does not match/);
    record();
    rmSync(join(directory, "payload"));
    assert.throws(() => verifyArtifact(path), /Empty artifact/);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
