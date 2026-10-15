import { readFileSync, mkdirSync, copyFileSync, writeFileSync } from "node:fs";
import { join, basename, resolve } from "node:path";
import { createHash } from "node:crypto";
import { parseArgs } from "node:util";
import { root, json } from "./tooling.mjs";
import { signUpdate } from "./sign-update.mjs";
import { verifyPe } from "./package-windows-native.mjs";
const { values, positionals } = parseArgs({
  allowPositionals: true,
  options: {
    arch: { type: "string" },
    output: { type: "string" },
    "app-executable": { type: "string" },
  },
});
if (
  positionals.length !== 1 ||
  !["x86_64", "aarch64"].includes(values.arch) ||
  !values.output ||
  !values["app-executable"]
)
  throw new Error("Supply installer, --arch, --app-executable and --output");
const version = json(join(root, "resources/app.json")).version;
verifyPe(readFileSync(values["app-executable"]), values.arch);
const output = resolve(values.output);
mkdirSync(output, { recursive: true });
const installer = join(
  output,
  `Newport-${version}-windows-${values.arch}-setup.exe`,
);
copyFileSync(positionals[0], installer);
signUpdate(installer, version);
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
