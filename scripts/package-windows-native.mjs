// Tauri-free NSIS packaging. This builds an unsigned candidate, never installs
// it or publishes it. Signing and native upgrade acceptance are separate gates.
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import {
  lstatSync,
  readFileSync,
  writeFileSync,
  mkdirSync,
  mkdtempSync,
  rmSync,
  readdirSync,
  copyFileSync,
  existsSync,
  realpathSync,
  renameSync,
} from "node:fs";
import { join, resolve, dirname, relative, isAbsolute, sep } from "node:path";
import { parseArgs } from "node:util";
import { root, isMain } from "./tooling.mjs";

const files = [
  "newport.exe",
  "grammars/newport_grammars.dll",
  "agents/newport-agent-x86_64.gz",
  "agents/newport-agent-aarch64.gz",
  "pdfium/pdfium.dll",
  "pdfium/LICENSE-PDFIUM",
  "Fonts/SymbolsNerdFontMono-Regular.ttf",
  "Fonts/NERD-FONTS-LICENSE",
  "Fonts/README.md",
  "LICENSE-GPUI-KIT",
  "Licenses/terminal/LICENSE-Ghostty",
  "Licenses/terminal/LICENSE-Highway-APACHE",
  "Licenses/terminal/LICENSE-Highway-BSD3",
  "Licenses/terminal/LICENSE-libghostty-rs",
  "Licenses/terminal/LICENSE-simdutf-APACHE",
  "Licenses/terminal/LICENSE-simdutf-MIT",
  "Licenses/terminal/LICENSE-simdutf-TH",
  "Licenses/terminal/README.md",
  "Licenses/terminal/uucode/LICENSE.md",
  "Licenses/terminal/uucode/licenses/LICENSE_Bjoern_Hoehrmann",
  "Licenses/terminal/uucode/licenses/LICENSE_unicode",
  "icon.ico",
  "newport-package.json",
];
const directories = [
  ...new Set(
    files.flatMap((file) => {
      const parts = file.split("/");
      return parts
        .slice(0, -1)
        .map((_, index) => parts.slice(0, index + 1).join("/"));
    }),
  ),
].sort(
  (a, b) => b.split("/").length - a.split("/").length || a.localeCompare(b),
);
const machines = { x86_64: 0x8664, aarch64: 0xaa64 };

export function verifyPe(bytes, arch, dll = false) {
  if (!(arch in machines)) throw new Error("Unsupported Windows architecture");
  if (bytes.length < 64 || bytes.toString("ascii", 0, 2) !== "MZ")
    throw new Error("Invalid PE executable");
  const offset = bytes.readUInt32LE(0x3c);
  if (
    offset < 64 ||
    offset + 26 > bytes.length ||
    bytes.readUInt32LE(offset) !== 0x4550 ||
    bytes.readUInt16LE(offset + 4) !== machines[arch] ||
    bytes.readUInt16LE(offset + 24) !== 0x20b ||
    !(bytes.readUInt16LE(offset + 22) & 2) ||
    Boolean(bytes.readUInt16LE(offset + 22) & 0x2000) !== dll
  )
    throw new Error(
      "PE architecture or executable type does not match the package",
    );
}

function quote(value) {
  // These are build paths, not user install paths. Reject preprocessor syntax
  // instead of letting an NSIS macro expand an untrusted filesystem name.
  if (/[\r\n\0$]/.test(value)) throw new Error("Unsupported NSIS build path");
  return `"${value.replaceAll('"', '$\\"')}"`;
}

export function inspectPayload(app, arch) {
  app = resolve(app);
  if (!lstatSync(app).isDirectory() || lstatSync(app).isSymbolicLink())
    throw new Error("Expected a real staged application directory");
  const visited = [];
  function walk(directory, prefix = "") {
    for (const name of readdirSync(directory)) {
      const relative = prefix + name,
        path = join(directory, name),
        stat = lstatSync(path);
      if (stat.isSymbolicLink())
        throw new Error("Package symlinks are not supported");
      if (
        stat.isDirectory() &&
        directories.includes(relative)
      )
        walk(path, relative + "/");
      else if (stat.isFile() && files.includes(relative))
        visited.push(relative);
      else throw new Error(`Unexpected package entry: ${relative}`);
    }
  }
  walk(app);
  for (const file of files)
    if (!visited.includes(file))
      throw new Error(`Missing package file: ${file}`);
  const metadata = JSON.parse(
    readFileSync(join(app, "newport-package.json"), "utf8"),
  );
  if (
    metadata.productName !== "Newport" ||
    metadata.identifier !== "app.newport" ||
    metadata.target !== `windows-${arch}`
  )
    throw new Error("Package identity or target does not match Newport");
  const version = metadata.version;
  if (
    typeof version !== "string" ||
    !/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(version) ||
    version.split(".").some((n) => Number(n) > 65535)
  )
    throw new Error("NSIS packages require a stable numeric release version");
  verifyPe(readFileSync(join(app, "newport.exe")), arch);
  verifyPe(
    readFileSync(join(app, "grammars/newport_grammars.dll")),
    arch,
    true,
  );
  verifyPe(readFileSync(join(app, "pdfium/pdfium.dll")), arch, true);
  const entries = files.map((path) => ({
    path,
    hash: createHash("sha256")
      .update(readFileSync(join(app, path)))
      .digest("hex"),
  }));
  const fingerprint = createHash("sha256")
    .update(JSON.stringify(entries))
    .digest("hex");
  return { app, arch, version, entries, fingerprint };
}

