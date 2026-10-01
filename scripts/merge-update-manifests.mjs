import {
  mkdtempSync,
  readFileSync,
  writeFileSync,
  statSync,
  rmSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, basename, dirname, resolve } from "node:path";
import { parseArgs } from "node:util";
import { root, json, run, isMain } from "./tooling.mjs";
function filename(entry) {
  const name = decodeURIComponent(
    new URL(entry.url).pathname.split("/").at(-1),
  );
  if (
    !name ||
    name === "." ||
    name === ".." ||
    basename(name) !== name ||
    /[\\\0]/.test(name)
  )
    throw new Error("Invalid update artifact filename");
  return name;
}
export function merge(paths, version, assets) {
  const result = { version, notes: `Newport ${version}`, platforms: {} };
  for (const path of paths) {
    const fragment = json(path);
    if (fragment.version !== version)
      throw new Error("Cannot merge different release versions");
    result.pub_date ??= fragment.pub_date;
    for (const [target, entry] of Object.entries(fragment.platforms)) {
      if (Object.hasOwn(result.platforms, target))
        throw new Error(`Duplicate update target: ${target}`);
      const url = new URL(entry.url);
      if (
        url.protocol !== "https:" ||
        url.host !== "github.com" ||
        url.username ||
        url.password ||
        !url.pathname.startsWith(
          `/ruiyangke/newport/releases/download/v${version}/`,
        )
      )
        throw new Error("Update URL does not match this release");
      const name = filename(entry);
      if (!statSync(join(assets, name)).isFile())
        throw new Error(`Missing update artifact: ${name}`);
      if (
        !entry.signature ||
        readFileSync(join(assets, `${name}.sig`), "utf8").trim() !==
          entry.signature
      )
        throw new Error(`Missing or inconsistent signature: ${name}`);
      Object.defineProperty(result.platforms, target, {
        value: entry,
        enumerable: true,
        configurable: true,
      });
    }
  }
  for (const target of ["darwin-aarch64", "windows-x86_64", "windows-aarch64"])
    if (!Object.hasOwn(result.platforms, target))
      throw new Error(`Missing release target: ${target}`);
  return result;
}
function base64(value) {
  if (
    !/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(
      value,
    )
  )
    throw new Error("Invalid base64");
  return Buffer.from(value, "base64");
}
export function verifyPackages(manifest, assets, publicKey) {
  const temp = mkdtempSync(join(tmpdir(), "newport-signatures-"));
  try {
    const key = join(temp, "public.key"),
      signature = join(temp, "signature");
    writeFileSync(key, base64(publicKey));
    for (const entry of Object.values(manifest.platforms)) {
      const text = new TextDecoder("utf-8", { fatal: true }).decode(
        base64(entry.signature),
      );
      writeFileSync(signature, text);
      run("minisign", [
        "-Vm",
        join(assets, filename(entry)),
        "-p",
        key,
        "-x",
        signature,
      ]);
      const comment = text
        .split("\n")
        .find((line) => line.startsWith("trusted comment: "))
        ?.slice("trusted comment: ".length);
      const signed = comment
        ?.split("\t")
        .find((field) => field.startsWith("version:"))
        ?.slice(8);
      if (signed !== manifest.version)
        throw new Error(
          "Artifact signature is not bound to this release version",
        );
    }
  } finally {
    rmSync(temp, { recursive: true, force: true });
  }
}
if (isMain(import.meta.url)) {
  const { values, positionals } = parseArgs({
    allowPositionals: true,
    options: { version: { type: "string" }, output: { type: "string" } },
  });
  if (!values.version || !values.output || !positionals.length)
    throw new Error("Supply fragments, --version and --output");
  const assets = dirname(resolve(values.output));
  const manifest = merge(positionals, values.version, assets);
  verifyPackages(
    manifest,
    assets,
    json(join(root, "src-tauri/tauri.conf.json")).plugins.updater.pubkey,
  );
  writeFileSync(values.output, `${JSON.stringify(manifest, null, 2)}\n`);
}
