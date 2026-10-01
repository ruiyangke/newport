import { writeFileSync, appendFileSync } from "node:fs";
import { randomBytes } from "node:crypto";
import { join } from "node:path";
import { run, plist } from "../tooling.mjs";
for (const name of [
  "RUNNER_TEMP",
  "APPLE_CERTIFICATE",
  "APPLE_PROVISIONING_PROFILE",
  "APPLE_ENTITLEMENTS",
  "GITHUB_ENV",
])
  if (!process.env[name]) throw new Error(`Missing secret or setting: ${name}`);
if (process.env.APPLE_CERTIFICATE_PASSWORD === undefined)
  throw new Error("Missing APPLE_CERTIFICATE_PASSWORD");
const temp = process.env.RUNNER_TEMP;
const cert = join(temp, "signing.p12"),
  profile = join(temp, "release.provisionprofile"),
  ent = join(temp, "release.entitlements");
writeFileSync(cert, Buffer.from(process.env.APPLE_CERTIFICATE, "base64"), {
  mode: 0o600,
});
writeFileSync(
  profile,
  Buffer.from(process.env.APPLE_PROVISIONING_PROFILE, "base64"),
  { mode: 0o600 },
);
writeFileSync(ent, process.env.APPLE_ENTITLEMENTS, { mode: 0o600 });
if (plist(ent)["com.apple.security.get-task-allow"])
  throw new Error("Release signing must disable get-task-allow");
run(process.execPath, ["scripts/check-signing.mjs", ent, profile]);
const kc = join(temp, "release.keychain-db"),
  password = randomBytes(32).toString("base64url");
for (const args of [
  ["create-keychain", "-p", password, kc],
  ["set-keychain-settings", "-lut", "7200", kc],
  ["unlock-keychain", "-p", password, kc],
  [
    "import",
    cert,
    "-k",
    kc,
    "-P",
    process.env.APPLE_CERTIFICATE_PASSWORD,
    "-T",
    "/usr/bin/codesign",
  ],
  [
    "set-key-partition-list",
    "-S",
    "apple-tool:,apple:,codesign:",
    "-s",
    "-k",
    password,
    kc,
  ],
  ["list-keychains", "-d", "user", "-s", kc],
]) {
  try {
    run("security", args, { stdio: "ignore" });
  } catch {
    throw new Error("Signing keychain setup failed");
  }
}
appendFileSync(
  process.env.GITHUB_ENV,
  `NEWPORT_PROVISIONING_PROFILE=${profile}\nNEWPORT_SIGNING_ENTITLEMENTS=${ent}\n`,
);
