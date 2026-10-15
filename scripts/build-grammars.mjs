// Build and stage the grammar library beside Cargo's app/test executables.
import { spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
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
const build = spawnSync(
  "cargo",
  ["build", "--locked", "-p", "newport-grammars", ...args],
  { cwd: root, stdio: "inherit" },
);
if (build.error) throw build.error;
if (build.status !== 0) process.exit(build.status ?? 1);
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
mkdirSync(join(output, "grammars"), { recursive: true });
copyFileSync(join(output, name), join(output, "grammars", name));
console.log(`Staged ${join(output, "grammars", name)}`);
