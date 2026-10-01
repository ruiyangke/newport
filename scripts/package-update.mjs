import {
  readFileSync,
  mkdirSync,
  writeFileSync,
  mkdtempSync,
  rmSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve, basename } from "node:path";
import { createHash } from "node:crypto";
import { parseArgs } from "node:util";
import { root, json, run, capture, plist, signerEnv } from "./tooling.mjs";
const { values, positionals } = parseArgs({
  allowPositionals: true,
  options: { output: { type: "string" } },
});
if (positionals.length !== 1 || !values.output)
  throw new Error("Supply app and --output");
const app = resolve(positionals[0]),
  output = resolve(values.output),
  info = plist(join(app, "Contents/Info.plist"));
const version = info.CFBundleShortVersionString,
  config = json(join(root, "src-tauri/tauri.conf.json"));
if (version !== config.version || info.CFBundleIdentifier !== config.identifier)
  throw new Error("App version/identifier does not match the release source");
run("codesign", ["--verify", "--deep", "--strict", app]);
run("xcrun", ["stapler", "validate", app]);
run("spctl", ["--assess", "--type", "execute", app]);
if (
  capture("lipo", [
    "-archs",
    join(app, "Contents/MacOS", info.CFBundleExecutable),
  ]) !== "arm64"
)
  throw new Error(
    "This release pipeline currently packages Apple Silicon only",
  );
const env = signerEnv();
mkdirSync(output, { recursive: true });
const name = `Newport-${version}-macos-arm64`,
  archive = join(output, `${name}.app.tar.gz`),
  zip = join(output, `${name}.zip`);
const temp = mkdtempSync(join(tmpdir(), "newport-package-"));
try {
  // ditto preserves bundle permissions, symlinks, and the stapled ticket.
  run("ditto", [app, join(temp, "Newport.app")]);
  // AppleDouble sidecars can be mistaken for the app root by the updater.
  run("tar", ["-czf", archive, "-C", temp, "Newport.app"], {
    env: { ...process.env, COPYFILE_DISABLE: "1" },
  });
} finally {
  rmSync(temp, { recursive: true, force: true });
}
run(
  process.execPath,
  [
    join(root, "node_modules/@tauri-apps/cli/tauri.js"),
    "signer",
    "sign",
    "--app-version",
    version,
    archive,
  ],
  { env },
);
run("ditto", ["-c", "-k", "--sequesterRsrc", "--keepParent", app, zip]);
const manifest = {
  version,
  notes: `Newport ${version}`,
  pub_date: new Date().toISOString(),
  platforms: {
    "darwin-aarch64": {
      signature: readFileSync(`${archive}.sig`, "utf8").trim(),
      url: `https://github.com/ruiyangke/newport/releases/download/v${version}/${basename(archive)}`,
    },
  },
};
writeFileSync(
  join(output, "latest.json"),
  `${JSON.stringify(manifest, null, 2)}\n`,
);
writeFileSync(
  join(output, "SHA256SUMS"),
  [archive, zip]
    .map(
      (path) =>
        `${createHash("sha256").update(readFileSync(path)).digest("hex")}  ${basename(path)}\n`,
    )
    .join(""),
);
console.log(`Release artifacts: ${output}`);
