import { spawnSync } from "node:child_process";
import { run, isMain } from "../tooling.mjs";

export function prepareMetal(check = () => spawnSync(
  "xcrun", ["--sdk", "macosx", "metal", "--version"], { stdio: "ignore" },
).status === 0, download = () => run("xcodebuild", ["-downloadComponent", "MetalToolchain"])) {
  // Xcode 16 bundles Metal; newer Xcode versions install it separately.
  // Finding the launcher alone is insufficient: verify it can actually run.
  if (check()) return;
  download();
  if (!check()) throw new Error("The selected Xcode cannot run the Metal compiler");
}

if (isMain(import.meta.url)) prepareMetal();
