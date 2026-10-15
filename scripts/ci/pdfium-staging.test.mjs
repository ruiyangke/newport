import { test } from "node:test";
import assert from "node:assert/strict";
import {
  mkdtempSync,
  mkdirSync,
  writeFileSync,
  readFileSync,
  rmSync,
  symlinkSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { stagePdfium, validateLibrary } from "../stage-pdfium.mjs";

test("PDFium rejects an altered archive before touching staged files", async () => {
  const root = mkdtempSync(join(tmpdir(), "pdfium-staging-test-"));
  try {
    const output = join(root, "pdfium");
    mkdirSync(output);
    writeFileSync(join(output, "pdfium.dll"), "previous library");
    writeFileSync(join(output, "LICENSE-PDFIUM"), "previous notices");
    await assert.rejects(
      stagePdfium(output, "aarch64", async () =>
        Buffer.from("altered archive"),
      ),
      /checksum mismatch/,
    );
    assert.equal(
      readFileSync(join(output, "pdfium.dll"), "utf8"),
      "previous library",
    );
    assert.equal(
      readFileSync(join(output, "LICENSE-PDFIUM"), "utf8"),
      "previous notices",
    );
    assert.throws(
      () => validateLibrary(Buffer.from("MZ"), "x86_64"),
      /pinned library/,
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("PDFium rejects unsupported targets and aliased output before download", async () => {
  const root = mkdtempSync(join(tmpdir(), "pdfium-staging-test-"));
  const download = async () => {
    assert.fail("must not download");
  };
  try {
    await assert.rejects(
      stagePdfium(join(root, "out"), "armv7", download),
      /Unsupported/,
    );
    const destination = join(root, "destination");
    mkdirSync(destination);
    if (process.platform !== "win32") {
      symlinkSync(destination, join(root, "alias"));
      await assert.rejects(
        stagePdfium(join(root, "alias"), "x86_64", download),
        /real directory/,
      );
    }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
