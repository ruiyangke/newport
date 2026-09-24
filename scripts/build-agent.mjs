import { existsSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
const root = fileURLToPath(new URL("../", import.meta.url));
if (process.platform === "win32") {
  // Linux binaries are built in the Linux CI job, never replaced with stubs.
  for (const arch of ["x86_64", "aarch64"]) {
    if (!existsSync(`${root}/src-tauri/agents/porthop-agent-${arch}`)) {
      throw new Error(
        "Linux agent binaries are missing. Copy the linux-agents CI artifact into src-tauri/agents before building on Windows.",
      );
    }
  }
} else {
  const result = spawnSync("bash", ["scripts/build-agent.sh"], {
    cwd: root,
    stdio: "inherit",
  });
  if (result.error) throw result.error;
  process.exit(result.status ?? 1);
}
