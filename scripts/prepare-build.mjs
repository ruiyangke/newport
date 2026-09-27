import { spawnSync } from "node:child_process";
import { verifyArtifact } from "./ci/artifact.mjs";

if (process.env.PORTHOP_PREBUILT === "1") {
  verifyArtifact("src-tauri/agents");
  verifyArtifact("dist");
} else {
  for (const script of ["build:agent", "build"]) {
    const result = spawnSync(
      process.platform === "win32" ? "npm.cmd" : "npm",
      ["run", script],
      {
        stdio: "inherit",
        shell: process.platform === "win32",
      },
    );
    if (result.error) throw result.error;
    if (result.status !== 0) process.exit(result.status ?? 1);
  }
}
