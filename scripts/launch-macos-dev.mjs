// GUI launches must have their own macOS application identity. Executing the
// bundle's binary as a child of an IDE/terminal makes 1Password attribute SSH
// requests to that ancestor and suppress Newport's foreground approval prompt.
import { spawnSync } from "node:child_process";
import { closeSync, openSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { isMain } from "./tooling.mjs";

const checks = new Set([
  "--build-info", "--check-backend", "--check-update-health",
  "--check-needle", "--check-grammars", "--check-pdf",
]);

export function developmentLaunch(bundle, args, env = process.env) {
  bundle = resolve(bundle);
  if (args.some((arg) => checks.has(arg))) {
    // Probes need their actual exit code and stdout, and never request GUI SSH
    // approval. LaunchServices reports launch success, not the app's exit code.
    return { command: join(bundle, "Contents/MacOS/newport"), args };
  }
  const log = join(dirname(bundle), "newport-dev.log");
  const launchArgs = ["-n", "-W", "--stdout", log, "--stderr", log];
  // LaunchServices does not inherit the shell's environment. Preserve the
  // profile and agent selection explicitly, without putting signing secrets
  // or the launching terminal's identity in the application's environment.
  for (const name of [
    "NEWPORT_DATA_DIR", "NEWPORT_ACCOUNT_URL", "SSH_AUTH_SOCK", "RUST_LOG", "RUST_BACKTRACE",
  ]) {
    if (env[name] !== undefined) {
      const value = name === "NEWPORT_DATA_DIR" && env[name] ? resolve(env[name]) : env[name];
      launchArgs.push("--env", `${name}=${value}`);
    }
  }
  if (args.includes("--autostart")) launchArgs.push("-g");
  launchArgs.push(bundle, "--args", ...args);
  return { command: "/usr/bin/open", args: launchArgs, log };
}

if (isMain(import.meta.url)) {
  const [bundle, ...args] = process.argv.slice(2);
  if (!bundle) throw new Error("Supply the signed development app bundle");
  const launch = developmentLaunch(bundle, args);
  if (launch.log) {
    closeSync(openSync(launch.log, "w", 0o600));
    console.error(`Newport application log: ${launch.log}`);
  }
  const result = spawnSync(launch.command, launch.args, { stdio: "inherit" });
  if (result.error) throw result.error;
  process.exitCode = result.status ?? 1;
}
