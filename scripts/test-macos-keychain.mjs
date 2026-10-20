// Opt-in protected-Keychain acceptance. Only uniquely named fixture entries are
// touched; this neither launches Newport nor reads an installed user profile.
import { cpSync, mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { root, run, capture, releaseBuildEnv } from "./tooling.mjs";

if (process.platform !== "darwin")
  throw new Error("Protected-Keychain acceptance requires macOS");
const required = (name) => {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is required`);
  return value;
};
const identity = required("APPLE_SIGNING_IDENTITY");
if (!identity.startsWith("Developer ID Application:"))
  throw new Error("Provide an explicit Developer ID Application identity");
const profile = resolve(required("NEWPORT_PROVISIONING_PROFILE"));
const entitlements = resolve(required("NEWPORT_SIGNING_ENTITLEMENTS"));
const env = releaseBuildEnv();
run(
  process.execPath,
  [join(root, "scripts/check-signing.mjs"), entitlements, profile],
  { env },
);
const output = capture(
  "cargo",
  [
    "test",
    "--locked",
    "-p",
    "newport-platform",
    "--lib",
    "--no-run",
    "--message-format=json",
  ],
  { env, maxBuffer: 32 * 1024 * 1024 },
);
const artifacts = output
  .split("\n")
  .filter(Boolean)
  .map((line) => JSON.parse(line));
const executables = artifacts.filter(
  (item) =>
    item.reason === "compiler-artifact" &&
    item.target?.name === "newport_platform" &&
    item.profile?.test &&
    item.executable,
);
if (executables.length !== 1)
  throw new Error(
    "Cargo must return exactly one platform library test executable",
  );
const directory = mkdtempSync(join(tmpdir(), "newport-keychain-acceptance-"));
const app = join(directory, "NewportKeychainTest.app");
const bin = join(app, "Contents/MacOS/keychain-test");
mkdirSync(join(app, "Contents/MacOS"), { recursive: true });
cpSync(executables[0].executable, bin);
cpSync(profile, join(app, "Contents/embedded.provisionprofile"));
writeFileSync(
  join(app, "Contents/Info.plist"),
  `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>app.newport</string>
<key>CFBundleExecutable</key><string>keychain-test</string>
<key>CFBundleName</key><string>NewportKeychainTest</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleVersion</key><string>1</string>
</dict></plist>
`,
);
// Local test only: no network timestamp, notary submission or installation.
run(
  "codesign",
  [
    "--force",
    "--sign",
    identity,
    "--options",
    "runtime",
    "--timestamp=none",
    "--entitlements",
    entitlements,
    app,
  ],
  { env },
);
run("codesign", ["--verify", "--deep", "--strict", app], { env });
console.log(`Protected-Keychain test bundle: ${app}`);
run(
  bin,
  [
    "--exact",
    "secrets::macos::tests::protected_keychain_stores_and_isolates_profile_keys",
    "--ignored",
    "--nocapture",
  ],
  { env, timeout: 60_000 },
);
