import { verifyArtifact } from "./ci/artifact.mjs";
import { existsSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
const root = fileURLToPath(new URL("../", import.meta.url));
if (process.env.NEWPORT_PREBUILT === "1") {
  verifyArtifact("resources/agents");
} else if (process.platform === "win32") {
  // Linux binaries are built in the Linux CI job, never replaced with stubs.
  for (const arch of ["x86_64", "aarch64"]) {
    if (!existsSync(`${root}/resources/agents/newport-agent-${arch}`)) {
      throw new Error(
        "Linux agent binaries are missing. Copy the linux-agents CI artifact into resources/agents before building on Windows.",
      );
    }
  }
} else {
  // The Rust harness owns the Linux build; CI and local builds use this entry.
  const result = spawnSync(
    "cargo",
    ["run", "--locked", "--manifest-path", "tools/test-harness/Cargo.toml", "--", "build-agents"],
    { cwd: root, stdio: "inherit" },
  );
  if (result.error) throw result.error;
  process.exit(result.status ?? 1);
}
