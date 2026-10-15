// Prepare a native distribution locally. This does not publish or install it.
import { cpSync, mkdirSync } from "node:fs";
import { join } from "node:path";
import { parseArgs } from "node:util";
import { root, run, capture, signerEnv, releaseBuildEnv } from "./tooling.mjs";
import { readBuildInfo } from "./native-build-info.mjs";
import { notarizationAuth, submitNotarization } from "./notarize.mjs";

const { values } = parseArgs({
  options: { "skip-build": { type: "boolean" } },
});
if (process.platform !== "darwin")
  throw new Error("Native macOS distributions must be built on macOS");
const required = (name) => {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is required`);
  return value;
};
const identity = required("APPLE_SIGNING_IDENTITY");
if (!identity.startsWith("Developer ID Application:"))
  throw new Error("Distribution requires a Developer ID Application identity");
const profile = required("NEWPORT_PROVISIONING_PROFILE");
const entitlements = required("NEWPORT_SIGNING_ENTITLEMENTS");
const notaryAuth = notarizationAuth();
signerEnv(); // Fail before building if updater signing credentials are missing.
run(process.execPath, [
  join(root, "scripts/check-signing.mjs"),
  entitlements,
  profile,
]);

// Native dependencies and build scripts do not need distribution credentials.
const buildEnv = releaseBuildEnv({ ...process.env, CARGO_INCREMENTAL: "0" });
const target = "aarch64-apple-darwin";
run(
  process.execPath,
  [
    join(root, "scripts/package-native.mjs"),
    "--target",
    target,
    ...(values["skip-build"] ? ["--skip-build"] : []),
  ],
  { env: buildEnv },
);
const metadata = JSON.parse(
  capture(
    "cargo",
    ["metadata", "--locked", "--no-deps", "--format-version", "1"],
    { env: buildEnv },
  ),
);
const directory = join(
  metadata.target_directory,
  target,
  "release",
  "native-package",
);
const app = join(directory, "Newport.app");
readBuildInfo(join(app, "Contents/MacOS/newport"), {
  os: "macos",
  arch: "aarch64",
  debug: false,
});
cpSync(profile, join(app, "Contents/embedded.provisionprofile"));
// Sign nested executable code first, then the enclosing bundle. Only the host
// receives the restricted Keychain entitlements and matching profile.
run("codesign", [
  "--force",
  "--sign",
  identity,
  "--options",
  "runtime",
  "--timestamp",
  join(app, "Contents/Resources/grammars/libnewport_grammars.dylib"),
]);
run("codesign", [
  "--force",
  "--sign",
  identity,
  "--options",
  "runtime",
  "--timestamp",
  "--entitlements",
  entitlements,
  app,
]);
run("codesign", ["--verify", "--deep", "--strict", app]);
run(join(app, "Contents/MacOS/newport"), ["--check-grammars"]);

const distribution = join(directory, "distribution");
mkdirSync(distribution, { recursive: true });
const submission = join(distribution, "notarization.zip");
run("ditto", ["-c", "-k", "--sequesterRsrc", "--keepParent", app, submission]);
submitNotarization(submission, notaryAuth);
run("xcrun", ["stapler", "staple", app]);
// Revalidates notarization, Gatekeeper and architecture before signing updates.
run(process.execPath, [
  join(root, "scripts/package-update.mjs"),
  app,
  "--output",
  join(distribution, "update"),
]);
console.log(`Signed native candidate: ${join(distribution, "update")}`);