export function installerSource(payload, output) {
  const { app, version, arch, entries, fingerprint } = payload;
  const own = entries.map((entry) => entry.path).concat("uninstall.exe");
  // Back up the executable first, preventing launch of mixed-version resources;
  // promote it last. Each file has a flushed write-ahead recovery record.
  const resources = own.filter((path) => path !== "newport.exe");
  const macro = (name, lines) =>
    `!macro ${name}\n${lines.join("\n")}\n!macroend\n`;
  const native = (path) => path.replaceAll("/", "\\");
  const call = (operation, path) =>
    `  !insertmacro ${operation} ${own.indexOf(path)} "${native(path)}"`;
  return [
    `!define VERSION "${version}"`,
    `!define VERSION_QUAD "${version}.0"`,
    `!define ARCH "${arch}"`,
    `!define FINGERPRINT "${fingerprint}"`,
    `!define OUTPUT ${quote(resolve(output))}`,
    `!define ICON ${quote(join(app, "icon.ico"))}`,
    macro(
      "ExtractPayload",
      entries.flatMap(({ path }) => [
        `  SetOutPath "$Stage\\${native(dirname(path) === "." ? "" : dirname(path))}"`,
        `  File ${quote(join(app, path))}`,
      ]),
    ),
    macro(
      "BackupPayload",
      own.map((path) => call("BackupFile", path)),
    ),
    macro(
      "PromotePayload",
      resources.concat("newport.exe").map((path) => call("PromoteFile", path)),
    ),
    macro(
      "RestorePayload",
      resources.concat("newport.exe").map((path) => call("RestoreFile", path)),
    ),
    macro(
      "RemovePayload",
      [
        ...own.map((path) => `  Delete "$INSTDIR\\${native(path)}"`),
        // Remove only empty owned directories, deepest first. Unknown files
        // belong to the user and must survive uninstall.
        ...directories.map((path) => `  RMDir "$INSTDIR\\${native(path)}"`),
      ],
    ),
    `!include ${quote(join(root, "resources/windows/installer.nsi"))}`,
  ].join("\n");
}

export function buildWindowsInstaller({
  app,
  arch,
  output,
  compiler = "makensis",
}) {
  const payload = inspectPayload(app, arch);
  output = resolve(output);
  // Resolve existing ancestors as well as the lexical path so a directory
  // symlink cannot redirect the output into the source application.
  let ancestor = dirname(output);
  while (!existsSync(ancestor)) ancestor = dirname(ancestor);
  const destination = resolve(
    realpathSync(ancestor),
    relative(ancestor, output),
  );
  const within = relative(realpathSync(payload.app), destination);
  if (
    within === "" ||
    (!isAbsolute(within) && within !== ".." && !within.startsWith(`..${sep}`))
  )
    throw new Error("Installer output must be outside the staged application");
  mkdirSync(dirname(output), { recursive: true });
  // Build alongside the destination so publication can use an atomic rename.
  // NSIS never writes to an existing output, which could alias a payload file.
  const temporary = mkdtempSync(join(dirname(output), ".newport-nsis-"));
  try {
    const snapshot = join(temporary, "payload");
    for (const { path } of payload.entries) {
      const target = join(snapshot, path);
      mkdirSync(dirname(target), { recursive: true });
      copyFileSync(join(payload.app, path), target);
    }
    const captured = inspectPayload(snapshot, arch);
    if (captured.fingerprint !== payload.fingerprint)
      throw new Error(
        "Staged application changed while preparing the installer",
      );
    const source = join(temporary, "installer.nsi");
    const installer = join(temporary, "Newport-installer.exe");
    writeFileSync(source, installerSource(captured, installer));
    const prefix = process.platform === "win32" ? "/" : "-";
    execFileSync(
      compiler,
      [
        prefix + "NOCONFIG",
        prefix + "NOCD",
        prefix + "WX",
        prefix + "V2",
        source,
      ],
      { stdio: "inherit" },
    );
    if (!lstatSync(installer).isFile())
      throw new Error("NSIS did not produce an installer");
    renameSync(installer, output);
    return output;
  } finally {
    rmSync(temporary, { recursive: true, force: true });
  }
}

if (isMain(import.meta.url)) {
  const { values } = parseArgs({
    options: {
      app: { type: "string" },
      arch: { type: "string" },
      output: { type: "string" },
      compiler: { type: "string" },
    },
  });
  if (!values.app || !values.arch || !values.output)
    throw new Error("Supply --app, --arch and --output");
  console.log(`Unsigned native installer: ${buildWindowsInstaller(values)}`);
}
