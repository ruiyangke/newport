import { test } from "node:test";
import assert from "node:assert/strict";
import { prepareMetal } from "./prepare-metal.mjs";

test("bundled Metal works without an unsupported component download", () => {
  prepareMetal(() => true, () => assert.fail("should not download"));
});
test("missing Metal is downloaded and must work before building", () => {
  let installed = false;
  prepareMetal(() => installed, () => { installed = true; });
  assert.ok(installed);
  assert.throws(() => prepareMetal(() => false, () => {}), /cannot run/);
  assert.throws(() => prepareMetal(() => false, () => { throw new Error("download failed"); }), /download failed/);
});
