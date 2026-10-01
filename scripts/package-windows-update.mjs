import { readFileSync, mkdirSync, copyFileSync, writeFileSync } from "node:fs";
import { join, basename, resolve } from "node:path";
import { createHash } from "node:crypto";
import { parseArgs } from "node:util";
import { root, json, run, signerEnv } from "./tooling.mjs";
const { values, positionals } = parseArgs({
  allowPositionals: true,
  options: { arch: { type: "string" }, output: { type: "string" } },
});
if (
  positionals.length !== 1 ||
  !["x86_64", "aarch64"].includes(values.arch) ||
  !values.output
)
  throw new Error("Supply installer, --arch and --output");
const version = json(join(root, "src-tauri/tauri.conf.json")).version;
const data = readFileSync(join(root, "src-tauri/target/release/newport.exe"));
const offset = data.readUInt32LE(0x3c);
if (
  data.subarray(offset, offset + 4).toString("hex") !== "50450000" ||
  data.readUInt16LE(offset + 4) !==
    { x86_64: 0x8664, aarch64: 0xaa64 }[values.arch]
)
  throw new Error(
    "Application architecture does not match the requested update target",
  );
const env = signerEnv(),
  output = resolve(values.output);
mkdirSync(output, { recursive: true });
const installer = join(
  output,
  `Newport-${version}-windows-${values.arch}-setup.exe`,
);
copyFileSync(positionals[0], installer);
run(
  process.execPath,
  [
    join(root, "node_modules/@tauri-apps/cli/tauri.js"),
    "signer",
    "sign",
    "--app-version",
    version,
    installer,
  ],
  { env },
);
const manifest = {
  version,
  notes: `Newport ${version}`,
  pub_date: new Date().toISOString(),
  platforms: {
    [`windows-${values.arch}`]: {
      signature: readFileSync(`${installer}.sig`, "utf8").trim(),
      url: `https://github.com/ruiyangke/newport/releases/download/v${version}/${basename(installer)}`,
    },
  },
};
writeFileSync(
  join(output, `windows-${values.arch}.json`),
  `${JSON.stringify(manifest, null, 2)}\n`,
);
writeFileSync(
  join(output, `SHA256SUMS-windows-${values.arch}`),
  `${createHash("sha256").update(readFileSync(installer)).digest("hex")}  ${basename(installer)}\n`,
);
