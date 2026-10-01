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
export function signerEnv() {
  const env = {
    ...process.env,
    TAURI_SIGNING_PRIVATE_KEY_PASSWORD:
      process.env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD || "",
  };
  const key = env.TAURI_SIGNING_PRIVATE_KEY;
  if (!key) throw new Error("TAURI_SIGNING_PRIVATE_KEY is required");
  let file = false;
  if (!key.includes("\n") && key.length < 1024) {
    try {
      file = statSync(key).isFile();
    } catch {}
  }
  if (file) {
    env.TAURI_SIGNING_PRIVATE_KEY_PATH = key;
    delete env.TAURI_SIGNING_PRIVATE_KEY;
  }
  return env;
}
export function plist(path) {
  return JSON.parse(capture("plutil", ["-convert", "json", "-o", "-", path]));
}
