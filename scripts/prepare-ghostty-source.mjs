// Keep Newport's temporary upstream fixes reproducible without editing a user's
// source checkout. Each revision + patch set gets an immutable build directory.
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  renameSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { join } from "node:path";

const manifestName = ".newport-ghostty-source.json";
const digest = (data) => createHash("sha256").update(data).digest("hex");

function run(command, args, env) {
  const result = spawnSync(command, args, { env, encoding: "utf8" });
  if (result.error) throw result.error;
  if (result.status !== 0)
    throw new Error(`${command} failed: ${result.stderr}`);
  return result.stdout.trim();
}

export function prepareGhosttySource({
  source,
  revision,
  patches,
  cache,
  env,
}) {
  const files = readdirSync(patches)
    .filter((name) => name.endsWith(".patch"))
    .sort();
  const patchSet = files.map((name) => ({
    name,
    hash: digest(readFileSync(join(patches, name), "utf8").replace(/\r\n/g, "\n")),
  }));
  const fingerprint = digest(
    JSON.stringify({ preparationVersion: 4, revision, patchSet }),
  );
  const destination = join(
    cache,
    `patched-${revision}-${fingerprint.slice(0, 16)}`,
  );
  const manifestPath = join(source, manifestName);
  // Nested build wrappers (including CI -> packaging) can receive our own
  // prepared directory. Recover its base when the patch set has changed.
  if (existsSync(manifestPath)) {
    const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
    source = manifest.source;
  }
  if (existsSync(destination)) {
    const manifest = JSON.parse(
      readFileSync(join(destination, manifestName), "utf8"),
    );
    if (manifest.fingerprint !== fingerprint)
      throw new Error("Ghostty source cache fingerprint mismatch");
    for (const [file, hash] of Object.entries(manifest.modifiedFiles)) {
      if (digest(readFileSync(join(destination, file))) !== hash)
        throw new Error(`Prepared Ghostty source was modified: ${file}`);
    }
    return destination;
  }
  if (run("git", ["-C", source, "rev-parse", "HEAD"], env) !== revision)
    throw new Error("Ghostty source revision does not match the Rust bindings");
  mkdirSync(cache, { recursive: true });
  const temporary = mkdtempSync(join(cache, "patch-source-"));
  try {
    const archive = join(temporary, "source.tar");
    const staged = join(temporary, "source");
    mkdirSync(staged);
    // The build cache is nested inside Newport's Git checkout. Stop repository
    // discovery here or git apply silently skips paths outside that prefix.
    const patchEnv = { ...env, GIT_CEILING_DIRECTORIES: temporary };
    // Archive the pinned tree; local edits and untracked files are never copied.
    run(
      "git",
      [
        "-C",
        source,
        "archive",
        "--format=tar",
        `--output=${archive}`,
        revision,
      ],
      env,
    );
    run("tar", ["-xf", archive, "-C", staged], env);
    const modifiedFiles = {};
    for (const name of files) {
      // Git archive emits LF source even when the parent Windows checkout uses
      // CRLF. Apply a canonical patch without changing the user's checkout.
      const path = join(temporary, name);
      writeFileSync(path, readFileSync(join(patches, name), "utf8").replace(/\r\n/g, "\n"));
      run("git", ["-C", staged, "apply", "--check", path], patchEnv);
      const before = {};
      for (const match of readFileSync(path, "utf8").matchAll(
        /^\+\+\+ b\/(.+)$/gm,
      )) {
        before[match[1]] = digest(readFileSync(join(staged, match[1])));
      }
      run("git", ["-C", staged, "apply", path], patchEnv);
      for (const match of readFileSync(path, "utf8").matchAll(
        /^\+\+\+ b\/(.+)$/gm,
      )) {
        const file = match[1];
        if (file.split("/").includes("..") || file.includes("\\"))
          throw new Error("Invalid Ghostty patch path");
        modifiedFiles[file] = digest(readFileSync(join(staged, file)));
        if (modifiedFiles[file] === before[file])
          throw new Error(`Ghostty patch was not applied: ${name} (${file})`);
      }
    }
    writeFileSync(
      join(staged, manifestName),
      JSON.stringify(
        {
          revision,
          fingerprint,
          source,
          patchSet,
          modifiedFiles,
        },
        null,
        2,
      ),
    );
    try {
      renameSync(staged, destination);
    } catch (error) {
      // A concurrent build may have prepared the identical source first.
      if (!existsSync(destination)) throw error;
      return prepareGhosttySource({ source, revision, patches, cache, env });
    }
    return destination;
  } finally {
    rmSync(temporary, { recursive: true, force: true });
  }
}
