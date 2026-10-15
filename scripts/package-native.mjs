// Stage the native application and grammar resources together. This produces a
// development archive; production signing/notarization remains a release gate.
import { spawnSync } from "node:child_process";
import {
  cpSync,
  mkdirSync,
  writeFileSync,
  readFileSync,
  rmSync,
} from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import { readBuildInfo, macInfoPlist } from "./native-build-info.mjs";
import { buildWindowsInstaller } from "./package-windows-native.mjs";
import { stagePdfium, validateLibrary } from "./stage-pdfium.mjs";
import { ghosttyBuildEnv } from "./with-ghostty.mjs";
import { releaseBuildEnv } from "./tooling.mjs";
const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const { values } = parseArgs({
  options: {
    "skip-build": { type: "boolean" },
    debug: { type: "boolean" },
    nsis: { type: "boolean" },
    target: { type: "string" },
  },
});
const profile = values.debug ? "debug" : "release";
const profileArgs = values.debug ? [] : ["--release"];
const target = values.target ?? process.env.CARGO_BUILD_TARGET;
if (
  target &&
  (!/^(aarch64|x86_64)-(apple-darwin|pc-windows-msvc|unknown-linux-(gnu|musl))$/.test(
    target,
  ) ||
    target.includes("windows") !== (process.platform === "win32") ||
    target.includes("apple") !== (process.platform === "darwin"))
)
  throw new Error(
    "Package target must be a supported triple for this host platform",
  );
if (values.nsis && process.platform !== "win32")
  throw new Error(
    "Build native Windows packages on Windows; use package-windows-native.mjs for a prebuilt payload",
  );
const product = JSON.parse(
  readFileSync(join(root, "resources/app.json"), "utf8"),
);
function run(command, args, env = process.env) {
  const result = spawnSync(command, args, { cwd: root, stdio: "inherit", env });
  if (result.error) throw result.error;
  if (result.status !== 0)
    throw new Error(`${command} failed (${result.status})`);
}
function verifyBuild(binary) {
  return readBuildInfo(binary, {
    os: { darwin: "macos", win32: "windows", linux: "linux" }[process.platform],
    arch:
      target?.split("-")[0] ??
      { arm64: "aarch64", x64: "x86_64" }[process.arch],
    debug: Boolean(values.debug),
  });
}
if (!values["skip-build"]) {
  // Prepare pinned compiler/source before Cargo invokes Ghostty's build script.
  // Compiler and dependency processes never need distribution signing secrets.
  const buildEnv = await ghosttyBuildEnv(releaseBuildEnv());
  run(
    process.execPath,
    [
      "scripts/build-grammars.mjs",
      ...profileArgs,
      ...(target ? ["--target", target] : []),
    ],
    buildEnv,
  );
  run(
    "cargo",
    [
      "build",
      "--locked",
      ...profileArgs,
      "-p",
      "newport-app",
      ...(target ? ["--target", target] : []),
    ],
    buildEnv,
  );
}
const result = spawnSync(
  "cargo",
  ["metadata", "--no-deps", "--format-version", "1"],
  { cwd: root, encoding: "utf8" },
);
if (result.status !== 0) throw new Error(result.stderr);
const metadata = JSON.parse(result.stdout);
if (
  metadata.packages.find((p) => p.name === "newport-app")?.version !==
  product.version
)
  throw new Error("Native package and product versions differ");
