import { test } from "node:test";
import assert from "node:assert/strict";
import { resolve } from "node:path";
import { developmentLaunch } from "../launch-macos-dev.mjs";

test("interactive dev launches have an independent app identity and retain their profile", () => {
  const bundle = resolve("target/test space/Newport.app");
  const launch = developmentLaunch(bundle, ["--setup-preview"], {
    NEWPORT_DATA_DIR: "test profile & data",
    SSH_AUTH_SOCK: "/tmp/agent socket",
    RUST_BACKTRACE: "1",
    TERM_PROGRAM: "Launching IDE",
    NEWPORT_SIGNING_PRIVATE_KEY: "must not propagate",
  });
  assert.equal(launch.command, "/usr/bin/open");
  assert.deepEqual(launch.args, [
    "-n", "-W", "--stdout", launch.log, "--stderr", launch.log,
    "--env", `NEWPORT_DATA_DIR=${resolve("test profile & data")}`,
    "--env", "SSH_AUTH_SOCK=/tmp/agent socket",
    "--env", "RUST_BACKTRACE=1",
    bundle, "--args", "--setup-preview",
  ]);
});

test("health probes retain direct execution, output and process exit status", () => {
  for (const check of [
    "--build-info", "--check-backend", "--check-update-health",
    "--check-needle", "--check-grammars", "--check-pdf",
  ]) {
    const launch = developmentLaunch("target/Newport.app", [check]);
    assert.equal(launch.command, resolve("target/Newport.app/Contents/MacOS/newport"));
    assert.deepEqual(launch.args, [check]);
    assert.equal(launch.log, undefined);
  }
});

test("login launch remains background and empty profile overrides remain invalid", () => {
  const launch = developmentLaunch("target/Newport.app", ["--autostart"], {
    NEWPORT_DATA_DIR: "",
    PORTHOP_DATA_DIR: "legacy profile",
  });
  assert.ok(launch.args.includes("-g"));
  assert.ok(launch.args.includes("NEWPORT_DATA_DIR="));
  assert.ok(launch.args.includes(`PORTHOP_DATA_DIR=${resolve("legacy profile")}`));
  assert.deepEqual(launch.args.slice(-2), ["--args", "--autostart"]);
});
