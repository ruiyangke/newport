// Reproducible build prerequisites for newport-terminal. Downloads live under
// target/ghostty; no system installation or global PATH change is performed.
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import {
  appendFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  renameSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { delimiter, dirname, join } from "node:path";
import { isMain, root } from "./tooling.mjs";
import { prepareGhosttySource } from "./prepare-ghostty-source.mjs";
import { prepareZigStandardLibrary } from "./prepare-zig-stdlib.mjs";

export const GHOSTTY_REVISION = "3425025e585a3403340d4dc6d65132f42e05e605";
export const ZIG_VERSION = "0.16.0";
const archives = {
  "darwin-arm64": [
    "aarch64-macos",
    "b23d70deaa879b5c2d486ed3316f7eaa53e84acf6fc9cc747de152450d401489",
  ],
  "darwin-x64": [
    "x86_64-macos",
    "0387557ed1877bc6a2e1802c8391953baddba76081876301c522f52977b52ba7",
  ],
  "win32-arm64": [
    "aarch64-windows",
    "aee38316ee4111717900f45dd3130145c39289e105541d737eb8c5ed653c78ef",
  ],
  "win32-x64": [
    "x86_64-windows",
    "68659eb5f1e4eb1437a722f1dd889c5a322c9954607f5edcf337bc3684a75a7e",
  ],
  "linux-arm64": [
    "aarch64-linux",
    "ea4b09bfb22ec6f6c6ceac57ab63efb6b46e17ab08d21f69f3a48b38e1534f17",
  ],
  "linux-x64": [
    "x86_64-linux",
    "70e49664a74374b48b51e6f3fdfbf437f6395d42509050588bd49abe52ba3d00",
  ],
};

// The native Windows ARM64 Zig 0.16 compiler crashes in `zig build` on
// hosted ARM runners (0xc0000005). Run the pinned x64 compiler under Windows
// emulation; Ghostty still receives the native aarch64 target from Cargo.
export function zigArchiveKey(platform, arch) {
  return `${platform}-${platform === "win32" && arch === "arm64" ? "x64" : arch}`;
}

function run(command, args, env, capture = false) {
  const result = spawnSync(command, args, {
    env,
    cwd: root,
    encoding: "utf8",
    stdio: capture ? ["ignore", "pipe", "pipe"] : "inherit",
  });
  if (result.error) throw result.error;
  if (result.status !== 0)
    throw new Error(
      `${command} failed (${result.status}): ${result.stderr ?? ""}`,
    );
  return result.stdout?.trim();
}

export async function ghosttyBuildEnv(source = process.env) {
  const env = { ...source };
  const cache = join(root, "target", "ghostty");
  mkdirSync(cache, { recursive: true });
  const pathKey =
    Object.keys(env).find((key) => key.toLowerCase() === "path") ?? "PATH";
  if (process.platform === "win32") {
    const host = run("rustc", ["-vV"], env, true).match(/^host: (.+)$/m)?.[1];
    if (host === "aarch64-pc-windows-msvc") {
      // libghostty-rs emits -Dtarget only for a Rust cross build. Pair the x64
      // Zig host with an x64 Rust host so the produced library remains ARM64.
      const channel = readFileSync(join(root, "rust-toolchain.toml"), "utf8")
        .match(/^channel = "([^"]+)"$/m)?.[1];
      if (!channel) throw new Error("Missing pinned Rust toolchain");
      const toolchain = `${channel}-x86_64-pc-windows-msvc`;
      run("rustup", ["toolchain", "install", toolchain, "--profile", "minimal",
        "--component", "clippy,rustfmt", "--force-non-host"], env);
      env.RUSTUP_TOOLCHAIN = toolchain;
      env.CARGO_BUILD_TARGET ??= host;
      run("rustup", ["target", "add", env.CARGO_BUILD_TARGET, "--toolchain", toolchain], env);
    }
  }
  const available = spawnSync("zig", ["version"], { env, encoding: "utf8" });
  if (available.status !== 0 || available.stdout.trim() !== ZIG_VERSION
      || (process.platform === "win32" && process.arch === "arm64")) {
    const archive = archives[zigArchiveKey(process.platform, process.arch)];
    if (!archive)
      throw new Error(
        `No pinned Zig build for ${process.platform}-${process.arch}`,
      );
    const [platform, checksum] = archive;
    const name = `zig-${platform}-${ZIG_VERSION}`;
    const install = join(cache, name);
    const executable = join(
      install,
      process.platform === "win32" ? "zig.exe" : "zig",
    );
    if (!existsSync(executable)) {
      const staging = mkdtempSync(join(cache, "zig-download-"));
      try {
        const extension = process.platform === "win32" ? "zip" : "tar.xz";
        const response = await fetch(
          `https://ziglang.org/download/${ZIG_VERSION}/${name}.${extension}`,
          { signal: AbortSignal.timeout(300_000) },
        );
        if (!response.ok)
          throw new Error(`Zig download failed: HTTP ${response.status}`);
        const data = Buffer.from(await response.arrayBuffer());
        if (createHash("sha256").update(data).digest("hex") !== checksum)
          throw new Error("Zig archive checksum mismatch");
        const path = join(staging, `archive.${extension}`);
        writeFileSync(path, data);
        run("tar", ["-xf", path, "-C", staging], env);
        renameSync(join(staging, name), install);
      } finally {
        rmSync(staging, { recursive: true, force: true });
      }
    }
    if (run(executable, ["version"], env, true) !== ZIG_VERSION)
      throw new Error("Cached Zig version does not match the build pin");
    env[pathKey] = `${install}${delimiter}${env[pathKey] ?? ""}`;
  }
  if (process.platform === "win32") {
    // Zig 0.16 reports its environment as ZON. Its quoted lib_dir is a JSON
    // compatible string, including escaped Windows path separators.
    const lib = run("zig", ["env"], env, true).match(
      /^\s*\.lib_dir = ("(?:\\.|[^"\\])*"),$/m,
    );
    if (!lib) throw new Error("Cannot locate the pinned Zig standard library");
    env.ZIG_LIB_DIR = prepareZigStandardLibrary({
      lib: JSON.parse(lib[1]),
      cache,
    });
  }
  const sourceDir =
    env.GHOSTTY_SOURCE_DIR ?? join(cache, `source-${GHOSTTY_REVISION}`);
  if (!existsSync(sourceDir)) {
    const staging = mkdtempSync(join(cache, "source-download-"));
    try {
      run("git", ["init", "--quiet", staging], env);
      run(
        "git",
        [
          "-C",
          staging,
          "fetch",
          "--depth",
          "1",
          "https://github.com/ghostty-org/ghostty.git",
          GHOSTTY_REVISION,
        ],
        env,
      );
      run(
        "git",
        ["-C", staging, "checkout", "--quiet", "--detach", GHOSTTY_REVISION],
        env,
      );
      renameSync(staging, sourceDir);
    } finally {
      rmSync(staging, { recursive: true, force: true });
    }
  }
  env.GHOSTTY_SOURCE_DIR = prepareGhosttySource({
    source: sourceDir,
    revision: GHOSTTY_REVISION,
    patches: join(root, "crates", "newport-terminal", "patches"),
    cache,
    env,
  });
  // The archived source has no .git directory. Ghostty's version detection
  // must not discover Newport's parent checkout and embed its version/hash.
  env.GIT_CEILING_DIRECTORIES = [
    ...new Set([
      dirname(env.GHOSTTY_SOURCE_DIR),
      ...(env.GIT_CEILING_DIRECTORIES?.split(delimiter).filter(Boolean) ?? []),
    ]),
  ].join(delimiter);
  // Debug Ghostty adds expensive invariant checks to every parser operation.
  // Keep overflow/bounds checks while making interactive development usable.
  env.LIBGHOSTTY_VT_SYS_OPTIMIZE ??= "ReleaseSafe";
  return env;
}

