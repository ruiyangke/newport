import { root, json, capture, plist, isMain } from "./tooling.mjs";
import { join } from "node:path";
const matches = (value, pattern) =>
  new RegExp(
    `^${String(pattern)
      .replace(/[.+^$(){}|[\]\\]/g, "\\$&")
      .replace(/\*/g, ".*")
      .replace(/\?/g, ".")}$`,
  ).test(value);
export function validate(entitlements, profile, identifier) {
  const allowed = profile.Entitlements;
  const id =
    entitlements["com.apple.application-identifier"] ??
    entitlements["application-identifier"] ??
    "";
  if (!id.endsWith(`.${identifier}`))
    throw new Error(`Signing entitlements must identify ${identifier}`);
  if (
    !matches(
      id,
      allowed["com.apple.application-identifier"] ??
        allowed["application-identifier"] ??
        "",
    )
  )
    throw new Error(
      "Provisioning profile does not authorize the app identifier",
    );
  const groups = entitlements["keychain-access-groups"] ?? [];
  const permitted = allowed["keychain-access-groups"] ?? [];
  if (!groups[0]?.endsWith(`.${identifier}`))
    throw new Error("The first Keychain group must be the Newport group");
  if (!groups.some((group) => group.endsWith(".com.porthop.desktop")))
    throw new Error(
      "Retain the previous com.porthop.desktop Keychain group for credential migration",
    );
  if (
    groups.some(
      (group) => !permitted.some((pattern) => matches(group, pattern)),
    )
  )
    throw new Error(
      "Provisioning profile does not authorize all migration Keychain groups",
    );
}
export function profileEntitlements(xml) {
  // Older plutil versions cannot convert a profile containing dates/data to JSON.
  const entitlements = capture(
    "plutil",
    ["-extract", "Entitlements", "xml1", "-o", "-", "-"],
    { input: xml },
  );
  return JSON.parse(
    capture("plutil", ["-convert", "json", "-o", "-", "-"], {
      input: entitlements,
    }),
  );
}
if (isMain(import.meta.url)) {
  const profileXml = capture("security", ["cms", "-D", "-i", process.argv[3]], {
    stdio: ["ignore", "pipe", "ignore"],
  });
  const profile = { Entitlements: profileEntitlements(profileXml) };
  validate(
    plist(process.argv[2]),
    profile,
    json(join(root, "src-tauri/tauri.conf.json")).identifier,
  );
}
