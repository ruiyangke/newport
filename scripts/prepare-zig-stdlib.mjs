// Zig 0.16.0's dynamically loaded ntdll function pointer needs an explicit
// alignment assertion on Windows ARM64. Keep the compiler installation intact:
// use a private, versioned library copy and reject unknown source contents.
import { createHash } from "node:crypto";
import {
  cpSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  renameSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { join } from "node:path";

const relativePath = "std/debug/SelfInfo/Windows.zig";
const originalHash =
  "b7a7b689399c854700fa08d6541956e8d9ae72c05f7009d36a438384a22de1fb";
const fixedHash =
  "e28bab3539d3e6b21b260bd78b555fee8af40a989313095efef1255042d1bf51";
const hash = (bytes) => createHash("sha256").update(bytes).digest("hex");

export function prepareZigStandardLibrary({ lib, cache }) {
  const source = readFileSync(join(lib, relativePath));
  const sourceHash = hash(source);
  // Nested wrappers may receive the already prepared library through ZIG_LIB_DIR.
  if (sourceHash === fixedHash) return lib;
  if (sourceHash !== originalHash)
    throw new Error("Unrecognized Zig 0.16.0 Windows standard library");
  const destination = join(cache, `zig-stdlib-0.16.0-${fixedHash.slice(0, 16)}`);
  const validate = () => {
    if (hash(readFileSync(join(destination, relativePath))) !== fixedHash)
      throw new Error("Prepared Zig standard library was modified");
    return destination;
  };
  if (existsSync(destination)) return validate();
  mkdirSync(cache, { recursive: true });
  const staging = mkdtempSync(join(cache, "zig-stdlib-"));
  try {
    const stagedLib = join(staging, "lib");
    cpSync(lib, stagedLib, { recursive: true });
    const fixed = source
      .toString("utf8")
      .replace(
        "@ptrCast(try si.loadNtdllProc(@tagName(proc)))",
        "@ptrCast(@alignCast(try si.loadNtdllProc(@tagName(proc))))",
      );
    if (hash(fixed) !== fixedHash)
      throw new Error("Zig Windows alignment patch did not match");
    writeFileSync(join(stagedLib, relativePath), fixed);
    try {
      renameSync(stagedLib, destination);
    } catch (error) {
      if (!existsSync(destination)) throw error;
    }
    return validate();
  } finally {
    rmSync(staging, { recursive: true, force: true });
  }
}