const output = join(
  metadata.target_directory,
  ...(target ? [target] : []),
  profile,
);
const stage = join(output, "native-package");
mkdirSync(stage, { recursive: true });
const mac = process.platform === "darwin";
const windows = process.platform === "win32";
const app = join(stage, mac ? "Newport.app" : "Newport");
// This directory is generated packaging output, never an installed application.
rmSync(app, { recursive: true, force: true });
const bin = mac ? join(app, "Contents/MacOS") : app;
const resources = mac ? join(app, "Contents/Resources") : app;
mkdirSync(bin, { recursive: true });
mkdirSync(resources, { recursive: true });
cpSync(
  join(output, windows ? "newport.exe" : "newport"),
  join(bin, windows ? "newport.exe" : "newport"),
);
cpSync(join(output, "agents"), join(resources, "agents"), { recursive: true });
cpSync(join(output, "grammars"), join(resources, "grammars"), {
  recursive: true,
});
cpSync(join(root, "resources/fonts"), join(resources, "Fonts"), {
  recursive: true,
});
cpSync(
  join(root, "crates/newport-grammars/LICENSE-GPUI-KIT"),
  join(resources, "LICENSE-GPUI-KIT"),
);
cpSync(join(root, "resources/licenses"), join(resources, "Licenses"), {
  recursive: true,
});
if (mac) {
  cpSync(join(root, "resources/icons/icon.icns"), join(resources, "icon.icns"));
  writeFileSync(
    join(app, "Contents/Info.plist"),
    macInfoPlist(verifyBuild(join(bin, "newport"))),
  );
  run("xcrun", ["strip", join(bin, "newport")]);
  const library = join(resources, "grammars/libnewport_grammars.dylib");
  run("xcrun", ["strip", "-x", library]);
  run("codesign", ["--force", "--sign", "-", library]);
  run("codesign", ["--force", "--sign", "-", app]);
  verifyBuild(join(bin, "newport"));
  run(join(bin, "newport"), ["--check-needle"]);
  run(join(bin, "newport"), ["--check-grammars"]);
  run(join(bin, "newport"), ["--check-pdf"]);
  run("ditto", [
    "-c",
    "-k",
    "--keepParent",
    app,
    join(stage, "Newport-macos.zip"),
  ]);
} else if (windows) {
  const arch = target
    ? {
        "x86_64-pc-windows-msvc": "x86_64",
        "aarch64-pc-windows-msvc": "aarch64",
      }[target]
    : { x64: "x86_64", arm64: "aarch64" }[process.arch];
  if (!arch) throw new Error("Unsupported native Windows target");
  await stagePdfium(join(output, "pdfium"), arch);
  validateLibrary(readFileSync(join(output, "pdfium/pdfium.dll")), arch);
  cpSync(join(output, "pdfium"), join(resources, "pdfium"), {
    recursive: true,
  });
  verifyBuild(join(bin, "newport.exe"));
  cpSync(join(root, "resources/icons/icon.ico"), join(app, "icon.ico"));
  writeFileSync(
    join(app, "newport-package.json"),
    JSON.stringify({ ...product, target: `windows-${arch}` }, null, 2) + "\n",
  );
  run(join(bin, "newport.exe"), ["--check-needle"]);
  run(join(bin, "newport.exe"), ["--check-grammars"]);
  run(join(bin, "newport.exe"), ["--check-pdf"]);
  if (values.nsis) {
    buildWindowsInstaller({
      app,
      arch,
      output: join(
        stage,
        `Newport-${product.version}-windows-${arch}-setup.exe`,
      ),
    });
  }
  // Pass paths as environment variables, never interpolate them as shell code.
  const zip = spawnSync(
    "powershell",
    [
      "-NoProfile",
      "-Command",
      "Compress-Archive -LiteralPath $env:NEWPORT_PACKAGE_SOURCE -DestinationPath $env:NEWPORT_PACKAGE_ZIP -Force",
    ],
    {
      stdio: "inherit",
      env: {
        ...process.env,
        NEWPORT_PACKAGE_SOURCE: app,
        NEWPORT_PACKAGE_ZIP: join(stage, "Newport-windows.zip"),
      },
    },
  );
  if (zip.status !== 0) throw new Error("Archive creation failed");
} else {
  verifyBuild(join(bin, "newport"));
  run(join(bin, "newport"), ["--check-needle"]);
  run(join(bin, "newport"), ["--check-grammars"]);
  run("tar", [
    "-czf",
    join(stage, "Newport-linux.tar.gz"),
    "-C",
    stage,
    "Newport",
  ]);
}
console.log(`Development package: ${stage}`);
