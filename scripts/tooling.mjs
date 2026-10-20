import { execFileSync } from "node:child_process";
import { readFileSync, statSync } from "node:fs";
import { fileURLToPath, pathToFileURL } from "node:url";
import { resolve } from "node:path";
export const root = fileURLToPath(new URL("../", import.meta.url));
export const json = (path) => JSON.parse(readFileSync(path, "utf8"));
export const run = (command, args, options = {}) =>
  execFileSync(command, args, { cwd: root, stdio: "inherit", ...options });
export const capture = (command, args, options = {}) =>
  run(command, args, {
    stdio: ["pipe", "pipe", "inherit"],
    encoding: "utf8",
    ...options,
  }).trim();
export const isMain = (url) =>
  process.argv[1] && url === pathToFileURL(resolve(process.argv[1])).href;
export function releaseBuildEnv(source = process.env) {
  const env = { ...source };
  for (const name of Object.keys(env))
    if (
      /^NEWPORT_SIGNING_PRIVATE_KEY(?:_PATH|_PASSWORD)?$/.test(name) ||
      [
        "APPLE_PASSWORD",
        "APPLE_CERTIFICATE",
        "APPLE_CERTIFICATE_PASSWORD",
      ].includes(name)
    )
      delete env[name];
  return env;
}
export function signerEnv(source = process.env) {
  const env = {
    ...source,
    NEWPORT_SIGNING_PRIVATE_KEY_PASSWORD:
      source.NEWPORT_SIGNING_PRIVATE_KEY_PASSWORD ?? "",
  };
  const key = env.NEWPORT_SIGNING_PRIVATE_KEY;
  const path = env.NEWPORT_SIGNING_PRIVATE_KEY_PATH;
  if (!key && !path)
    throw new Error(
      "NEWPORT_SIGNING_PRIVATE_KEY or NEWPORT_SIGNING_PRIVATE_KEY_PATH is required",
    );
  if (key && path)
    throw new Error("Supply a signing key or a key path, not both");
  let file = false;
  if (key && !key.includes("\n") && key.length < 1024) {
    try {
      file = statSync(key).isFile();
    } catch {}
  }
  if (path || file) env.NEWPORT_SIGNING_PRIVATE_KEY_PATH = path || key;
  else env.NEWPORT_SIGNING_PRIVATE_KEY = key;
  if (path || file) delete env.NEWPORT_SIGNING_PRIVATE_KEY;
  return env;
}
export function plist(path) {
  return JSON.parse(capture("plutil", ["-convert", "json", "-o", "-", path]));
}
