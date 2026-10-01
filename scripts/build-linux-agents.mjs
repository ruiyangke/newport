// One build implementation is shared by release packaging and the Rust test harness.
import { run } from "./tooling.mjs";
run("cargo", ["run", "--locked", "--manifest-path", "tools/test-harness/Cargo.toml", "--", "build-agents"]);
