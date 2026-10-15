// Build an isolated macOS development bundle; launching is a separate step.
import { copyFile, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { dirname, isAbsolute, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

if (process.platform !== "darwin") throw new Error("This development bundle is for macOS.");
const dataDirectory = process.env.NEWPORT_DATA_DIR;
if (!dataDirectory || !isAbsolute(dataDirectory)) {
  throw new Error("Set NEWPORT_DATA_DIR to an absolute isolated profile directory.");
}
const project = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repository = resolve(project, "../..");
const target = join(repository, "target");
const manifest = JSON.parse(await readFile(join(repository, "resources/app.json"), "utf8"));
const build = spawnSync("cargo", ["build", "--locked", "--manifest-path", join(project, "Cargo.toml")], {
  stdio: "inherit",
  env: { ...process.env, CARGO_TARGET_DIR: target },
});
if (build.error) throw build.error;
if (build.status !== 0) process.exit(build.status ?? 1);

const bundle = join(target, "Newport Native Test.app");
const contents = join(bundle, "Contents");
await mkdir(join(contents, "MacOS"), { recursive: true });
await mkdir(join(contents, "Resources"), { recursive: true });
const executable = join(contents, "MacOS/newport-native");
// Remove an older development symlink before copying a standalone executable.
await rm(executable, { force: true });
await copyFile(join(target, "debug/newport-gpui"), executable);
await copyFile(join(project, "assets/XTERM-LICENSE"), join(contents, "Resources/XTERM-LICENSE"));
await copyFile(join(repository, "resources/icons/icon.icns"), join(contents, "Resources/newport.icns"));
await mkdir(join(contents, "Resources/Fonts"), { recursive: true });
for (const name of ["LICENSE-symbols", "LICENSE-lucide"]) {
  await copyFile(join(project, "assets/file-icons", name), join(contents, "Resources", name));
}
await copyFile(join(repository, "resources/fonts/SymbolsNerdFontMono-Regular.ttf"), join(contents, "Resources/Fonts/SymbolsNerdFontMono-Regular.ttf"));
await copyFile(join(repository, "resources/fonts/NERD-FONTS-LICENSE"), join(contents, "Resources/NERD-FONTS-LICENSE"));
const xml = (value) => String(value).replace(/[&<>"']/g, (char) => ({
  "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&apos;",
})[char]);
await writeFile(join(contents, "Info.plist"), `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>newport-native</string>
<key>CFBundleIdentifier</key><string>app.newport.native-test</string>
<key>CFBundleName</key><string>Newport Native Test</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleIconFile</key><string>newport.icns</string>
<key>ATSApplicationFontsPath</key><string>Fonts/</string>
<key>CFBundleShortVersionString</key><string>${xml(manifest.version)}</string>
<key>CFBundleVersion</key><string>${xml(manifest.version)}</string>
<key>NSHighResolutionCapable</key><true/>
<key>LSEnvironment</key><dict><key>NEWPORT_DATA_DIR</key><string>${xml(dataDirectory)}</string></dict>
</dict></plist>
`);
console.log(bundle);
