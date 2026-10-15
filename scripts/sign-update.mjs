import { mkdtempSync, writeFileSync, rmSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import {
  root,
  run,
  capture,
  json,
  signerEnv,
  releaseBuildEnv,
} from "./tooling.mjs";

export function signUpdate(
  artifact,
  version,
  publicKey = json(join(root, "resources/updater.json")).pubkey,
) {
  return signArtifact("sign", artifact, version, publicKey);
}

export function signNeedle(directory, version, publicKey = json(join(root, "resources/updater.json")).pubkey) {
  return signArtifact("needle", directory, version, publicKey);
}

function signArtifact(command, artifact, version, publicKey) {
  const env = signerEnv();
  // Build scripts must never inherit the release signing credentials.
  const buildEnv = releaseBuildEnv(env);
  run("cargo", ["build", "--locked", "--release", "-p", "newport-release", "--features", "signing"], {
    env: buildEnv,
  });
  const metadata = JSON.parse(
    capture(
      "cargo",
      ["metadata", "--locked", "--no-deps", "--format-version", "1"],
      { env: buildEnv },
    ),
  );
  const binary = join(
    metadata.target_directory,
    ...(buildEnv.CARGO_BUILD_TARGET ? [buildEnv.CARGO_BUILD_TARGET] : []),
    "release",
    process.platform === "win32" ? "newport-release.exe" : "newport-release",
  );
  const temporary = mkdtempSync(join(tmpdir(), "newport-sign-update-"));
  try {
    const keyPath = join(temporary, "public.key");
    writeFileSync(keyPath, publicKey);
    run(binary, [command, artifact, version, keyPath], { env });
  } finally {
    rmSync(temporary, { recursive: true, force: true });
  }
}
