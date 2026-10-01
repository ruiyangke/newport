import { appendFileSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { root, json, capture } from "../tooling.mjs";
const tag = process.env.RELEASE_TAG || "";
if (!/^v\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(tag))
  throw new Error("Expected a version tag such as v0.3.0");
const versions = [
  "package.json",
  "package-lock.json",
  "src-tauri/tauri.conf.json",
].map((path) => json(join(root, path)).version);
const cargo = readFileSync(join(root, "src-tauri/Cargo.toml"), "utf8")
  .split("[package]")[1]
  ?.split(/^\[/m)[0];
versions.push(cargo?.match(/^version\s*=\s*"([^"]+)"/m)?.[1]);
if (versions.some((version) => `v${version}` !== tag))
  throw new Error("Release tag and package versions must match");
const commit = capture("git", ["rev-parse", `refs/tags/${tag}^{commit}`]);
if (commit !== capture("git", ["rev-parse", "HEAD"]))
  throw new Error("Checkout must match the release tag");
if (process.env.GITHUB_OUTPUT)
  appendFileSync(process.env.GITHUB_OUTPUT, `commit=${commit}\n`);
