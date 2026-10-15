// Build and stage per-language grammar libraries beside Cargo's app/test executables.
import { spawnSync } from "node:child_process";
import {
  copyFileSync, mkdirSync, writeFileSync, renameSync, rmSync, mkdtempSync,
} from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { grammarLanguages, grammarLibrary } from "./grammar-resources.mjs";
const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const args = process.argv.slice(2);
if (
  args.some(
    (x, i) =>
      x !== "--release" && x !== "--target" && args[i - 1] !== "--target",
  )
) {
  throw new Error(
    "Usage: node scripts/build-grammars.mjs [--release] [--target <triple>]",
  );
}
const targetIndex = args.indexOf("--target");
const target =
  targetIndex < 0 ? process.env.CARGO_BUILD_TARGET : args[targetIndex + 1];
if (targetIndex >= 0 && (!target || target.startsWith("--")))
  throw new Error("Missing target triple");
function cargo(buildArgs) {
  const build = spawnSync(
    "cargo",
    ["build", "--locked", "-p", "newport-grammars", ...buildArgs, ...args],
    { cwd: root, stdio: "inherit" },
  );
  if (build.error) throw build.error;
  if (build.status !== 0) throw new Error(`Grammar build failed (${build.status})`);
}
const metadata = spawnSync(
  "cargo",
  ["metadata", "--no-deps", "--format-version", "1"],
  { cwd: root, encoding: "utf8" },
);
if (metadata.status !== 0) throw new Error(metadata.stderr);
const output = join(
  JSON.parse(metadata.stdout).target_directory,
  ...(target ? [target] : []),
  args.includes("--release") ? "release" : "debug",
);
const platform = target
  ? target.includes("windows")
    ? "win32"
    : target.includes("apple")
      ? "darwin"
      : "linux"
  : process.platform;
const name =
  platform === "win32"
    ? "newport_grammars.dll"
    : platform === "darwin"
      ? "libnewport_grammars.dylib"
      : "libnewport_grammars.so";
// Generate portable query metadata once. The catalog executable is build tooling
// only; it is never copied into the application.
cargo([
  "--bin", "newport-grammar-catalog", "--no-default-features", "--features", "catalog",
]);
const catalog = spawnSync(
  join(output, platform === "win32" ? "newport-grammar-catalog.exe" : "newport-grammar-catalog"),
  [],
  { encoding: "utf8", maxBuffer: 4 * 1024 * 1024 },
);
if (catalog.error || catalog.status !== 0)
  throw new Error("Cannot generate grammar catalog on this host");
const parsed = JSON.parse(catalog.stdout);
if (
  parsed.schema !== 1 ||
  JSON.stringify(parsed.grammars.map(g => g.name).sort()) !== JSON.stringify(grammarLanguages)
)
  throw new Error("Grammar catalog does not match the language inventory");
mkdirSync(output, { recursive: true });
const staging = mkdtempSync(join(output, "grammars-staging-"));
try {
  writeFileSync(join(staging, "catalog.json"), JSON.stringify(parsed));
  for (const language of grammarLanguages) {
    cargo(["--lib", "--no-default-features", "--features", `lang-${language}`]);
    const destination = join(staging, grammarLibrary(language, platform));
    copyFileSync(join(output, name), destination);
    if (platform === "darwin") {
      const result = spawnSync("install_name_tool", [
        "-id", `@rpath/${grammarLibrary(language, platform)}`, destination,
      ], { stdio: "inherit" });
      if (result.status !== 0) throw new Error("Cannot set grammar library identity");
      const signed = spawnSync("codesign", ["--force", "--sign", "-", destination], { stdio: "inherit" });
      if (signed.status !== 0) throw new Error("Cannot sign staged grammar");
    }
  }
  // Replace only after every language builds; remove stale monolithic libraries.
  const previous = join(output, "grammars-previous");
  rmSync(previous, { recursive: true, force: true });
  try {
    renameSync(join(output, "grammars"), previous);
  } catch (error) {
    if (error.code !== "ENOENT") throw error;
  }
  try {
    renameSync(staging, join(output, "grammars"));
  } catch (error) {
    try { renameSync(previous, join(output, "grammars")); } catch {}
    throw error;
  }
  rmSync(previous, { recursive: true, force: true });
} finally {
  rmSync(staging, { recursive: true, force: true });
}
console.log(`Staged ${grammarLanguages.length} language libraries in ${join(output, "grammars")}`);
