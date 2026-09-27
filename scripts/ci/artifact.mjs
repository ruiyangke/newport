// Bind reusable build artifacts to both their source commit and exact contents.
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { resolve, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../../", import.meta.url));
const manifest = "newport-build.json";
function snapshot(directory) {
  const files = {};
  function walk(relative = "") {
    for (const entry of readdirSync(join(root, directory, relative), {
      withFileTypes: true,
    }).sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0))) {
      const path = relative ? `${relative}/${entry.name}` : entry.name;
      if (path === manifest) continue;
      if (entry.isDirectory()) walk(path);
      else if (entry.isFile()) {
        files[path] = createHash("sha256")
          .update(readFileSync(join(root, directory, path)))
          .digest("hex");
      } else throw new Error(`Unexpected artifact entry: ${path}`);
    }
  }
  walk();
  if (!Object.keys(files).length)
    throw new Error(`Empty artifact: ${directory}`);
  const commit = execFileSync("git", ["rev-parse", "HEAD"], {
    cwd: root,
    encoding: "utf8",
  }).trim();
  return { commit, files };
}
export function verifyArtifact(directory) {
  const expected = JSON.parse(
    readFileSync(join(root, directory, manifest), "utf8"),
  );
  const actual = snapshot(directory);
  if (
    expected.commit !== actual.commit ||
    JSON.stringify(expected.files) !== JSON.stringify(actual.files)
  ) {
    throw new Error(`Artifact does not match this checkout: ${directory}`);
  }
}
if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  const [command, directory] = process.argv.slice(2);
  if (command === "record")
    writeFileSync(
      join(root, directory, manifest),
      JSON.stringify(snapshot(directory)) + "\n",
    );
  else if (command === "verify") verifyArtifact(directory);
  else throw new Error("Usage: artifact.mjs record|verify DIRECTORY");
}
