// Pinned non-V8 PDFium for Windows. No download occurs during app startup.
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import {
  readFileSync,
  writeFileSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  lstatSync,
  rmSync,
  renameSync,
  existsSync,
} from "node:fs";
import { join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { parseArgs } from "node:util";
import { root, isMain } from "./tooling.mjs";
import { verifyPe } from "./package-windows-native.mjs";
export const pdfium = JSON.parse(
  readFileSync(join(root, "resources/pdfium.json"), "utf8"),
);
const digest = (bytes) => createHash("sha256").update(bytes).digest("hex");

export function validateLibrary(bytes, arch) {
  const pinned = pdfium.architectures[arch];
  if (!pinned || digest(bytes) !== pinned.librarySha256)
    throw new Error("PDF renderer does not match the pinned library");
  verifyPe(bytes, arch, true);
}
export async function stagePdfium(
  output,
  arch,
  download = async (url) => {
    const response = await fetch(url, { signal: AbortSignal.timeout(60000) });
    if (!response.ok)
      throw new Error(`PDFium download failed: ${response.status}`);
    return Buffer.from(await response.arrayBuffer());
  },
) {
  const pinned = pdfium.architectures[arch];
  if (!pinned) throw new Error("Unsupported PDFium architecture");
  output = resolve(output);
  if (
    existsSync(output) &&
    (!lstatSync(output).isDirectory() || lstatSync(output).isSymbolicLink())
  )
    throw new Error("PDFium staging destination must be a real directory");
  if (
    existsSync(join(output, "pdfium.dll")) &&
    existsSync(join(output, "LICENSE-PDFIUM"))
  ) {
    try {
      validateLibrary(readFileSync(join(output, "pdfium.dll")), arch);
      if (
        digest(readFileSync(join(output, "LICENSE-PDFIUM"))) !==
        pinned.licenseSha256
      )
        throw new Error("PDFium license checksum mismatch");
      return;
    } catch {
      /* rebuild invalid staging output */
    }
  }
  const temporary = mkdtempSync(join(tmpdir(), "newport-pdfium-"));
  try {
    const archive = await download(
      `${pdfium.source}/releases/download/${pdfium.release}/${pinned.archive}`,
    );
    if (digest(archive) !== pinned.sha256)
      throw new Error("PDFium archive checksum mismatch");
    const archivePath = join(temporary, "pdfium.tgz");
    writeFileSync(archivePath, archive);
    const unpacked = join(temporary, "unpacked");
    mkdirSync(unpacked);
    execFileSync("tar", ["-xzf", archivePath, "-C", unpacked]);
    const library = readFileSync(join(unpacked, "bin/pdfium.dll"));
    validateLibrary(library, arch);
    const licenses = [
      "LICENSE",
      ...readdirSync(join(unpacked, "licenses"))
        .sort()
        .map((name) => `licenses/${name}`),
    ];
    const notice = licenses
      .map(
        (name) =>
          `===== ${name} =====\n${readFileSync(join(unpacked, name), "utf8")}\n`,
      )
      .join("\n");
    if (digest(notice) !== pinned.licenseSha256)
      throw new Error("PDFium license checksum mismatch");
    mkdirSync(output, { recursive: true });
    // Stage complete files before replacing build output. Packages are validated
    // again before copying; this directory is never an installed application.
    const suffix = `.new-${process.pid}`;
    for (const [name, bytes] of [
      ["LICENSE-PDFIUM", notice],
      ["pdfium.dll", library],
    ]) {
      const pending = join(output, name + suffix);
      try {
        writeFileSync(pending, bytes);
        renameSync(pending, join(output, name));
      } finally {
        rmSync(pending, { force: true });
      }
    }
  } finally {
    rmSync(temporary, { recursive: true, force: true });
  }
}
if (isMain(import.meta.url)) {
  const { values } = parseArgs({
    options: { target: { type: "string" }, release: { type: "boolean" } },
  });
  const target = values.target ?? process.env.CARGO_BUILD_TARGET;
  const arch = target
    ? {
        "aarch64-pc-windows-msvc": "aarch64",
        "x86_64-pc-windows-msvc": "x86_64",
      }[target]
    : process.platform === "win32"
      ? { arm64: "aarch64", x64: "x86_64" }[process.arch]
      : undefined;
  if (!arch) throw new Error("Specify a Windows target for PDFium staging");
  const metadata = JSON.parse(
    execFileSync("cargo", ["metadata", "--no-deps", "--format-version", "1"], {
      cwd: root,
      encoding: "utf8",
    }),
  );
  const output = join(
    metadata.target_directory,
    ...(target ? [target] : []),
    values.release ? "release" : "debug",
    "pdfium",
  );
  await stagePdfium(output, arch);
  console.log(`Staged PDFium ${pdfium.release}: ${output}`);
}
