import { spawnSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { root, isMain } from "./tooling.mjs";

export function readBuildInfo(binary, { os, arch, debug } = {}) {
  const result = spawnSync(binary, ["--build-info"], {
    encoding: "utf8",
    timeout: 30000,
    maxBuffer: 4096,
  });
  if (result.error || result.status !== 0)
    throw new Error(
      "Cannot read native binary build metadata; rebuild before packaging",
    );
  const info = JSON.parse(result.stdout);
  const product = JSON.parse(
    readFileSync(join(root, "resources/app.json"), "utf8"),
  );
  if (
    info.schema !== 1 ||
    info.productName !== product.productName ||
    info.identifier !== product.identifier ||
    info.version !== product.version ||
    (os !== undefined && info.os !== os) ||
    (arch !== undefined && info.arch !== arch) ||
    (debug !== undefined && info.debugAssertions !== debug)
  )
    throw new Error(
      "The native binary does not match the requested package; rebuild before packaging",
    );
  return info;
}

export function macInfoPlist(info) {
  const xml = (value) =>
    String(value)
      .replaceAll("&", "&amp;")
      .replaceAll("<", "&lt;")
      .replaceAll(">", "&gt;")
      .replaceAll('"', "&quot;")
      .replaceAll("'", "&apos;");
  return `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>${xml(info.identifier)}</string>
<key>CFBundleExecutable</key><string>newport</string>
<key>CFBundleName</key><string>${xml(info.productName)}</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleIconFile</key><string>icon.icns</string>
<key>CFBundleVersion</key><string>${xml(info.version)}</string>
<key>CFBundleShortVersionString</key><string>${xml(info.version)}</string>
<key>ATSApplicationFontsPath</key><string>Fonts/</string>
<key>NSHighResolutionCapable</key><true/>
<key>LSMinimumSystemVersion</key><string>14.0</string>
</dict></plist>`;
}

if (isMain(import.meta.url)) {
  const [binary, output, ...extra] = process.argv.slice(2);
  if (!binary || extra.length)
    throw new Error("Supply native binary and optional output Info.plist");
  const plist = macInfoPlist(readBuildInfo(binary, { os: "macos" }));
  if (output) writeFileSync(output, plist);
  else process.stdout.write(plist);
}