export function exportGitHubEnvironment(env, destination) {
  appendFileSync(
    destination.GITHUB_ENV,
    `GHOSTTY_SOURCE_DIR=${env.GHOSTTY_SOURCE_DIR}\nLIBGHOSTTY_VT_SYS_OPTIMIZE=${env.LIBGHOSTTY_VT_SYS_OPTIMIZE}\nGIT_CEILING_DIRECTORIES=${env.GIT_CEILING_DIRECTORIES}\n`,
  );
  for (const key of ["ZIG_LIB_DIR", "RUSTUP_TOOLCHAIN", "CARGO_BUILD_TARGET"])
    if (env[key] && env[key] !== destination[key])
      appendFileSync(destination.GITHUB_ENV, `${key}=${env[key]}\n`);
  const pathKey = Object.keys(env).find(
    (key) => key.toLowerCase() === "path",
  );
  if (pathKey && env[pathKey] !== destination[pathKey])
    appendFileSync(
      destination.GITHUB_PATH,
      `${env[pathKey].split(delimiter)[0]}\n`,
    );
}

if (isMain(import.meta.url)) {
  const args = process.argv.slice(2);
  if (!args.length)
    throw new Error(
      "Usage: node scripts/with-ghostty.mjs <command> [args...] | --github",
    );
  const env = await ghosttyBuildEnv();
  if (args[0] === "--github") {
    if (!process.env.GITHUB_ENV || !process.env.GITHUB_PATH)
      throw new Error("--github requires a GitHub Actions environment");
    exportGitHubEnvironment(env, process.env);
  } else {
    const result = spawnSync(args[0], args.slice(1), {
      env,
      cwd: root,
      stdio: "inherit",
    });
    if (result.error) throw result.error;
    process.exitCode = result.status ?? 1;
  }
}
