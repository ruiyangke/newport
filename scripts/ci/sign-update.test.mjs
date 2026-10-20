import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, writeFileSync, rmSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { signerEnv, releaseBuildEnv } from "../tooling.mjs";

test("release Cargo builds preserve target routing without inheriting signing and notarization secrets", () => {
  const routing = {
    CARGO_BUILD_TARGET: "aarch64-pc-windows-msvc",
    CARGO_TARGET_DIR: "/build",
    PATH: "/bin",
  };
  const secrets = {
    APPLE_PASSWORD: "notary-password",
    APPLE_CERTIFICATE: "certificate",
    APPLE_CERTIFICATE_PASSWORD: "certificate-password",
    NEWPORT_SIGNING_PRIVATE_KEY: "key",
    NEWPORT_SIGNING_PRIVATE_KEY_PATH: "/key",
    NEWPORT_SIGNING_PRIVATE_KEY_PASSWORD: "password",
  };
  const source = { ...routing, ...secrets };
  assert.deepEqual(releaseBuildEnv(source), routing);
  assert.deepEqual(source, { ...routing, ...secrets });
});

test("native signer passes inline keys and defaults an empty password", () => {
  const result = signerEnv({
    NEWPORT_SIGNING_PRIVATE_KEY: "fixture-base64",
    NEWPORT_SIGNING_PRIVATE_KEY_PASSWORD: "fixture-password",
    PATH: "/bin",
  });
  assert.equal(result.NEWPORT_SIGNING_PRIVATE_KEY, "fixture-base64");
  assert.equal(result.NEWPORT_SIGNING_PRIVATE_KEY_PASSWORD, "fixture-password");
  assert.equal(result.NEWPORT_SIGNING_PRIVATE_KEY_PATH, undefined);
  assert.equal(result.PATH, "/bin");
  assert.equal(
    signerEnv({ NEWPORT_SIGNING_PRIVATE_KEY: "new" })
      .NEWPORT_SIGNING_PRIVATE_KEY_PASSWORD,
    "",
  );
  assert.throws(() => signerEnv({}), /required/);
  assert.throws(
    () =>
      signerEnv({
        NEWPORT_SIGNING_PRIVATE_KEY: "contents",
        NEWPORT_SIGNING_PRIVATE_KEY_PATH: "/key",
      }),
    /not both/,
  );
});

test("key paths are normalized without reading their contents into child environment variables", () => {
  const directory = mkdtempSync(join(tmpdir(), "newport-sign-env-"));
  try {
    const path = join(directory, "secret.key");
    writeFileSync(path, "fixture-key-contents");
    for (const source of [
      { NEWPORT_SIGNING_PRIVATE_KEY: path },
      { NEWPORT_SIGNING_PRIVATE_KEY_PATH: path },
    ]) {
      const result = signerEnv(source);
      assert.equal(result.NEWPORT_SIGNING_PRIVATE_KEY_PATH, path);
      assert.equal(result.NEWPORT_SIGNING_PRIVATE_KEY, undefined);
      assert.equal(
        Object.values(result).includes("fixture-key-contents"),
        false,
      );
    }
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
