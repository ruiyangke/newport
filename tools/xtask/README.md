# xtask

Newport's build, packaging, release and test tooling, written in Rust and run
with `cargo xtask <command>` from the repository root. The alias lives in the
committed `.cargo/config.toml`.

The crate is outside the root workspace and has its own `Cargo.lock` and
`target/` directory. Its test-container dependencies would otherwise enter the
desktop dependency graph, and it always builds for the host while the desktop
crates may build for another target.

## Commands

### Develop

| Command | Does |
| --- | --- |
| `cargo xtask env [--github] [-- <cmd>…]` | Installs the pinned Zig, prepares the patched Ghostty source, and runs `<cmd>` with Zig on `PATH`. `--github` writes the same environment to `GITHUB_ENV`/`GITHUB_PATH`. |
| `cargo xtask dev [--remote-apps] [-- <args>…]` | Runs the desktop app: builds the Linux agents (Docker) when missing or stale, stages grammars when stale and PDFium on Windows, then `cargo run -p newport-app` in that environment. Without `NEWPORT_DATA_DIR` it uses the disposable profile `target/dev-profile`. On macOS the app's output streams to the terminal, and a development Newport still running from the same bundle is quit first. `--remote-apps` compiles Remote apps in (see below). |
| `cargo xtask build [--release] [--target <triple>] [--remote-apps]` | The same resources, then `cargo build -p newport-app` in that environment. |
| `cargo xtask grammars [--release] [--target <triple>]` | Builds every tree-sitter grammar library and stages it in `<target>/<profile>/grammars`. `dev` and `build` do this when the staged libraries are missing or older than `crates/newport-grammars`, `Cargo.lock` or the language list. |
| `cargo xtask pdfium [--release] [--target <triple>]` | Downloads, verifies and stages PDFium for Windows. |
| `cargo xtask agents [--if-changed]` | Builds the Linux agent binaries in Docker; `--if-changed` only when they are missing or older than `tools/agent`'s sources (as `dev` and `build` do). With `NEWPORT_PREBUILT=1` it verifies downloaded ones instead. |
| `cargo xtask artifact record\|verify <dir>` | Records or checks `newport-build.json`: per-file SHA-256 and the commit. |
| `cargo xtask dev-run <binary> [<args>…]` | macOS cargo runner. Bundles, signs and launches `newport` through LaunchServices; other binaries run directly. |

Desktop builds need that environment: use `cargo xtask dev`/`build`, or prefix
any Cargo command with `cargo xtask env --`. `.cargo/config.toml` points
`GHOSTTY_SOURCE_DIR` at a placeholder that `env` overrides, so a plain
`cargo build` stops at the Ghostty build script instead of fetching unpatched
upstream source. The prepared source directory name includes the patch hashes,
so changing a patch rebuilds the terminal engine.

### Check and test

| Command | Does |
| --- | --- |
| `cargo xtask check [rust\|agent\|xtask]` | Formatting, Clippy and tests. With no argument, all three. |
| `cargo xtask test desktop [--headless]` | Profile, preferences and metrics persistence, resource staging, the desktop app's lifecycle. |
| `cargo xtask test ssh` | Disposable SSH fixture: terminal, file and RPC operations. |
| `cargo xtask test remote [--filter <test>]` | Testcontainers with real OpenSSH and remote services. |
| `cargo xtask test https` | Agent Git over HTTPS against a local `git http-backend`. |
| `cargo xtask test runtime` | Agent runtime checks inside the Alpine image. |
| `cargo xtask test git-contract [--output <path>]` | Real agent Git responses through the app's validators. |
| `cargo xtask test git-cli-contract [--output <path>]` | Agent Git CLI responses through the app's validators. |
| `cargo xtask test benchmark --output <path> [--agent <path>] [--before <rev>] [--after <rev>] [--size <size>]… [--rounds <n>] [--latency-ms <ms>]` | Git benchmark; `--output` is required and `--rounds` defaults to 3. |
| `cargo xtask test keychain` | macOS: signs a test bundle and runs the protected Keychain test. |

### Package and release

| Command | Does |
| --- | --- |
| `cargo xtask package [--debug] [--skip-build] [--target <triple>] [--nsis]` | Stages the app and verifies it. macOS: `Newport.app` plus a zip. Windows: a zip, and with `--nsis` the installer. |
| `cargo xtask release verify [--ci]` | Checks the tag against every version and, with `--ci`, whether an existing CI run can be reused. |
| `cargo xtask release setup-signing` | CI: writes the certificate and profile and creates a temporary keychain. |
| `cargo xtask release macos [--skip-build]` | Developer ID signing, notarization, stapling, and the signed update tarball and manifest fragment. |
| `cargo xtask release windows-update <installer> --arch <arch> --app-executable <exe> --output <dir>` | Signed Windows update payload and manifest fragment. |
| `cargo xtask release merge <latest.json> <fragment>… --version <v> --output <path>` | Merges the fragments, verifies every signature, and writes `SHA256SUMS`. |
| `cargo xtask release sign-needle` | Verifies `resources/agents` and signs the agent release. |

Signing uses `newport-release` as a library. The release workflow builds the
xtask binary in a step without secrets and runs that binary with them; commands
that build the desktop remove the signing secrets from the child environment.

## Local signing

On macOS, `cargo xtask dev` (or `cargo xtask env -- cargo run -p newport-app`)
goes through `tools/xtask/runner.sh`, which runs `dev-run`. It signs with the identity, provisioning profile and
entitlements from `NEWPORT_SIGNING_IDENTITY`, `NEWPORT_PROVISIONING_PROFILE` and
`NEWPORT_SIGNING_ENTITLEMENTS`, or from the git-ignored `.cargo/signing.toml`:

```toml
identity = "<certificate SHA-1>"
provisioning_profile = ".cargo/Newport.provisionprofile"
entitlements = ".cargo/Newport.entitlements"
```

Relative paths are resolved from the repository root. Without signing settings
the binary runs unsigned.

## Remote apps

`--remote-apps` on `dev` and `build`, and `cargo xtask test remote-apps
[<cargo test args>…]` (newport-gpui's tests, the only way that code is
tested), compile Remote apps in from a private newport-remote checkout:
`NEWPORT_REMOTE_DIR`, by default `../newport-remote` beside this repository.
Without it, build the free edition.

For the build only, the streaming crates are added to newport-gpui with
`cargo add`, GPUI is patched to this lockfile's revision on the command line,
and the agent archives that Install sends are made from newport-remote's static
agents (rebuilt in Docker when older than their sources). A guard takes the
crates out of `Cargo.toml` and `Cargo.lock` again when the build ends, fails or
is interrupted with Ctrl-C; a build that was killed outright is cleaned up by
the next `dev`, `build` or `test`. The checkout never keeps a reference to the
private crates.

## Not in xtask

- `tests/agent-runtime/private-directory.sh`, `tests/remote/start.sh` and
  `tests/remote/docker/start.sh` run inside containers.
- `tools/xtask/runner.sh` is the cargo runner shim: Cargo needs a runner that
  exists before anything is built.
- `scripts/test-windows-activation.ps1` is a manual Windows desktop check.
